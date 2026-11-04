//! Literal-only DEFLATE encoding with fixed Huffman codes.
//!
//! RFC 1951, sections 3.1.1, 3.2.3 and 3.2.6:
//! <https://www.rfc-editor.org/rfc/rfc1951#section-3.2.6>

use std::io::{self, Write};

use super::DeflateBitWriter;

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
        let (code, count) = if byte <= 143 {
            (0x30 + u32::from(byte), 8)
        } else {
            (0x190 + u32::from(byte) - 144, 9)
        };

        // Huffman codes are transmitted most-significant bit first.
        let reversed = code.reverse_bits() >> (32 - count);

        bits.write_bits(reversed, count as u8)?;
    }

    bits.write_bits(0, 7)?; // Symbol 256: end of block.

    bits.finish()
}

#[cfg(test)]
mod tests;
