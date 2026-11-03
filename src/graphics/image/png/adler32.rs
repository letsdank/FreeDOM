//! Adler-32 checksums for zlib streams.
//!
//! RFC 1950, sections 2.2 and 9:
//! <https://www.rfc-editor.org/rfc/rfc1950#section-2.2>

use std::io::{self, Read, Write};

use super::DeflateBitReader;

const MODULUS: u32 = 65_521;

// With normalized sums, at most 5552 bytes can be accumulated
// without overflowing u32.
const NMAX: usize = 5_552;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Adler32 {
    a: u32,
    b: u32,
}

impl Adler32 {
    pub fn new() -> Self {
        Self { a: 1, b: 0 }
    }

    pub fn update(&mut self, data: &[u8]) {
        for chunk in data.chunks(NMAX) {
            for &byte in chunk {
                self.a += u32::from(byte);
                self.b += self.a;
            }

            self.a %= MODULUS;
            self.b %= MODULUS;
        }
    }

    pub fn checksum(&self) -> u32 {
        (self.b << 16) | self.a
    }
}

impl Default for Adler32 {
    fn default() -> Self {
        Self::new()
    }
}

/// Tracks bytes successfully written to the underlying writer.
pub struct Adler32Writer<W> {
    inner: W,
    adler: Adler32,
}

impl<W> Adler32Writer<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            adler: Adler32::new(),
        }
    }

    pub fn checksum(&self) -> u32 {
        self.adler.checksum()
    }

    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for Adler32Writer<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(bytes)?;

        if written > bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "writer returned an invalid byte count",
            ));
        }

        self.adler.update(&bytes[..written]);

        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Reads and verifies the zlib Adler-32 trailer after the final DEFLATE block.
///
/// The caller must decode every block before calling this function. It aligns
/// to the next byte and consumes the four trailer bytes in big-endian order.
pub fn verify_zlib_adler32<R: Read>(
    bits: &mut DeflateBitReader<R>,
    checksum: u32,
) -> io::Result<()> {
    bits.align_to_byte();

    let mut bytes = [0; 4];

    for byte in &mut bytes {
        *byte = bits.read_bits(8)? as u8;
    }

    if u32::from_be_bytes(bytes) != checksum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "zlib Adler-32 checksum mismatch",
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::graphics::image::png_format::{
        DeflateBlockType, IdatReader, SIGNATURE, ZlibHeader, copy_stored_block, read_block_header,
    };

    #[test]
    fn matches_known_check_values_and_incremental_updates() {
        let mut adler = Adler32::new();

        assert_eq!(adler.checksum(), 1);

        adler.update(b"1234");
        adler.update(b"56789");

        assert_eq!(adler.checksum(), 0x091e_01de);

        let mut adler = Adler32::new();
        adler.update(&[0xff; 6000]);

        assert_eq!(adler.checksum(), 0xa497_59ea);
    }

    struct PartialWriter(Vec<u8>);

    impl Write for PartialWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let count = bytes.len().min(2);
            self.0.extend_from_slice(&bytes[..count]);

            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn writer_hashes_only_bytes_actually_written() {
        let mut writer = Adler32Writer::new(PartialWriter(Vec::new()));

        writer.write_all(b"abcde").unwrap();

        assert_eq!(writer.checksum(), 0x05c8_01f0);
        assert_eq!(writer.into_inner().0, b"abcde");
    }

    #[test]
    fn verifies_two_stored_blocks_and_zlib_trailer() {
        let bytes = [
            // zlib header.
            0x78, 0x01, // Non-final stored block: "abc".
            0x00, 3, 0, 0xfc, 0xff, b'a', b'b', b'c', // Final stored block: "de".
            0x01, 2, 0, 0xfd, 0xff, b'd', b'e', // Adler-32 of "abcde".
            0x05, 0xc8, 0x01, 0xf0,
        ];

        let mut png = SIGNATURE.to_vec();

        // Valid 1x1 RGBA IHDR.
        png.extend_from_slice(&[
            0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15,
            0xc4, 0x89,
        ]);

        // Split both the zlib header and its trailer across IDAT chunks.
        let parts = [
            &bytes[..1],
            &bytes[1..bytes.len() - 2],
            &bytes[bytes.len() - 2..],
        ];

        for part in parts {
            png.extend_from_slice(&(part.len() as u32).to_be_bytes());
            png.extend_from_slice(b"IDAT");
            png.extend_from_slice(part);

            // IDAT CRCs are not checked yet.
            png.extend_from_slice(&[0; 4]);
        }

        png.extend_from_slice(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xae, 0x42, 0x60, 0x82]);

        let mut input = IdatReader::new(&png).unwrap();
        ZlibHeader::read_from(&mut input).unwrap();

        let mut bits = DeflateBitReader::new(input);
        let mut writer = Adler32Writer::new(Vec::new());
        let mut remaining = 5;

        loop {
            let header = read_block_header(&mut bits).unwrap();

            assert_eq!(header.block_type, DeflateBlockType::Stored);

            let copied = copy_stored_block(&mut bits, &mut writer, remaining).unwrap();

            remaining -= copied;

            if header.is_final {
                break;
            }
        }

        verify_zlib_adler32(&mut bits, writer.checksum()).unwrap();

        assert_eq!(writer.into_inner(), b"abcde");
    }

    #[test]
    fn rejects_wrong_and_truncated_trailers() {
        let mut wrong = DeflateBitReader::new(&[0x05, 0xc8, 0x01, 0xf1][..]);

        assert_eq!(
            verify_zlib_adler32(&mut wrong, 0x05c8_01f0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );

        let mut short = DeflateBitReader::new(&[0x05, 0xc8][..]);

        assert_eq!(
            verify_zlib_adler32(&mut short, 0x05c8_01f0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn aligns_after_a_non_byte_aligned_block() {
        let mut bits = DeflateBitReader::new(&[0b1111_1101, 0, 0, 0, 1][..]);

        assert_eq!(bits.read_bits(3).unwrap(), 5);

        verify_zlib_adler32(&mut bits, 1).unwrap();
    }
}
