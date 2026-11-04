//! DEFLATE encoding with fixed Huffman codes.
//!
//! RFC 1951, sections 3.1.1, 3.2.3 and 3.2.6:
//! <https://www.rfc-editor.org/rfc/rfc1951#section-3.2.6>

use std::io::{self, Write};

use super::tables::{DISTANCE_BASE, DISTANCE_EXTRA, LENGTH_BASE, LENGTH_EXTRA};
use super::{DeflateBitWriter, Lz77Finder, Lz77Token};

fn write_symbol<W: Write>(bits: &mut DeflateBitWriter<W>, symbol: u16) -> io::Result<()> {
    let (code, count) = match symbol {
        0..=143 => (0x30 + u32::from(symbol), 8u8),
        144..=255 => (0x190 + u32::from(symbol) - 144, 9),
        256..=279 => (u32::from(symbol) - 256, 7),
        280..=287 => (0xc0 + u32::from(symbol) - 280, 8),

        _ => unreachable!("invalid fixed-Huffman symbol"),
    };

    bits.write_bits(code.reverse_bits() >> (32 - count), count)
}

fn write_match<W: Write>(
    bits: &mut DeflateBitWriter<W>,
    length: usize,
    distance: usize,
) -> io::Result<()> {
    debug_assert!((3..=258).contains(&length));
    debug_assert!((1..=32_768).contains(&distance));

    let length_index = LENGTH_BASE
        .iter()
        .rposition(|&base| base <= length)
        .expect("valid match length");

    write_symbol(bits, 257 + length_index as u16)?;

    bits.write_bits(
        (length - LENGTH_BASE[length_index]) as u32,
        LENGTH_EXTRA[length_index],
    )?;

    let distance_index = DISTANCE_BASE
        .iter()
        .rposition(|&base| base <= distance)
        .expect("valid match distance");

    // Fixed distance codes are five bits long.
    bits.write_bits((distance_index as u32).reverse_bits() >> 27, 5)?;

    bits.write_bits(
        (distance - DISTANCE_BASE[distance_index]) as u32,
        DISTANCE_EXTRA[distance_index],
    )
}

/// Writer one final fixed-Huffman DEFLATE block containing literals only.
///
/// Returns the writer after padding the final byte with zero bits.
/// This writes raw DEFLATE, not a zlib stream, and does not flush.
/// Output can be partially written on error. Without LZ77 matches,
/// output can exceed input size.
pub fn encode_fixed_literals<W: Write>(input: &[u8], writer: W) -> io::Result<W> {
    let mut bits = DeflateBitWriter::new(writer);

    bits.write_bits(1, 1)?; // BFINAL=1.
    bits.write_bits(1, 2)?; // BTYPE=01: fixed Huffman.

    for &byte in input {
        write_symbol(&mut bits, u16::from(byte))?;
    }

    write_symbol(&mut bits, 256)?; // End of block.

    bits.finish()
}

/// Writes one final fixed-Huffman DEFLATE block with bounded LZ77 matches.
///
/// The result is raw DEFLATE, not zlib. The underlying writer is not
/// flushed. A write error may leave a partially written stream; output
/// can be larger than the input for incompressible data.
pub fn encode_fixed_lz77<W: Write>(input: &[u8], writer: W) -> io::Result<W> {
    // Allocate the match finder's tables before writing to the output.
    let finder = Lz77Finder::new(input)?;

    let mut bits = DeflateBitWriter::new(writer);

    bits.write_bits(1, 1)?; // BFINAL=1.
    bits.write_bits(1, 2)?; // BTYPE=01: fixed Huffman.

    for token in finder {
        match token {
            Lz77Token::Literal(byte) => {
                write_symbol(&mut bits, u16::from(byte))?;
            }

            Lz77Token::Match { length, distance } => {
                write_match(&mut bits, usize::from(length), usize::from(distance))?;
            }
        }
    }

    write_symbol(&mut bits, 256)?; // End of block.

    bits.finish()
}

#[cfg(test)]
#[path = "fixed_encode_tests.rs"]
mod tests;
