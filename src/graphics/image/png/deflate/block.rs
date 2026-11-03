//! DEFLATE block headers and non-compressed blocks.
//!
//! RFC 1951, sections 3.2.3 and 3.2.4:
//! <https://www.rfc-editor.org/rfc/rfc1951#section-3.2.3>

use std::io::{self, Read, Write};

use super::DeflateBitReader;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeflateBlockType {
    Stored,
    FixedHuffman,
    DynamicHuffman,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeflateBlockHeader {
    pub is_final: bool,
    pub block_type: DeflateBlockType,
}

/// Reads BFINAL and BTYPE. A reserved block type is rejected.
pub fn read_block_header<R: Read>(
    bits: &mut DeflateBitReader<R>,
) -> io::Result<DeflateBlockHeader> {
    let is_final = bits.read_bits(1)? != 0;

    let block_type = match bits.read_bits(2)? {
        0 => DeflateBlockType::Stored,
        1 => DeflateBlockType::FixedHuffman,
        2 => DeflateBlockType::DynamicHuffman,

        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "reserved DEFLATE block type",
            ));
        }
    };

    Ok(DeflateBlockHeader {
        is_final,
        block_type,
    })
}

/// Copies a stored block after its header and returns the number of bytes.
///
/// The caller must check that the block type is `Stored` and maintain a
/// cumulative output limit across blocks. Errors may leave partial output.
pub fn copy_stored_block<R: Read, W: Write>(
    bits: &mut DeflateBitReader<R>,
    output: &mut W,
    max_output_bytes: usize,
) -> io::Result<usize> {
    bits.align_to_byte();

    let length = bits.read_bits(16)? as u16;
    let complement = bits.read_bits(16)? as u16;

    if length != !complement {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid DEFLATE stored block length",
        ));
    }

    let length = usize::from(length);

    if length > max_output_bytes {
        return Err(io::Error::other("DEFLATE output limit exceeded"));
    }

    let mut remaining = length;
    let mut buffer = [0u8; 1024];

    while remaining != 0 {
        let count = remaining.min(buffer.len());

        for byte in &mut buffer[..count] {
            *byte = bits.read_bits(8)? as u8;
        }

        output.write_all(&buffer[..count])?;
        remaining -= count;
    }

    Ok(length)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::graphics::image::png_format::ZlibHeader;

    #[test]
    fn reads_all_header_types_and_rejects_reserved() {
        let cases = [
            (
                0b000,
                DeflateBlockHeader {
                    is_final: false,
                    block_type: DeflateBlockType::Stored,
                },
            ),
            (
                0b011,
                DeflateBlockHeader {
                    is_final: true,
                    block_type: DeflateBlockType::FixedHuffman,
                },
            ),
            (
                0b100,
                DeflateBlockHeader {
                    is_final: false,
                    block_type: DeflateBlockType::DynamicHuffman,
                },
            ),
        ];

        for (byte, expected) in cases {
            let bytes = [byte];
            let mut bits = DeflateBitReader::new(&bytes[..]);

            assert_eq!(read_block_header(&mut bits).unwrap(), expected);
        }

        let mut bits = DeflateBitReader::new(&[0b110][..]);

        assert_eq!(
            read_block_header(&mut bits).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );

        let mut bits = DeflateBitReader::new(&[][..]);

        assert_eq!(
            read_block_header(&mut bits).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn copies_two_stored_blocks_and_preserves_next_header() {
        let input = [
            // Non-final stored block containing "abc".
            0x00, 3, 0, 0xfc, 0xff, b'a', b'b', b'c',
            // Final stored block containing "de".
            0x01, 2, 0, 0xfd, 0xff, b'd', b'e',
        ];

        let mut bits = DeflateBitReader::new(&input[..]);
        let mut output = Vec::new();

        let first = read_block_header(&mut bits).unwrap();

        assert_eq!(
            first,
            DeflateBlockHeader {
                is_final: false,
                block_type: DeflateBlockType::Stored
            }
        );

        assert_eq!(copy_stored_block(&mut bits, &mut output, 5).unwrap(), 3);

        let second = read_block_header(&mut bits).unwrap();

        assert_eq!(
            second,
            DeflateBlockHeader {
                is_final: true,
                block_type: DeflateBlockType::Stored
            }
        );

        assert_eq!(copy_stored_block(&mut bits, &mut output, 2).unwrap(), 2);

        assert_eq!(output, b"abcde");
    }

    #[test]
    fn validates_lengths_truncation_and_output_limit() {
        let cases = [
            (&[1, 1, 0, 0, 0, b'x'][..], 5, io::ErrorKind::InvalidData),
            (
                &[1, 2, 0, 0xfd, 0xff, b'x'][..],
                5,
                io::ErrorKind::UnexpectedEof,
            ),
            (
                &[1, 2, 0, 0xfd, 0xff, b'x', b'y'][..],
                1,
                io::ErrorKind::Other,
            ),
        ];

        for (bytes, limit, expected_kind) in cases {
            let mut bits = DeflateBitReader::new(bytes);
            read_block_header(&mut bits).unwrap();

            let mut output = Vec::new();
            let error = copy_stored_block(&mut bits, &mut output, limit).unwrap_err();

            assert_eq!(error.kind(), expected_kind);
            assert!(output.is_empty());
        }
    }

    #[test]
    fn copies_stored_data_after_a_zlib_header_without_consuming_the_trailer() {
        let bytes = [
            0x78, 0x01, 0x01, 3, 0, 0xfc, 0xff, b'a', b'b', b'c', 0x02, 0x4d, 0x01, 0x27,
        ];

        let mut input = &bytes[..];
        ZlibHeader::read_from(&mut input).unwrap();

        let mut bits = DeflateBitReader::new(input);

        assert_eq!(
            read_block_header(&mut bits).unwrap().block_type,
            DeflateBlockType::Stored
        );

        let mut output = Vec::new();

        assert_eq!(copy_stored_block(&mut bits, &mut output, 3).unwrap(), 3);

        assert_eq!(output, b"abc");

        // Adler-32 is still available for a future checksum verifier.
        assert_eq!(bits.read_bits(8).unwrap(), 0x02);
        assert_eq!(bits.read_bits(8).unwrap(), 0x4d);
        assert_eq!(bits.read_bits(8).unwrap(), 0x01);
        assert_eq!(bits.read_bits(8).unwrap(), 0x27);
    }

    #[test]
    fn copies_empty_and_multi_buffer_blocks() {
        let mut empty_bits = DeflateBitReader::new(&[1, 0, 0, 0xff, 0xff][..]);

        read_block_header(&mut empty_bits).unwrap();

        let mut output = Vec::new();

        assert_eq!(
            copy_stored_block(&mut empty_bits, &mut output, 0).unwrap(),
            0
        );

        // LEN = 1025, NLEN = !1025.
        let mut data = vec![1, 0x01, 0x04, 0xfe, 0xfb];
        data.extend(std::iter::repeat_n(0x55, 1025));

        let mut bits = DeflateBitReader::new(&data[..]);
        read_block_header(&mut bits).unwrap();

        assert_eq!(
            copy_stored_block(&mut bits, &mut output, 1025).unwrap(),
            1025
        );

        assert_eq!(output, vec![0x55; 1025]);
    }
}
