//! Minimal zlib encoding with stored or fixed-Huffman DEFLATE blocks.
//!
//! RFC 1950, section 2.2:
//! <https://www.rfc-editor.org/rfc/rfc1950#section-2.2>
//!
//! RFC 1951, section 3.2.4:
//! <https://www.rfc-editor.org/rfc/rfc1951#section-3.2.4>

use std::io::{self, Write};

use super::super::{Adler32, encode_fixed_literals, encode_fixed_lz77};

const MAX_STORED_BLOCK: usize = u16::MAX as usize;

/// Writes one complete zlib stream with DEFLATE stored blocks.
///
/// The payload is not compressed. On Error, the writer may contain a
/// partial stream. The writer is not flushed. An empty input still emits
/// a final block.
pub fn encode_zlib_stored(input: &[u8], mut writer: impl Write) -> io::Result<()> {
    // CM=8, CINFO=7, no dictionary, FCHECK makes the header
    // divisible by 31.
    writer.write_all(&[0x78, 0x01])?;

    let mut checksum = Adler32::new();
    let mut remaining = input;

    loop {
        let count = remaining.len().min(MAX_STORED_BLOCK);
        let (chunk, rest) = remaining.split_at(count);

        let length = count as u16;

        // BFINAL is bit 0; BTYPE=00. The remaining bits are padding.
        writer.write_all(&[u8::from(rest.is_empty())])?;

        writer.write_all(&length.to_le_bytes())?;
        writer.write_all(&(!length).to_le_bytes())?;
        writer.write_all(chunk)?;

        checksum.update(chunk);

        if rest.is_empty() {
            break;
        }

        remaining = rest;
    }

    writer.write_all(&checksum.checksum().to_be_bytes())?;

    Ok(())
}

/// Writes a complete zlib stream containing one fixed-Huffman block.
///
/// The block contains literals only, without LZ77 matches. The writer is
/// not flushed; an I/O error may leave a partial stream.
pub fn encode_zlib_fixed_literals(input: &[u8], writer: impl Write) -> io::Result<()> {
    encode_zlib_fixed(input, writer, false)
}

/// Writes a complete zlib stream using fixed Huffman codes and LZ77 matches.
///
/// The writer is not flushed; an error may leave a partial stream. For
/// incompressible input the result can be larger than a stored stream.
pub fn encode_zlib_fixed_lz77(input: &[u8], writer: impl Write) -> io::Result<()> {
    encode_zlib_fixed(input, writer, true)
}

fn encode_zlib_fixed(input: &[u8], mut writer: impl Write, use_lz77: bool) -> io::Result<()> {
    // CM=8, CINFO=7, no preset dictionary.
    writer.write_all(&[0x78, 0x01])?;

    if use_lz77 {
        encode_fixed_lz77(input, &mut writer)?;
    } else {
        encode_fixed_literals(input, &mut writer)?;
    }

    let mut checksum = Adler32::new();
    checksum.update(input);

    writer.write_all(&checksum.checksum().to_be_bytes())?;

    Ok(())
}

#[cfg(test)]
#[path = "encode_tests.rs"]
mod tests;
