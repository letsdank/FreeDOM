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

pub fn decode_uncompressed_meta_blocks(
    data: &[u8],
    maximum_output_size: usize,
) -> Result<Vec<u8>, FontError> {
    let mut reader = BitReader::new(data);
    let _window_bits = read_window_bits(&mut reader)?;
    let mut output = Vec::new();

    loop {
        let is_last = reader.read_bit()?;

        if is_last && reader.read_bit()? {
            consume_zero_padding(&mut reader)?;

            if reader.bits_remaining() != 0 {
                return Err(error(
                    reader.bit_position(),
                    "Brotli stream has bytes after its final empty meta-block",
                ));
            }

            return Ok(output);
        }

        let nibble_count = read_meta_block_nibble_count(&mut reader)?;

        if nibble_count == 0 {
            skip_empty_meta_block(&mut reader)?;
            continue;
        }

        let output_length = read_meta_block_length(&mut reader, nibble_count)?;

        if is_last {
            return Err(error(
                reader.bit_position(),
                "compressed Brotli meta-blocks are not supported yet",
            ));
        }

        if !reader.read_bit()? {
            return Err(error(
                reader.bit_position(),
                "compressed Brotli meta-blocks are not supported yet",
            ));
        }

        consume_zero_padding(&mut reader)?;

        let new_length = output
            .len()
            .checked_add(output_length)
            .ok_or_else(|| error(reader.bit_position(), "Brotli output size overflows"))?;

        if new_length > maximum_output_size {
            return Err(error(
                reader.bit_position(),
                "Brotli output exceeds the configured size limit",
            ));
        }

        output
            .try_reserve(output_length)
            .map_err(|_| error(reader.bit_position(), "cannot allocate Brotli output"))?;

        output.extend_from_slice(reader.read_aligned_bytes(output_length)?);
    }
}

fn read_window_bits(reader: &mut BitReader<'_>) -> Result<u8, FontError> {
    if !reader.read_bit()? {
        return Ok(16);
    }

    let short_code = reader.read_bits(3)? as u8;

    if short_code != 0 {
        return Ok(17 + short_code);
    }

    match reader.read_bits(3)? as u8 {
        0 => Ok(17),
        1 => Err(error(
            reader.bit_position(),
            "Brotli stream uses the reserved WBITS value",
        )),
        value => Ok(8 + value),
    }
}

fn read_meta_block_nibble_count(reader: &mut BitReader<'_>) -> Result<usize, FontError> {
    match reader.read_bits(2)? {
        0 => Ok(4),
        1 => Ok(5),
        2 => Ok(6),
        3 => Ok(0),
        _ => unreachable!(),
    }
}

fn read_meta_block_length(
    reader: &mut BitReader<'_>,
    nibble_count: usize,
) -> Result<usize, FontError> {
    let length_minus_one = reader.read_bits(nibble_count * 4)?;

    if nibble_count > 4 {
        let highest_nibble = (length_minus_one >> ((nibble_count - 1) * 4)) & 0x0f;

        if highest_nibble == 0 {
            return Err(error(
                reader.bit_position(),
                "Brotli meta-block length has a redundant leading zero nibble",
            ));
        }
    }

    usize::try_from(length_minus_one)
        .ok()
        .and_then(|length| length.checked_add(1))
        .ok_or_else(|| error(reader.bit_position(), "Brotli meta-block length overflows"))
}

fn skip_empty_meta_block(reader: &mut BitReader<'_>) -> Result<(), FontError> {
    if reader.read_bit()? {
        return Err(error(
            reader.bit_position(),
            "Brotli empty meta-block has its reserved bit set",
        ));
    }

    let skip_byte_count = reader.read_bits(2)? as usize;

    let metadata_length = if skip_byte_count == 0 {
        0
    } else {
        let length_minus_one = reader.read_bits(skip_byte_count * 8)?;

        if skip_byte_count > 1 {
            let most_significant_byte = (length_minus_one >> ((skip_byte_count - 1) * 8)) & 0xff;

            if most_significant_byte == 0 {
                return Err(error(
                    reader.bit_position(),
                    "Brotli metadata length has a redundant leading zero byte",
                ));
            }
        }

        usize::try_from(length_minus_one)
            .ok()
            .and_then(|length| length.checked_add(1))
            .ok_or_else(|| error(reader.bit_position(), "Brotli metadata length overflows"))?
    };

    consume_zero_padding(reader)?;
    reader.read_aligned_bytes(metadata_length)?;

    Ok(())
}

fn consume_zero_padding(reader: &mut BitReader<'_>) -> Result<(), FontError> {
    let padding_bits = (8 - reader.bit_position() % 8) % 8;

    if reader.read_bits(padding_bits)? != 0 {
        return Err(error(
            reader.bit_position(),
            "Brotli byte-alignment padding must be zero",
        ));
    }

    Ok(())
}

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

