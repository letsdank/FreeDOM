//! Canonical DEFLATE Huffman code construction and symbol reading.
//!
//! RFC 1951, sections 3.1.1, 3.2.2 and 3.2.7:
//! <https://www.rfc-editor.org/rfc/rfc1951#section-3.2.2>

use std::{
    error::Error,
    fmt,
    io::{self, Read},
};

use super::DeflateBitReader;

const MAX_BITS: usize = 15;
const MAX_SYMBOLS: usize = 288;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HuffmanError {
    TooManySymbols,
    InvalidCodeLength(u8),
    EmptyAlphabet,
    Oversubscribed,
}

impl fmt::Display for HuffmanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid DEFLATE Huffman lengths: {self:?}")
    }
}

impl Error for HuffmanError {}

#[derive(Default)]
struct Node {
    children: [Option<usize>; 2],
    symbol: Option<u16>,
}

/// A nonempty canonical tree.
///
/// Incomplete trees can be built here. Callers must validate whether they are
/// permitted in their specific DEFLATE context.
pub struct HuffmanTree {
    nodes: Vec<Node>,
    complete: bool,
}

impl HuffmanTree {
    /// Builds a canonical tree from symbol code lengths.
    ///
    /// Zero lengths indicate unused symbols. An entirely empty alphabet is
    /// returned as an error; an absent distance tree must be handled by its
    /// caller.
    pub fn from_code_lengths(lengths: &[u8]) -> Result<Self, HuffmanError> {
        if lengths.len() > MAX_SYMBOLS {
            return Err(HuffmanError::TooManySymbols);
        }

        let mut counts = [0u32; MAX_BITS + 1];

        for &length in lengths {
            if usize::from(length) > MAX_BITS {
                return Err(HuffmanError::InvalidCodeLength(length));
            }

            if length != 0 {
                counts[usize::from(length)] += 1;
            }
        }

        if counts.iter().all(|&count| count == 0) {
            return Err(HuffmanError::EmptyAlphabet);
        }

        // Count unused code slots at each depth.
        let mut slots = 1i32;

        for &count in &counts[1..] {
            slots = slots * 2 - count as i32;

            if slots < 0 {
                return Err(HuffmanError::Oversubscribed);
            }
        }

        // Find the first canonical code for each length.
        let mut next_code = [0u32; MAX_BITS + 1];
        let mut code = 0u32;

        for length in 1..=MAX_BITS {
            code = (code + counts[length - 1]) << 1;
            next_code[length] = code;
        }

        let mut nodes = vec![Node::default()];

        for (symbol, &length) in lengths.iter().enumerate() {
            if length == 0 {
                continue;
            }

            let code = next_code[usize::from(length)];
            next_code[usize::from(length)] += 1;

            let mut current = 0;

            // Huffman codes are traversed from their most significant bit.
            for shift in (0..length).rev() {
                let bit = ((code >> shift) & 1) as usize;

                let next = match nodes[current].children[bit] {
                    Some(next) => next,

                    None => {
                        let next = nodes.len();

                        nodes.push(Node::default());
                        nodes[current].children[bit] = Some(next);

                        next
                    }
                };

                current = next;
            }

            nodes[current].symbol = Some(symbol as u16);
        }

        Ok(Self {
            nodes,
            complete: slots == 0,
        })
    }

    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Reads one symbol. Invalid paths and truncated input are errors.
    ///
    /// A failure may consume part of the code from the input.
    pub fn read_symbol<R: Read>(&self, bits: &mut DeflateBitReader<R>) -> io::Result<u16> {
        let mut current = 0;

        for _ in 0..MAX_BITS {
            let bit = bits.read_bits(1)? as usize;

            current = self.nodes[current].children[bit].ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid DEFLATE Huffman code")
            })?;

            if let Some(symbol) = self.nodes[current].symbol {
                return Ok(symbol);
            }
        }

        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "DEFLATE Huffman code is too long",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::graphics::image::png_format::{DeflateBlockType, read_block_header};

    #[test]
    fn decodes_the_rfc_canonical_example() {
        // A=10, B=0, C=110, D=111; input encodes A, B, C, D.
        let tree = HuffmanTree::from_code_lengths(&[2, 1, 3, 3]).unwrap();

        assert!(tree.is_complete());

        let mut bits = DeflateBitReader::new(&[0xd9, 0x01][..]);

        for symbol in 0..4 {
            assert_eq!(tree.read_symbol(&mut bits).unwrap(), symbol);
        }
    }

    #[test]
    fn accepts_incomplete_tree_but_rejects_missing_branch() {
        let tree = HuffmanTree::from_code_lengths(&[0, 1]).unwrap();

        assert!(!tree.is_complete());

        let mut zero = DeflateBitReader::new(&[0][..]);
        assert_eq!(tree.read_symbol(&mut zero).unwrap(), 1);

        let mut one = DeflateBitReader::new(&[1][..]);

        assert_eq!(
            tree.read_symbol(&mut one).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );

        let mut empty = DeflateBitReader::new(&[][..]);

        assert_eq!(
            tree.read_symbol(&mut empty).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn rejects_invalid_length_sets() {
        assert!(matches!(
            HuffmanTree::from_code_lengths(&[0, 0]),
            Err(HuffmanError::EmptyAlphabet)
        ));

        assert!(matches!(
            HuffmanTree::from_code_lengths(&[16]),
            Err(HuffmanError::InvalidCodeLength(16))
        ));

        assert!(matches!(
            HuffmanTree::from_code_lengths(&[1, 1, 1]),
            Err(HuffmanError::Oversubscribed)
        ));

        assert!(matches!(
            HuffmanTree::from_code_lengths(&[0; 289]),
            Err(HuffmanError::TooManySymbols)
        ));
    }

    #[test]
    fn decodes_fixed_huffman_literals_from_a_real_deflate_stream() {
        // Raw DEFLATE stream containing the bytes "hello".
        let data = [0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00];
        let mut bits = DeflateBitReader::new(&data[..]);

        let header = read_block_header(&mut bits).unwrap();

        assert!(header.is_final);
        assert_eq!(header.block_type, DeflateBlockType::FixedHuffman);

        let mut lengths = [0u8; 288];

        lengths[..144].fill(8);
        lengths[144..256].fill(9);
        lengths[256..280].fill(7);
        lengths[280..].fill(8);

        let tree = HuffmanTree::from_code_lengths(&lengths).unwrap();

        assert!(tree.is_complete());

        for &byte in b"hello" {
            assert_eq!(tree.read_symbol(&mut bits).unwrap(), u16::from(byte));
        }

        assert_eq!(tree.read_symbol(&mut bits).unwrap(), 256);
    }

    #[test]
    fn fixed_tree_matches_every_code_in_the_rfc_table() {
        let mut lengths = [0u8; 288];

        lengths[..144].fill(8);
        lengths[144..256].fill(9);
        lengths[256..280].fill(7);
        lengths[280..].fill(8);

        let tree = HuffmanTree::from_code_lengths(&lengths).unwrap();

        for symbol in 0u16..288 {
            let (code, width) = match symbol {
                0..=143 => (0x30 + symbol, 8),
                144..=255 => (0x190 + symbol - 144, 9),
                256..=279 => (symbol - 256, 7),
                _ => (0xc0 + symbol - 280, 8),
            };

            let bytes = (code.reverse_bits() >> (16 - width)).to_le_bytes();
            let mut reader = DeflateBitReader::new(&bytes[..]);

            assert_eq!(tree.read_symbol(&mut reader).unwrap(), symbol);
        }
    }
}
