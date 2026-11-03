//! Bit-level input for DEFLATE streams.
//!
//! RFC 1951, section 3.1.1:
//! <https://www.rfc-editor.org/rfc/rfc1951#section-3.1.1>
//!
//! Integer fields are read least-significant bit first. Huffman codes have
//! different bit-order rules and must be interpreted separately.

use std::io::{self, Read};

mod block;
mod huffman;
mod output;

pub use self::block::{DeflateBlockHeader, DeflateBlockType, copy_stored_block, read_block_header};
pub use self::huffman::{HuffmanError, HuffmanTree};
pub use self::output::DeflateOutput;

/// Streaming bit reader for DEFLATE. It does not decompress data.
pub struct DeflateBitReader<R> {
    inner: R,
    bits: u64,
    available: u8,
}

impl<R: Read> DeflateBitReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            bits: 0,
            available: 0,
        }
    }

    /// Reads at most 32 bits as an LSB-first integer.
    ///
    /// On input failure, buffered bits are retained; the underlying reader
    /// may have advanced. A request of zero bits returns zero without I/O.
    pub fn read_bits(&mut self, count: u8) -> io::Result<u32> {
        if count > 32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot read more than 32 bits",
            ));
        }

        while self.available < count {
            let mut byte = [0];
            self.inner.read_exact(&mut byte)?;

            self.bits |= u64::from(byte[0]) << self.available;
            self.available += 8;
        }

        let mask = (1u64 << count) - 1;
        let value = (self.bits & mask) as u32;

        self.bits >>= count;
        self.available -= count;

        Ok(value)
    }

    /// Discards bits up to the next byte boundary.
    pub fn align_to_byte(&mut self) {
        let discard = self.available % 8;

        self.bits >>= discard;
        self.available -= discard;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Cursor;

    use crate::graphics::{
        image::{
            encode_png,
            png_format::{IdatReader, ZlibHeader},
        },
        raster::{Color, Surface},
    };

    #[test]
    fn reads_integer_fields_low_bit_first_across_bytes() {
        let mut bits = DeflateBitReader::new(&[0b1001_0110, 0b1100_0101][..]);

        assert_eq!(bits.read_bits(3).unwrap(), 6);
        assert_eq!(bits.read_bits(4).unwrap(), 2);
        assert_eq!(bits.read_bits(5).unwrap(), 11);
        assert_eq!(bits.read_bits(4).unwrap(), 12);
    }

    #[test]
    fn handles_zero_and_thirty_two_bit_fields() {
        let mut bits = DeflateBitReader::new(&[0x78, 0x56, 0x34, 0x12][..]);

        assert_eq!(bits.read_bits(0).unwrap(), 0);
        assert_eq!(bits.read_bits(32).unwrap(), 0x1234_5678);

        assert_eq!(
            bits.read_bits(1).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn aligns_without_reading_ahead() {
        let mut bits = DeflateBitReader::new(Cursor::new([0xff, 0x42, 0x11]));

        assert_eq!(bits.read_bits(3).unwrap(), 7);
        assert_eq!(bits.inner.position(), 1);

        bits.align_to_byte();

        assert_eq!(bits.read_bits(8).unwrap(), 0x42);
        assert_eq!(bits.inner.position(), 2);
    }

    #[test]
    fn retains_buffered_bits_when_input_is_truncated() {
        let mut bits = DeflateBitReader::new(&[0x96][..]);

        assert_eq!(
            bits.read_bits(16).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );

        assert_eq!(bits.read_bits(4).unwrap(), 6);
        assert_eq!(bits.read_bits(4).unwrap(), 9);
    }

    #[test]
    fn rejects_oversized_fields_without_consuming_input() {
        let mut bits = DeflateBitReader::new(Cursor::new([0x42]));

        assert_eq!(
            bits.read_bits(33).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );

        assert_eq!(bits.inner.position(), 0);
        assert_eq!(bits.read_bits(8).unwrap(), 0x42);
    }

    #[test]
    fn reads_first_deflate_block_from_encoded_png() {
        let surface = Surface::new(1, 1, Color::rgba(255, 0, 0, 255));

        let bytes = encode_png(&surface).unwrap();

        let mut idat = IdatReader::new(&bytes).unwrap();
        ZlibHeader::read_from(&mut idat).unwrap();

        let mut bits = DeflateBitReader::new(idat);

        let _is_final = bits.read_bits(1).unwrap();
        let block_type = bits.read_bits(2).unwrap();

        // Block type 3 is reserved and must not appear in a valid stream.
        assert_ne!(block_type, 3);
    }
}
