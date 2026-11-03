//! Bounded zlib decoding for PNG compression method 0.
//!
//! RFC 1950, section 2.2:
//! <https://www.rfc-editor.org/rfc/rfc1950#section-2.2>
//!
//! RFC 1951, section 3.2:
//! <https://www.rfc-editor.org/rfc/rfc1951#section-3.2>

use std::io::{self, Read, Write};

use super::ZlibHeader;

use super::super::{
    DeflateBitReader, DeflateBlockType, DeflateOutput, copy_stored_block,
    decode_dynamic_huffman_block, decode_fixed_huffman_block, read_block_header,
    verify_zlib_adler32,
};

#[derive(Debug)]
pub struct ZlibDecodeResult<W> {
    pub output: W,
    pub bytes_written: usize,
    pub checksum: u32,
}

/// Decodes one zlib stream, checking its header and Adler-32 trailer.
///
/// The output limit applies across all blocks. This function does not validate
/// PNG chunk CRCs or pixel data. Data after the zlib trailer is not consumed.
///
/// On error, the input and output may be partially consumed or written. Pass
/// a borrowed writer if its state must remain accessible. Output is not
/// flushed automatically.
pub fn decode_zlib<R: Read, W: Write>(
    mut input: R,
    writer: W,
    max_output_bytes: usize,
) -> io::Result<ZlibDecodeResult<W>> {
    let header = ZlibHeader::read_from(&mut input)?;
    let mut bits = DeflateBitReader::new(input);
    let mut output = DeflateOutput::new(writer, header.window_size, max_output_bytes)?;

    loop {
        let block = read_block_header(&mut bits)?;

        match block.block_type {
            DeflateBlockType::Stored => {
                let remaining = max_output_bytes - output.bytes_written();

                copy_stored_block(&mut bits, &mut output, remaining)?;
            }

            DeflateBlockType::FixedHuffman => {
                decode_fixed_huffman_block(&mut bits, &mut output)?;
            }

            DeflateBlockType::DynamicHuffman => {
                decode_dynamic_huffman_block(&mut bits, &mut output)?;
            }
        }

        if block.is_final {
            break;
        }
    }

    let checksum = output.checksum();

    verify_zlib_adler32(&mut bits, checksum)?;

    let bytes_written = output.bytes_written();

    Ok(ZlibDecodeResult {
        output: output.into_inner(),
        bytes_written,
        checksum,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::graphics::image::png_format::{IdatReader, SIGNATURE};

    #[test]
    fn decodes_fixed_huffman_from_split_idat() {
        // A zlib stream containing "hello".
        let bytes = [
            0x78, 0x9c, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00, 0x06, 0x2c, 0x02, 0x15,
        ];

        let mut png = SIGNATURE.to_vec();

        // Valid 1x1 RGBA IHDR.
        png.extend_from_slice(&[
            0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15,
            0xc4, 0x89,
        ]);

        // Split both the zlib header and the trailer.
        let parts = [&bytes[..1], &bytes[1..11], &bytes[11..]];

        for part in parts {
            png.extend_from_slice(&(part.len() as u32).to_be_bytes());
            png.extend_from_slice(b"IDAT");
            png.extend_from_slice(part);

            // IDAT CRCs are not checked yet.
            png.extend_from_slice(&[0; 4]);
        }

        png.extend_from_slice(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xae, 0x42, 0x60, 0x82]);

        let reader = IdatReader::new(&png).unwrap();
        let result = decode_zlib(reader, Vec::new(), 5).unwrap();

        assert_eq!(result.output, b"hello");
        assert_eq!(result.bytes_written, 5);
        assert_eq!(result.checksum, 0x062c_0215);
    }

    #[test]
    fn decodes_mixed_blocks_with_shared_history() {
        let bytes = [
            // zlib header.
            0x78, 0x01, // Non-final stored block containing "abc".
            0x00, 3, 0, 0xfc, 0xff, b'a', b'b', b'c',
            // Final fixed-Huffman block referring to the previous block.
            0x03, 0x02, 0x00, // Adler-32 of "abcccc".
            0x08, 0x14, 0x02, 0x50, // Data after the zlib stream.
            0xee,
        ];

        let mut input = &bytes[..];
        let result = decode_zlib(&mut input, Vec::new(), 6).unwrap();

        assert_eq!(result.output, b"abcccc");
        assert_eq!(result.bytes_written, 6);
        assert_eq!(result.checksum, 0x0814_0250);
        assert_eq!(input, &[0xee]);
    }

    #[test]
    fn decodes_dynamic_huffman_and_enforces_global_limit() {
        let bytes = [
            0x78, 0x9c, 0xed, 0xc6, 0x21, 0x01, 0x00, 0x20, 0x10, 0x00, 0xb1, 0x2c, 0x44, 0x3b,
            0x20, 0xc1, 0xf7, 0x17, 0xc4, 0xc0, 0x6c, 0x6a, 0xed, 0xe9, 0x74, 0xdb, 0xd3, 0xca,
            0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd,
            0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xfd, 0xd3, 0x1f, 0xe6, 0x8d, 0x6a,
            0x08,
        ];

        let result = decode_zlib(&bytes[..], Vec::new(), 12_000).unwrap();

        assert_eq!(result.output, b"abracadabra!".repeat(1000));
        assert_eq!(result.checksum, 0xe68d_6a08);

        assert_eq!(
            decode_zlib(&bytes[..], Vec::new(), 11_999)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Other
        );
    }

    #[test]
    fn rejects_bad_header_truncation_checksum_and_reserved_block() {
        let cases: &[(&[u8], io::ErrorKind)] = &[
            (&[0x78, 0x20], io::ErrorKind::InvalidData),
            (&[0x78], io::ErrorKind::UnexpectedEof),
            (&[0x78, 0x01, 0x07], io::ErrorKind::InvalidData),
            (&[0x78, 0x9c, 0xcb], io::ErrorKind::UnexpectedEof),
            (
                &[
                    0x78, 0x9c, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00, 0x06, 0x2c, 0x02, 0x14,
                ],
                io::ErrorKind::InvalidData,
            ),
            (
                &[
                    0x78, 0x9c, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00, 0x06, 0x2c,
                ],
                io::ErrorKind::UnexpectedEof,
            ),
        ];

        for &(data, expected_kind) in cases {
            let error = decode_zlib(data, Vec::new(), 100).unwrap_err();

            assert_eq!(error.kind(), expected_kind);
        }
    }

    #[test]
    fn limits_stored_output_across_blocks_before_writing_the_next() {
        let bytes = [
            0x78, 0x01, 0x00, 3, 0, 0xfc, 0xff, b'a', b'b', b'c', 0x01, 2, 0, 0xfd, 0xff, b'd',
            b'e', 0x05, 0xc8, 0x01, 0xf0,
        ];

        let mut output = Vec::new();

        let error = decode_zlib(&bytes[..], &mut output, 4).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(output, b"abc");
    }

    #[test]
    fn matches_independent_zlib_decoder_for_encoded_pngs() {
        use crate::graphics::{
            image::encode_png,
            raster::{Color, Surface},
        };

        for size in [1usize, 2, 17, 64] {
            let mut surface = Surface::new(size, size, Color::TRANSPARENT);

            for y in 0..size {
                for x in 0..size {
                    surface.set_pixel(
                        x,
                        y,
                        Color::rgba((x * 3) as u8, (y * 5) as u8, (x ^ y) as u8, 255),
                    );
                }
            }

            let png = encode_png(&surface).unwrap();

            // flate2 is used only as an independent test reference.
            let reader = IdatReader::new(&png).unwrap();
            let mut reference = Vec::new();

            flate2::read::ZlibDecoder::new(reader)
                .read_to_end(&mut reference)
                .unwrap();

            let reader = IdatReader::new(&png).unwrap();
            let result = decode_zlib(reader, Vec::new(), 100_000).unwrap();

            assert_eq!(result.output, reference, "image size: {size}");
        }
    }
}
