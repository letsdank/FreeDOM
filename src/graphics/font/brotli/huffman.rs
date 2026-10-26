//! Huffman trees used by Brotli prefix codes.
//!
//! Specification: RFC 7932, sections 3.1 and 3.2.
//! <https://www.rfc-editor.org/rfc/rfc7932.html#section-3>

use super::{BitReader, FontError, error};

const MAX_HUFFMAN_CODE_LENGTH: usize = 15;

#[derive(Clone, Debug)]
enum HuffmanNode {
    Branch([Option<usize>; 2]),
    Symbol(usize),
}

#[derive(Clone, Debug)]
pub struct HuffmanTree {
    nodes: Vec<HuffmanNode>,
    single_symbol: Option<usize>,
}

impl HuffmanTree {
    pub fn single(symbol: usize) -> Self {
        Self {
            nodes: Vec::new(),
            single_symbol: Some(symbol),
        }
    }

    pub fn from_code_lengths(code_lengths: &[u8]) -> Result<Self, FontError> {
        let mut counts = [0_usize; MAX_HUFFMAN_CODE_LENGTH + 1];
        let mut symbol_count = 0_usize;

        for &length in code_lengths {
            if length == 0 {
                continue;
            }

            let length = usize::from(length);

            if length > MAX_HUFFMAN_CODE_LENGTH {
                return Err(error(
                    0,
                    "Brotli Huffman code length exceeds the supported maximum",
                ));
            }

            counts[length] += 1;
            symbol_count += 1;
        }

        if symbol_count == 0 {
            return Err(error(0, "Brotli Huffman tree has no symbols"));
        }

        let mut next_codes = [0_u32; MAX_HUFFMAN_CODE_LENGTH + 1];
        let mut code = 0_u32;

        for length in 1..=MAX_HUFFMAN_CODE_LENGTH {
            code = (code + counts[length - 1] as u32) << 1;
            next_codes[length] = code;
        }

        let mut tree = Self {
            nodes: vec![HuffmanNode::Branch([None, None])],
            single_symbol: None,
        };

        for (symbol, &length) in code_lengths.iter().enumerate() {
            if length == 0 {
                continue;
            }

            let length = usize::from(length);
            let code = next_codes[length];
            next_codes[length] += 1;

            if code >= (1_u32 << length) {
                return Err(error(0, "oversubscribed Brotli Huffman tree"));
            }

            tree.insert(symbol, code, length)?;
        }

        Ok(tree)
    }

    pub fn decode(&self, reader: &mut BitReader<'_>) -> Result<usize, FontError> {
        if let Some(symbol) = self.single_symbol {
            return Ok(symbol);
        }

        let mut node_index = 0_usize;

        loop {
            match &self.nodes[node_index] {
                HuffmanNode::Symbol(symbol) => return Ok(*symbol),

                HuffmanNode::Branch(children) => {
                    let bit = usize::from(reader.read_bit()?);
                    node_index = children[bit].ok_or_else(|| {
                        error(
                            reader.bit_position(),
                            "Brotli prefix code does not exist in the Huffman tree",
                        )
                    })?;
                }
            }
        }
    }

    fn insert(&mut self, symbol: usize, code: u32, length: usize) -> Result<(), FontError> {
        let mut node_index = 0_usize;

        for shift in (0..length).rev() {
            let bit = usize::from(((code >> shift) & 1) != 0);
            let is_last_bit = shift == 0;

            let existing_child = match &self.nodes[node_index] {
                HuffmanNode::Branch(children) => children[bit],
                HuffmanNode::Symbol(_) => {
                    return Err(error(0, "Brotli Huffman tree contains a prefix conflict"));
                }
            };

            if is_last_bit {
                if existing_child.is_some() {
                    return Err(error(
                        0,
                        "Brotli Huffman tree contains duplicate or conflicting codes",
                    ));
                }

                let leaf_index = self.nodes.len();
                self.nodes.push(HuffmanNode::Symbol(symbol));

                let HuffmanNode::Branch(children) = &mut self.nodes[node_index] else {
                    unreachable!();
                };

                children[bit] = Some(leaf_index);
                continue;
            }

            node_index = match existing_child {
                Some(child_index) => child_index,

                None => {
                    let branch_index = self.nodes.len();
                    self.nodes.push(HuffmanNode::Branch([None, None]));

                    let HuffmanNode::Branch(children) = &mut self.nodes[node_index] else {
                        unreachable!();
                    };

                    children[bit] = Some(branch_index);
                    branch_index
                }
            };
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_canonical_huffman_codes() {
        let tree = HuffmanTree::from_code_lengths(&[2, 1, 3, 3]).unwrap();
        let mut reader = BitReader::new(&[0b0001_1001]);

        assert_eq!(tree.decode(&mut reader).unwrap(), 0);
        assert_eq!(tree.decode(&mut reader).unwrap(), 1);
        assert_eq!(tree.decode(&mut reader).unwrap(), 2);
    }

    #[test]
    fn rejects_oversubscribed_huffman_trees() {
        let error = HuffmanTree::from_code_lengths(&[1, 1, 1]).unwrap_err();

        assert_eq!(error.message, "oversubscribed Brotli Huffman tree");
    }
}
