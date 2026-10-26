use super::sfnt::FontError;

const MAX_HUFFMAN_CODE_LENGTH: usize = 15;

#[derive(Clone, Debug)]
pub struct BitReader<'a> {
    data: &'a [u8],
    bit_position: usize,
}

impl<'a> BitReader<'a> {
    pub const fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            bit_position: 0,
        }
    }

    pub const fn bit_position(&self) -> usize {
        self.bit_position
    }

    pub fn bits_remaining(&self) -> usize {
        self.data
            .len()
            .saturating_mul(8)
            .saturating_sub(self.bit_position)
    }

    pub fn read_bit(&mut self) -> Result<bool, FontError> {
        Ok(self.read_bits(1)? != 0)
    }

    pub fn read_bits(&mut self, bit_count: usize) -> Result<u32, FontError> {
        if bit_count > 32 {
            return Err(error(
                self.bit_position,
                "Brotli bit reader cannot read more than 32 bits",
            ));
        }

        let end = self
            .bit_position
            .checked_add(bit_count)
            .ok_or_else(|| error(self.bit_position, "Brotli bit position overflows"))?;

        let total_bits = self
            .data
            .len()
            .checked_mul(8)
            .ok_or_else(|| error(self.bit_position, "Brotli input is too large"))?;

        if end > total_bits {
            return Err(error(self.bit_position, "truncated Brotli bit stream"));
        }

        let mut value = 0_u32;

        for bit_index in 0..bit_count {
            let source_bit = self.bit_position + bit_index;
            let byte = self.data[source_bit / 8];
            let bit = (byte >> (source_bit % 8)) & 1;

            value |= u32::from(bit) << bit_index;
        }

        self.bit_position = end;

        Ok(value)
    }

    pub fn align_to_byte(&mut self) -> Result<(), FontError> {
        let aligned = self
            .bit_position
            .checked_add(7)
            .ok_or_else(|| error(self.bit_position, "Brotli bit position overflows"))?
            & !7;

        let total_bits = self
            .data
            .len()
            .checked_mul(8)
            .ok_or_else(|| error(self.bit_position, "Brotli input is too large"))?;

        if aligned > total_bits {
            return Err(error(self.bit_position, "truncated Brotli bit stream"));
        }

        self.bit_position = aligned;

        Ok(())
    }

    pub fn read_aligned_bytes(&mut self, length: usize) -> Result<&'a [u8], FontError> {
        self.align_to_byte()?;

        let start = self.bit_position / 8;
        let end = start
            .checked_add(length)
            .ok_or_else(|| error(start, "Brotli byte range overflows"))?;

        let bytes = self
            .data
            .get(start..end)
            .ok_or_else(|| error(start, "truncated Brotli byte stream"))?;

        self.bit_position = end
            .checked_mul(8)
            .ok_or_else(|| error(end, "Brotli bit position overflows"))?;

        Ok(bytes)
    }
}

#[derive(Clone, Debug)]
enum HuffmanNode {
    Branch([Option<usize>; 2]),
    Symbol(usize),
}

#[derive(Clone, Debug)]
pub struct HuffmanTree {
    nodes: Vec<HuffmanNode>,
}

impl HuffmanTree {
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

fn error(offset: usize, message: impl Into<String>) -> FontError {
    FontError {
        offset,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_brotli_integer_fields_from_low_bits_first() {
        let mut reader = BitReader::new(&[0b1001_0110, 0b1100_0101]);

        assert_eq!(reader.read_bits(3).unwrap(), 6);
        assert_eq!(reader.read_bits(4).unwrap(), 2);
        assert_eq!(reader.bit_position(), 7);
        assert!(reader.read_bit().unwrap());
        assert_eq!(reader.read_bits(4).unwrap(), 5);
    }

    #[test]
    fn aligns_before_reading_byte_data() {
        let mut reader = BitReader::new(&[0b0000_0111, 0xaa, 0xbb]);

        assert_eq!(reader.read_bits(3).unwrap(), 7);
        assert_eq!(reader.read_aligned_bytes(2).unwrap(), &[0xaa, 0xbb]);
    }

    #[test]
    fn rejects_reads_past_the_end_of_the_input() {
        let mut reader = BitReader::new(&[0]);

        let error = reader.read_bits(9).unwrap_err();

        assert_eq!(error.message, "truncated Brotli bit stream");
    }

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