pub fn read_simple_prefix_code(
    reader: &mut BitReader<'_>,
    alphabet_size: usize,
) -> Result<HuffmanTree, FontError> {
    if alphabet_size == 0 {
        return Err(error(
            reader.bit_position(),
            "Brotli prefix-code alphabet must not be empty",
        ));
    }

    if reader.read_bits(2)? != 1 {
        return Err(error(
            reader.bit_position(),
            "expected a Brotli simple prefix code",
        ));
    }

    let symbol_count = reader.read_bits(2)? as usize + 1;
    let alphabet_bits = alphabet_bit_width(alphabet_size)?;
    let mut symbols = Vec::with_capacity(symbol_count);

    for _ in 0..symbol_count {
        let symbol = reader.read_bits(alphabet_bits)? as usize;

        if symbol >= alphabet_size {
            return Err(error(
                reader.bit_position(),
                "Brotli prefix-code symbol exceeds its alphabet",
            ));
        }

        if symbols.contains(&symbol) {
            return Err(error(
                reader.bit_position(),
                "Brotli simple prefix code contains a duplicate symbol",
            ));
        }

        symbols.push(symbol);
    }

    if symbol_count == 1 {
        return Ok(HuffmanTree::single(symbols[0]));
    }

    let lengths: &[u8] = match symbol_count {
        2 => &[1, 1],
        3 => &[1, 2, 2],
        4 if reader.read_bit()? => &[1, 2, 3, 3],
        4 => &[2, 2, 2, 2],
        _ => unreachable!(),
    };

    let mut code_lengths = vec![0_u8; alphabet_size];

    for (&symbol, &length) in symbols.iter().zip(lengths) {
        code_lengths[symbol] = length;
    }

    HuffmanTree::from_code_lengths(&code_lengths)
}

fn alphabet_bit_width(alphabet_size: usize) -> Result<usize, FontError> {
    if alphabet_size == 0 {
        return Err(error(0, "Brotli prefix-code alphabet must not be empty"));
    }

    Ok((usize::BITS - (alphabet_size - 1).leading_zeros()) as usize)
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

    #[test]
    fn decodes_an_uncompressed_meta_block() {
        let stream = [0x20, 0x00, 0x10, b'a', b'b', b'c', 0x03];

        assert_eq!(decode_uncompressed_meta_blocks(&stream, 3).unwrap(), b"abc");
    }

    #[test]
    fn skips_metadata_in_an_empty_meta_block() {
        let stream = [0x2c, 0x00, 0x99, 0x03];

        assert_eq!(
            decode_uncompressed_meta_blocks(&stream, 0).unwrap(),
            Vec::<u8>::new()
        );
    }

    #[test]
    fn rejects_non_zero_padding_after_a_final_empty_meta_block() {
        let error = decode_uncompressed_meta_blocks(&[0x0e], 0).unwrap_err();

        assert_eq!(error.message, "Brotli byte-alignment padding must be zero");
    }

    #[test]
    fn reports_compressed_meta_blocks_as_unsupported() {
        let error = decode_uncompressed_meta_blocks(&[0x02, 0x00, 0x00], 1).unwrap_err();

        assert_eq!(
            error.message,
            "compressed Brotli meta-blocks are not supported yet"
        );
    }

    #[test]
    fn decodes_a_three_symbol_simple_prefix_code() {
        let mut data = Vec::new();
        let mut bit_position = 0;

        append_bits(&mut data, &mut bit_position, 1, 2);
        append_bits(&mut data, &mut bit_position, 2, 2);
        append_bits(&mut data, &mut bit_position, 5, 8);
        append_bits(&mut data, &mut bit_position, 2, 8);
        append_bits(&mut data, &mut bit_position, 9, 8);

        append_bits(&mut data, &mut bit_position, 0, 1);
        append_bits(&mut data, &mut bit_position, 1, 2);
        append_bits(&mut data, &mut bit_position, 3, 2);

        let mut reader = BitReader::new(&data);
        let tree = read_simple_prefix_code(&mut reader, 256).unwrap();

        assert_eq!(tree.decode(&mut reader).unwrap(), 5);
        assert_eq!(tree.decode(&mut reader).unwrap(), 2);
        assert_eq!(tree.decode(&mut reader).unwrap(), 9);
    }

    #[test]
    fn decodes_a_single_symbol_simple_prefix_code_without_consuming_bits() {
        let mut data = Vec::new();
        let mut bit_position = 0;

        append_bits(&mut data, &mut bit_position, 1, 2);
        append_bits(&mut data, &mut bit_position, 0, 2);
        append_bits(&mut data, &mut bit_position, 42, 8);

        let mut reader = BitReader::new(&data);
        let tree = read_simple_prefix_code(&mut reader, 256).unwrap();
        let before_decode = reader.bit_position();

        assert_eq!(tree.decode(&mut reader).unwrap(), 42);
        assert_eq!(reader.bit_position(), before_decode);
    }

    #[test]
    fn rejects_duplicate_symbols_in_a_simple_prefix_code() {
        let mut data = Vec::new();
        let mut bit_position = 0;

        append_bits(&mut data, &mut bit_position, 1, 2);
        append_bits(&mut data, &mut bit_position, 1, 2);
        append_bits(&mut data, &mut bit_position, 7, 8);
        append_bits(&mut data, &mut bit_position, 7, 8);

        let mut reader = BitReader::new(&data);
        let error = read_simple_prefix_code(&mut reader, 256).unwrap_err();

        assert_eq!(
            error.message,
            "Brotli simple prefix code contains a duplicate symbol"
        );
    }

    fn append_bits(data: &mut Vec<u8>, bit_position: &mut usize, value: u32, bit_count: usize) {
        for bit_index in 0..bit_count {
            if *bit_position / 8 == data.len() {
                data.push(0);
            }

            let bit = (value >> bit_index) & 1;

            data[*bit_position / 8] |= (bit as u8) << (*bit_position % 8);
            *bit_position += 1;
        }
    }
}
