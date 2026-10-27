//! Brotli bitstream parsing for WOFF 2.0.
//!
//! The decoder handles uncompressed meta-blocks and compressed meta-blocks
//! that finish using literal insertion alone.
//!
//! Backward copies and static dictionary references are not supported yet.
//!
//! Specification: <https://www.rfc-editor.org/rfc/rfc7932.html>
mod bit_reader;
mod block;
mod command;
mod compressed;
mod context;
mod dictionary;
mod distance;
mod huffman;
mod prefix_code;

pub use self::bit_reader::BitReader;
use self::compressed::decode_compressed_meta_block;
use self::distance::DistanceHistory;
pub use self::huffman::HuffmanTree;
pub use self::prefix_code::{read_prefix_code, read_simple_prefix_code};

#[cfg(test)]
use self::block::{BlockPartition, read_block_partition};

#[cfg(test)]
use self::command::{InsertCopyCommand, read_insert_copy_command};

#[cfg(test)]
use self::context::{LiteralContextMode, distance_context_id, literal_context_id};

use super::sfnt::FontError;

pub fn decode_brotli(data: &[u8], maximum_output_size: usize) -> Result<Vec<u8>, FontError> {
    let mut reader = BitReader::new(data);
    let window_bits = read_window_bits(&mut reader)?;
    let window_size = (1_usize << window_bits) - 16;
    let mut output = Vec::new();
    let mut distance_history = DistanceHistory::new();

    loop {
        let is_last = reader.read_bit()?;
        let is_last_empty = is_last && reader.read_bit()?;

        if is_last_empty {
            consume_zero_padding(&mut reader)?;

            if reader.bits_remaining() != 0 {
                return Err(error(
                    reader.bit_position(),
                    "Brotli stream has bytes after its final empty meta-block",
                ));
            }

            return Ok(output);
        }

        let nibble_count = read_meta_block_nibble_count(&mut reader)?;

        if nibble_count == 0 {
            skip_empty_meta_block(&mut reader)?;
            continue;
        }

        let output_length = read_meta_block_length(&mut reader, nibble_count)?;

        // ISUNCOMPRESSED is present only for non-final meta-blocks.
        let is_uncompressed = !is_last && reader.read_bit()?;

        let new_length = output
            .len()
            .checked_add(output_length)
            .ok_or_else(|| error(reader.bit_position(), "Brotli output size overflows"))?;

        if new_length > maximum_output_size {
            return Err(error(
                reader.bit_position(),
                "Brotli output exceeds the configured size limit",
            ));
        }

        output
            .try_reserve(output_length)
            .map_err(|_| error(reader.bit_position(), "cannot allocate Brotli output"))?;

        if is_uncompressed {
            consume_zero_padding(&mut reader)?;
            output.extend_from_slice(reader.read_aligned_bytes(output_length)?);
        } else {
            decode_compressed_meta_block(
                &mut reader,
                &mut output,
                output_length,
                window_size,
                &mut distance_history,
            )?;
        }

        if is_last {
            consume_zero_padding(&mut reader)?;

            if reader.bits_remaining() != 0 {
                return Err(error(
                    reader.bit_position(),
                    "Brotli stream has bytes after its final meta-block",
                ));
            }

            return Ok(output);
        }
    }
}

fn read_window_bits(reader: &mut BitReader<'_>) -> Result<u8, FontError> {
    if !reader.read_bit()? {
        return Ok(16);
    }

    let short_code = reader.read_bits(3)? as u8;

    if short_code != 0 {
        return Ok(17 + short_code);
    }

    match reader.read_bits(3)? as u8 {
        0 => Ok(17),
        1 => Err(error(
            reader.bit_position(),
            "Brotli stream uses the reserved WBITS value",
        )),
        value => Ok(8 + value),
    }
}

fn read_meta_block_nibble_count(reader: &mut BitReader<'_>) -> Result<usize, FontError> {
    match reader.read_bits(2)? {
        0 => Ok(4),
        1 => Ok(5),
        2 => Ok(6),
        3 => Ok(0),
        _ => unreachable!(),
    }
}

fn read_meta_block_length(
    reader: &mut BitReader<'_>,
    nibble_count: usize,
) -> Result<usize, FontError> {
    let length_minus_one = reader.read_bits(nibble_count * 4)?;

    if nibble_count > 4 {
        let highest_nibble = (length_minus_one >> ((nibble_count - 1) * 4)) & 0x0f;

        if highest_nibble == 0 {
            return Err(error(
                reader.bit_position(),
                "Brotli meta-block length has a redundant leading zero nibble",
            ));
        }
    }

    usize::try_from(length_minus_one)
        .ok()
        .and_then(|length| length.checked_add(1))
        .ok_or_else(|| error(reader.bit_position(), "Brotli meta-block length overflows"))
}

fn read_variable_length_u8(reader: &mut BitReader<'_>) -> Result<usize, FontError> {
    if !reader.read_bit()? {
        return Ok(0);
    }

    let bit_count = reader.read_bits(3)? as usize;

    if bit_count == 0 {
        return Ok(1);
    }

    let suffix = reader.read_bits(bit_count)? as usize;

    Ok((1_usize << bit_count) | suffix)
}

fn read_block_type_count(reader: &mut BitReader<'_>) -> Result<usize, FontError> {
    read_variable_length_u8(reader)?
        .checked_add(1)
        .ok_or_else(|| error(reader.bit_position(), "Brotli block type count overflows"))
}

fn skip_empty_meta_block(reader: &mut BitReader<'_>) -> Result<(), FontError> {
    if reader.read_bit()? {
        return Err(error(
            reader.bit_position(),
            "Brotli empty meta-block has its reserved bit set",
        ));
    }

    let skip_byte_count = reader.read_bits(2)? as usize;

    let metadata_length = if skip_byte_count == 0 {
        0
    } else {
        let length_minus_one = reader.read_bits(skip_byte_count * 8)?;

        if skip_byte_count > 1 {
            let most_significant_byte = (length_minus_one >> ((skip_byte_count - 1) * 8)) & 0xff;

            if most_significant_byte == 0 {
                return Err(error(
                    reader.bit_position(),
                    "Brotli metadata length has a redundant leading zero byte",
                ));
            }
        }

        usize::try_from(length_minus_one)
            .ok()
            .and_then(|length| length.checked_add(1))
            .ok_or_else(|| error(reader.bit_position(), "Brotli metadata length overflows"))?
    };

    consume_zero_padding(reader)?;
    reader.read_aligned_bytes(metadata_length)?;

    Ok(())
}

fn consume_zero_padding(reader: &mut BitReader<'_>) -> Result<(), FontError> {
    let padding_bits = (8 - reader.bit_position() % 8) % 8;

    if reader.read_bits(padding_bits)? != 0 {
        return Err(error(
            reader.bit_position(),
            "Brotli byte-alignment padding must be zero",
        ));
    }

    Ok(())
}

fn error(offset: usize, message: impl Into<String>) -> FontError {
    FontError {
        offset,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests;
