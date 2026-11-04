//! Decoding DEFLATE blocks with fixed Huffman codes.
//!
//! RFC 1951, sections 3.2.5 and 3.2.6:
//! <https://www.rfc-editor.org/rfc/rfc1951#section-3.2.5>

use std::io::{self, Read, Write};

use super::tables::{DISTANCE_BASE, DISTANCE_EXTRA, LENGTH_BASE, LENGTH_EXTRA};
use super::{DeflateBitReader, DeflateOutput, HuffmanTree};

/// Decodes the body of one fixed-Huffman block after its header.
///
/// The caller must verify BTYPE and retain the same output across blocks.
/// On error, the input and output may already have been partially consumed.
pub fn decode_fixed_huffman_block<R: Read, W: Write>(
    bits: &mut DeflateBitReader<R>,
    output: &mut DeflateOutput<W>,
) -> io::Result<()> {
    let mut lengths = [0u8; 288];

    lengths[..144].fill(8);
    lengths[144..256].fill(9);
    lengths[256..280].fill(7);
    lengths[280..].fill(8);

    let literals = HuffmanTree::from_code_lengths(&lengths).map_err(io::Error::other)?;
    let distances = HuffmanTree::from_code_lengths(&[5; 32]).map_err(io::Error::other)?;

    decode_huffman_block(bits, output, &literals, Some(&distances))
}

// Dynamic-block parsing must validate its trees before calling this function.
pub(super) fn decode_huffman_block<R: Read, W: Write>(
    bits: &mut DeflateBitReader<R>,
    output: &mut DeflateOutput<W>,
    literals: &HuffmanTree,
    distances: Option<&HuffmanTree>,
) -> io::Result<()> {
    loop {
        match literals.read_symbol(bits)? {
            symbol @ 0..=255 => {
                output.write_all(&[symbol as u8])?;
            }

            256 => return Ok(()),

            symbol @ 257..=285 => {
                let index = usize::from(symbol - 257);

                let length = LENGTH_BASE[index] + bits.read_bits(LENGTH_EXTRA[index])? as usize;

                let distances = distances.ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "DEFLATE match without distance codes",
                    )
                })?;

                let index = usize::from(distances.read_symbol(bits)?);

                if index >= DISTANCE_BASE.len() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "reserved DEFLATE distance code",
                    ));
                }

                let distance =
                    DISTANCE_BASE[index] + bits.read_bits(DISTANCE_EXTRA[index])? as usize;

                output.copy_match(distance, length)?;
            }

            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "reserved DEFLATE literal/length code",
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::graphics::image::png_format::{DeflateBlockType, read_block_header};

    fn decode(data: &[u8], limit: usize) -> io::Result<Vec<u8>> {
        let mut bits = DeflateBitReader::new(data);
        let header = read_block_header(&mut bits)?;

        assert_eq!(header.block_type, DeflateBlockType::FixedHuffman);
        assert!(header.is_final);

        let mut output = DeflateOutput::new(Vec::new(), 32_768, limit)?;

        decode_fixed_huffman_block(&mut bits, &mut output)?;

        Ok(output.into_inner())
    }

    #[test]
    fn decodes_literals_and_overlapping_matches() {
        assert_eq!(
            decode(&[0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00], 5).unwrap(),
            b"hello"
        );

        assert_eq!(
            decode(&[0x4b, 0x4c, 0x4a, 0x4e, 0x84, 0x21, 0x00], 12).unwrap(),
            b"abcabcabcabc"
        );

        assert_eq!(
            decode(&[0x4b, 0x4c, 0x1c, 0x05, 0xc4, 0x02, 0x00], 300).unwrap(),
            vec![b'a'; 300]
        );
    }

    #[test]
    fn rejects_reserved_codes_and_invalid_back_reference() {
        let cases = [
            (&[0x1b, 0x03][..], "reserved DEFLATE literal/length code"),
            (&[0x03, 0x3e][..], "reserved DEFLATE distance code"),
            (&[0x03, 0x02][..], "invalid DEFLATE backward distance"),
        ];

        for (bytes, expected) in cases {
            let error = decode(bytes, 100).unwrap_err();

            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert_eq!(error.to_string(), expected);
        }
    }

    #[test]
    fn rejects_truncation_and_overlong_output() {
        assert_eq!(
            decode(&[0xcb], 10).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );

        assert_eq!(
            decode(&[0x4b, 0x4c, 0x1c, 0x05, 0xc4, 0x02, 0x00], 299)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Other
        );
    }

    #[test]
    fn can_reference_output_from_an_earlier_block() {
        let mut bits = DeflateBitReader::new(&[0x03, 0x02, 0x00][..]);

        assert!(read_block_header(&mut bits).unwrap().is_final);

        let mut output = DeflateOutput::new(Vec::new(), 256, 4).unwrap();

        // Simulate a byte emitted by an earlier block.
        output.write_all(b"a").unwrap();

        decode_fixed_huffman_block(&mut bits, &mut output).unwrap();

        assert_eq!(output.into_inner(), b"aaaa");
    }

    #[test]
    fn distance_code_table_matches_rfc_1951() {
        assert_eq!(
            DISTANCE_BASE,
            [
                1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769,
                1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577
            ]
        );

        assert_eq!(
            DISTANCE_EXTRA,
            [
                0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11,
                12, 12, 13, 13
            ]
        );
    }

    #[test]
    fn decodes_distance_code_17_at_both_range_boundaries() {
        use std::io::Read;

        fn write_bits(output: &mut Vec<u8>, position: &mut usize, value: usize, count: usize) {
            for i in 0..count {
                if *position % 8 == 0 {
                    output.push(0);
                }

                let last = output.len() - 1;
                let bit = ((value >> i) & 1) as u8;

                output[last] |= bit << (*position % 8);
                *position += 1;
            }
        }

        fn write_huffman(output: &mut Vec<u8>, position: &mut usize, code: usize, count: usize) {
            for i in (0..count).rev() {
                write_bits(output, position, (code >> i) & 1, 1);
            }
        }

        for distance in [385usize, 512] {
            let history: Vec<u8> = (0..512).map(|value| value as u8).collect();

            let mut encoded = Vec::new();
            let mut position = 0;

            // Final block using fixed Huffman codes.
            write_bits(&mut encoded, &mut position, 1, 1);
            write_bits(&mut encoded, &mut position, 1, 2);

            for &literal in &history {
                if literal < 144 {
                    write_huffman(&mut encoded, &mut position, 0x30 + usize::from(literal), 8);
                } else {
                    write_huffman(
                        &mut encoded,
                        &mut position,
                        0x190 + usize::from(literal) - 144,
                        9,
                    );
                }
            }

            // Length code 257 represents a three-byte match.
            write_huffman(&mut encoded, &mut position, 1, 7);

            // Distance code 17 uses seven additional bits.
            write_huffman(&mut encoded, &mut position, 17, 5);
            write_bits(&mut encoded, &mut position, distance - 385, 7);

            // End-of-block code 256.
            write_huffman(&mut encoded, &mut position, 0, 7);

            let mut expected = history.clone();
            let start = history.len() - distance;

            expected.extend_from_slice(&history[start..start + 3]);

            // Check the constructed stream with an independent decoder.
            let mut reference = Vec::new();

            flate2::read::DeflateDecoder::new(encoded.as_slice())
                .read_to_end(&mut reference)
                .unwrap();

            assert_eq!(
                reference, expected,
                "independent decoder, distance={distance}"
            );

            let actual = decode(&encoded, expected.len()).unwrap();

            assert_eq!(actual, expected, "FreeDOM decoder, distance={distance}");
        }
    }
}
