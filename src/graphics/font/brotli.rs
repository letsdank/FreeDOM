//! Brotli bitstream parsing for WOFF 2.0.
//!
//! The decoder currently handles uncompressed meta-blocks. Parsing support
//! for compressed meta-block headers, prefix codes, contexts, and commands is
//! being implemented separately and is not yet connected to the decoder.
//!
//! Specification: <https://www.rfc-editor.org/rfc/rfc7932.html>
mod bit_reader;
mod block;
mod command;
mod context;
mod huffman;
mod prefix_code;

pub use self::bit_reader::BitReader;
pub use self::huffman::HuffmanTree;
pub use self::prefix_code::{read_prefix_code, read_simple_prefix_code};

use self::block::{BlockPartition, read_block_partition};
use self::command::{InsertCopyCommand, read_insert_copy_command};
use self::context::{LiteralContextMode, distance_context_id, literal_context_id};

use super::sfnt::FontError;

const LITERAL_ALPHABET_SIZE: usize = 256;
const COMMAND_ALPHABET_SIZE: usize = 704;
const DISTANCE_SHORT_CODE_COUNT: usize = 16;
const DISTANCE_CODE_GROUP_COUNT: usize = 48;

pub fn decode_uncompressed_meta_blocks(
    data: &[u8],
    maximum_output_size: usize,
) -> Result<Vec<u8>, FontError> {
    let mut reader = BitReader::new(data);
    let _window_bits = read_window_bits(&mut reader)?;
    let mut output = Vec::new();

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

        if is_last {
            return Err(error(
                reader.bit_position(),
                "compressed Brotli meta-blocks are not supported yet",
            ));
        }

        if !reader.read_bit()? {
            return Err(error(
                reader.bit_position(),
                "compressed Brotli meta-blocks are not supported yet",
            ));
        }

        consume_zero_padding(&mut reader)?;

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

        output.extend_from_slice(reader.read_aligned_bytes(output_length)?);
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

#[derive(Clone, Debug)]
struct CompressedMetaBlockHeader {
    literal_partition: BlockPartition,
    command_partition: BlockPartition,
    distance_partition: BlockPartition,
    distance_postfix_bits: u8,
    direct_distance_code_count: usize,
    literal_context_modes: Vec<LiteralContextMode>,
    literal_tree_count: usize,
    literal_context_map: Vec<usize>,
    distance_tree_count: usize,
    distance_context_map: Vec<usize>,
    literal_trees: Vec<HuffmanTree>,
    command_trees: Vec<HuffmanTree>,
    distance_trees: Vec<HuffmanTree>,
}

impl CompressedMetaBlockHeader {
    fn read_command_symbol(&mut self, reader: &mut BitReader<'_>) -> Result<usize, FontError> {
        let block_type = self.command_partition.take_block_type(reader)?;

        let tree = self.command_trees.get(block_type).ok_or_else(|| {
            error(
                reader.bit_position(),
                "Brotli command block type has no Huffman tree",
            )
        })?;

        tree.decode(reader)
    }

    fn read_literal(
        &mut self,
        reader: &mut BitReader<'_>,
        previous: u8,
        before_previous: u8,
    ) -> Result<u8, FontError> {
        let block_type = self.literal_partition.take_block_type(reader)?;

        let mode = *self.literal_context_modes.get(block_type).ok_or_else(|| {
            error(
                reader.bit_position(),
                "Brotli literal block type has no context mode",
            )
        })?;

        let context = literal_context_id(mode, previous, before_previous);

        let map_index = block_type
            .checked_mul(64)
            .and_then(|index| index.checked_add(context))
            .ok_or_else(|| {
                error(
                    reader.bit_position(),
                    "Brotli literal context index overflow",
                )
            })?;

        let tree_index = *self.literal_context_map.get(map_index).ok_or_else(|| {
            error(
                reader.bit_position(),
                "Brotli literal context is missing from the context map",
            )
        })?;

        let tree = self.literal_trees.get(tree_index).ok_or_else(|| {
            error(
                reader.bit_position(),
                "Brotli literal context refers to a missing Huffman tree",
            )
        })?;

        let symbol = tree.decode(reader)?;

        u8::try_from(symbol).map_err(|_| {
            error(
                reader.bit_position(),
                "Brotli literal symbol is outside the byte range",
            )
        })
    }

    fn read_distance_symbol(
        &mut self,
        reader: &mut BitReader<'_>,
        copy_length: usize,
    ) -> Result<usize, FontError> {
        let block_type = self.distance_partition.take_block_type(reader)?;
        let context = distance_context_id(copy_length);

        let map_index = block_type
            .checked_mul(4)
            .and_then(|index| index.checked_add(context))
            .ok_or_else(|| {
                error(
                    reader.bit_position(),
                    "Brotli distance context index overflow",
                )
            })?;

        let tree_index = *self.distance_context_map.get(map_index).ok_or_else(|| {
            error(
                reader.bit_position(),
                "Brotli distance context is missing from the context map",
            )
        })?;

        let tree = self.distance_trees.get(tree_index).ok_or_else(|| {
            error(
                reader.bit_position(),
                "Brotli distance context refers to a missing Huffman tree",
            )
        })?;

        tree.decode(reader)
    }

    fn read_command(&mut self, reader: &mut BitReader<'_>) -> Result<InsertCopyCommand, FontError> {
        let symbol = self.read_command_symbol(reader)?;
        read_insert_copy_command(reader, symbol)
    }
}

fn read_compressed_meta_block_header_prefix(
    reader: &mut BitReader<'_>,
) -> Result<CompressedMetaBlockHeader, FontError> {
    let literal_partition = read_block_partition(reader)?;
    let command_partition = read_block_partition(reader)?;
    let distance_partition = read_block_partition(reader)?;

    let distance_postfix_bits = reader.read_bits(2)? as u8;
    let direct_distance_code_count =
        (reader.read_bits(4)? as usize) << usize::from(distance_postfix_bits);

    let mut literal_context_modes = Vec::with_capacity(literal_partition.block_type_count);

    for _ in 0..literal_partition.block_type_count {
        let mode = match reader.read_bits(2)? {
            0 => LiteralContextMode::Lsb6,
            1 => LiteralContextMode::Msb6,
            2 => LiteralContextMode::Utf8,
            3 => LiteralContextMode::Signed,
            _ => unreachable!(),
        };

        literal_context_modes.push(mode);
    }

    let literal_tree_count = read_block_type_count(reader)?;
    let literal_context_map_size = literal_partition
        .block_type_count
        .checked_mul(64)
        .ok_or_else(|| {
            error(
                reader.bit_position(),
                "Brotli literal context map is too large",
            )
        })?;
    let literal_context_map =
        read_context_map(reader, literal_context_map_size, literal_tree_count)?;

    let distance_tree_count = read_block_type_count(reader)?;
    let distance_context_map_size = distance_partition
        .block_type_count
        .checked_mul(4)
        .ok_or_else(|| {
            error(
                reader.bit_position(),
                "Brotli distance context map is too large",
            )
        })?;
    let distance_context_map =
        read_context_map(reader, distance_context_map_size, distance_tree_count)?;

    let literal_trees = read_prefix_code_array(reader, literal_tree_count, LITERAL_ALPHABET_SIZE)?;

    let command_trees = read_prefix_code_array(
        reader,
        command_partition.block_type_count,
        COMMAND_ALPHABET_SIZE,
    )?;

    let distance_code_group_count = DISTANCE_CODE_GROUP_COUNT
        .checked_shl(u32::from(distance_postfix_bits))
        .ok_or_else(|| {
            error(
                reader.bit_position(),
                "Brotli distance code-group count overflows",
            )
        })?;

    let distance_alphabet_size = DISTANCE_SHORT_CODE_COUNT
        .checked_add(direct_distance_code_count)
        .and_then(|count| count.checked_add(distance_code_group_count))
        .ok_or_else(|| {
            error(
                reader.bit_position(),
                "Brotli distance alphabet size overflows",
            )
        })?;

    let distance_trees =
        read_prefix_code_array(reader, distance_tree_count, distance_alphabet_size)?;

    Ok(CompressedMetaBlockHeader {
        literal_partition,
        command_partition,
        distance_partition,
        distance_postfix_bits,
        direct_distance_code_count,
        literal_context_modes,
        literal_tree_count,
        literal_context_map,
        distance_tree_count,
        distance_context_map,
        literal_trees,
        command_trees,
        distance_trees,
    })
}

fn read_context_map(
    reader: &mut BitReader<'_>,
    context_map_size: usize,
    tree_count: usize,
) -> Result<Vec<usize>, FontError> {
    if tree_count == 0 {
        return Err(error(
            reader.bit_position(),
            "Brotli context map has no prefix trees",
        ));
    }

    if tree_count == 1 {
        return Ok(vec![0; context_map_size]);
    }

    let rle_max = if reader.read_bit()? {
        reader.read_bits(4)? as usize + 1
    } else {
        0
    };

    let tree = read_prefix_code(reader, tree_count + rle_max)?;
    let mut context_map = Vec::with_capacity(context_map_size);

    while context_map.len() < context_map_size {
        let symbol = tree.decode(reader)?;

        if symbol == 0 {
            context_map.push(0);
            continue;
        }

        if symbol <= rle_max {
            let repeat_count = (1_usize << symbol)
                .checked_add(reader.read_bits(symbol)? as usize)
                .ok_or_else(|| {
                    error(
                        reader.bit_position(),
                        "Brotli context-map zero run overflows",
                    )
                })?;

            let end = context_map
                .len()
                .checked_add(repeat_count)
                .ok_or_else(|| error(reader.bit_position(), "Brotli context-map size overflows"))?;

            if end > context_map_size {
                return Err(error(
                    reader.bit_position(),
                    "Brotli context-map zero run exceeds its declared size",
                ));
            }

            context_map.resize(end, 0);
            continue;
        }

        let tree_index = symbol - rle_max;

        if tree_index >= tree_count {
            return Err(error(
                reader.bit_position(),
                "Brotli context-map tree index exceeds its tree count",
            ));
        }

        context_map.push(tree_index);
    }

    if reader.read_bit()? {
        inverse_move_to_front(&mut context_map);
    }

    let mut seen = vec![false; tree_count];

    for &tree_index in &context_map {
        seen[tree_index] = true;
    }

    if seen.iter().any(|present| !present) {
        return Err(error(
            reader.bit_position(),
            "Brotli context map does not reference every prefix tree",
        ));
    }

    Ok(context_map)
}

fn read_prefix_code_array(
    reader: &mut BitReader<'_>,
    count: usize,
    alphabet_size: usize,
) -> Result<Vec<HuffmanTree>, FontError> {
    let mut trees = Vec::with_capacity(count);

    for _ in 0..count {
        trees.push(read_prefix_code(reader, alphabet_size)?);
    }

    Ok(trees)
}

fn inverse_move_to_front(values: &mut [usize]) {
    let mut entries = Vec::with_capacity(256);

    for value in 0..256 {
        entries.push(value);
    }

    for index in values {
        let value = entries[*index];

        for position in (1..=*index).rev() {
            entries[position] = entries[position - 1];
        }

        entries[0] = value;
        *index = value;
    }
}

fn error(offset: usize, message: impl Into<String>) -> FontError {
    FontError {
        offset,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests;
