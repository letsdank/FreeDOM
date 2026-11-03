//! Dynamic Huffman trees for DEFLATE blocks.
//!
//! RFC 1951, section 3.2.7:
//! <https://www.rfc-editor.org/rfc/rfc1951#section-3.2.7>

use std::io::{self, Read, Write};

use super::{DeflateBitReader, DeflateOutput, HuffmanTree, compressed::decode_huffman_block};

const CODE_LENGTH_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Decodes one dynamic-Huffman block after its header.
///
/// The caller must verify BTYPE and keep the same output across blocks.
/// Errors can leave partially consumed input and partially written output.
pub fn decode_dynamic_huffman_block<R: Read, W: Write>(
    bits: &mut DeflateBitReader<R>,
    output: &mut DeflateOutput<W>,
) -> io::Result<()> {
    let (literals, distances) = read_dynamic_trees(bits)?;

    decode_huffman_block(bits, output, &literals, distances.as_ref())
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn make_tree(lengths: &[u8], allow_single: bool) -> io::Result<HuffmanTree> {
    let tree = HuffmanTree::from_code_lengths(lengths)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;

    let single_one_bit = allow_single
        && lengths.iter().filter(|&&length| length != 0).count() == 1
        && lengths.contains(&1);

    if !tree.is_complete() && !single_one_bit {
        return Err(invalid("incomplete DEFLATE Huffman tree"));
    }

    Ok(tree)
}

fn read_dynamic_trees<R: Read>(
    bits: &mut DeflateBitReader<R>,
) -> io::Result<(HuffmanTree, Option<HuffmanTree>)> {
    let hlit = bits.read_bits(5)? as usize;

    if hlit > 29 {
        return Err(invalid("invalid DEFLATE HLIT"));
    }

    let literal_count = hlit + 257;
    let distance_count = bits.read_bits(5)? as usize + 1;
    let code_count = bits.read_bits(4)? as usize + 4;

    let mut code_lengths = [0u8; 19];

    for &index in &CODE_LENGTH_ORDER[..code_count] {
        code_lengths[index] = bits.read_bits(3)? as u8;
    }

    let code_tree = make_tree(&code_lengths, false)?;

    // Both alphabets from one sequence: repeat codes can cross their boundary.
    let total = literal_count + distance_count;
    let mut lengths = Vec::with_capacity(total);

    while lengths.len() < total {
        let symbol = code_tree.read_symbol(bits)?;

        match symbol {
            0..=15 => {
                lengths.push(symbol as u8);
            }

            16..=18 => {
                let (value, count) = match symbol {
                    16 => {
                        let previous = *lengths
                            .last()
                            .ok_or_else(|| invalid("DEFLATE repeat has no previous length"))?;

                        let count = 3 + bits.read_bits(2)? as usize;

                        (previous, count)
                    }

                    17 => {
                        let count = 3 + bits.read_bits(3)? as usize;
                        (0, count)
                    }

                    18 => {
                        let count = 11 + bits.read_bits(7)? as usize;
                        (0, count)
                    }

                    _ => unreachable!(),
                };

                if count > total - lengths.len() {
                    return Err(invalid("DEFLATE code-length repeat exceeds alphabet"));
                }

                lengths.resize(lengths.len() + count, value);
            }

            _ => {
                return Err(invalid("invalid DEFLATE code-length symbol"));
            }
        }
    }

    let (literal_lengths, distance_lengths) = lengths.split_at(literal_count);

    if literal_lengths[256] == 0 {
        return Err(invalid("DEFLATE tree is missing end-of-block code"));
    }

    let literals = make_tree(literal_lengths, true)?;

    let distances = if distance_lengths.iter().all(|&length| length == 0) {
        None
    } else {
        Some(make_tree(distance_lengths, true)?)
    };

    Ok((literals, distances))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::graphics::image::png_format::{DeflateBlockType, read_block_header};

    fn decode(data: &[u8], limit: usize) -> io::Result<Vec<u8>> {
        let mut bits = DeflateBitReader::new(data);
        let header = read_block_header(&mut bits)?;

        assert_eq!(header.block_type, DeflateBlockType::DynamicHuffman);
        assert!(header.is_final);

        let mut output = DeflateOutput::new(Vec::new(), 32_768, limit)?;

        decode_dynamic_huffman_block(&mut bits, &mut output)?;

        Ok(output.into_inner())
    }

    #[test]
    fn decodes_dynamic_blocks_generated_by_zlib() {
        let bytes = [
            0xed, 0xc6, 0x21, 0x01, 0x00, 0x20, 0x10, 0x00, 0xb1, 0x2c, 0x44, 0x3b, 0x20, 0xc1,
            0xf7, 0x17, 0xc4, 0xc0, 0x6c, 0x6a, 0xed, 0xe9, 0x74, 0xdb, 0xd3, 0xca, 0xdd, 0xdd,
            0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd,
            0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xdd, 0xfd, 0xd3, 0x1f,
        ];

        assert_eq!(
            decode(&bytes, 12_000).unwrap(),
            b"abracadabra!".repeat(1000)
        );

        assert_eq!(
            decode(&bytes, 11_999).unwrap_err().kind(),
            io::ErrorKind::Other
        );

        let bytes = [
            0xed, 0xc9, 0x4b, 0x02, 0x40, 0x20, 0x00, 0x40, 0xc1, 0x7c, 0x23, 0xa5, 0x28, 0x21,
            0xc9, 0xfd, 0x6f, 0xd9, 0x25, 0x2c, 0xdf, 0x6c, 0x47, 0x34, 0x6d, 0xd7, 0x0f, 0xa3,
            0x9c, 0x66, 0xb5, 0x68, 0xb3, 0x5a, 0xb7, 0xed, 0x3e, 0x1c, 0xf1, 0xbc, 0xee, 0xf4,
            0xe4, 0xb7, 0x7c, 0x82, 0xe7, 0x79, 0x9e, 0xe7, 0x79, 0x9e, 0xe7, 0x79, 0xfe, 0xa7,
            0xaf,
        ];

        let expected: Vec<u8> = (0u8..32).cycle().take(3200).collect();

        assert_eq!(decode(&bytes, 3200).unwrap(), expected);
    }

    #[test]
    fn accepts_absent_distance_tree_and_repeat_across_alphabets() {
        // Empty distance alphabet and a single end-of-block literal.
        let no_dist = [0x05, 0xc0, 0x05, 0x09, 0, 0, 0, 0, 0xa0, 0xff, 0xaf, 0x05];

        // A repeat of the previous length crosses the alphabet boundary.
        let cross = [0x05, 0xc1, 0x05, 0x09, 0, 0, 0, 0, 0xa0, 0xff, 0xab, 0x8d];

        // A zero-length repeat encoded with symbol 17.
        let repeat_zero = [
            0x05, 0xc0, 0x21, 0x09, 0, 0, 0, 0, 0xa0, 0xf0, 0x7f, 0xb4, 0x00,
        ];

        assert!(decode(&no_dist, 0).unwrap().is_empty());
        assert!(decode(&cross, 0).unwrap().is_empty());
        assert!(decode(&repeat_zero, 0).unwrap().is_empty());
    }

    #[test]
    fn rejects_invalid_repeats_missing_end_and_hlit() {
        let cases = [
            (
                &[0x05, 0xc0, 0x05, 0x09, 0, 0, 0, 0, 0xa0, 0x00][..],
                "DEFLATE repeat has no previous length",
            ),
            (
                &[0x05, 0xc0, 0x05, 0x09, 0, 0, 0, 0, 0xa0, 0xff, 0xff, 0x01][..],
                "DEFLATE code-length repeat exceeds alphabet",
            ),
            (
                &[0x05, 0xc0, 0x05, 0x09, 0, 0, 0, 0, 0xa0, 0xff, 0xb7, 0x01][..],
                "DEFLATE tree is missing end-of-block code",
            ),
            (&[0xfd][..], "invalid DEFLATE HLIT"),
        ];

        for (bytes, message) in cases {
            let error = decode(bytes, 100).unwrap_err();

            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert_eq!(error.to_string(), message);
        }
    }

    #[test]
    fn validates_tree_completeness_and_no_distance_match() {
        assert!(make_tree(&[1], false).is_err());
        assert!(make_tree(&[1], true).is_ok());
        assert!(make_tree(&[2], true).is_err());

        let mut lengths = [0; 258];
        lengths[256] = 1;
        lengths[257] = 1;

        let literals = make_tree(&lengths, true).unwrap();

        // Code 1 selects length symbol 257, but no distance tree exists.
        let mut bits = DeflateBitReader::new(&[1][..]);
        let mut output = DeflateOutput::new(Vec::new(), 256, 3).unwrap();

        let error = decode_huffman_block(&mut bits, &mut output, &literals, None).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), "DEFLATE match without distance codes");
    }
}
