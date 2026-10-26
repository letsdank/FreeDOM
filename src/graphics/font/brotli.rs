//! Brotli bitstream parsing for WOFF 2.0.
//!
//! The decoder currently handles uncompressed meta-blocks. Parsing support
//! for compressed meta-block headers, prefix codes, contexts, and commands is
//! being implemented separately and is not yet connected to the decoder.
//!
//! Specification: <https://www.rfc-editor.org/rfc/rfc7932.html>
mod bit_reader;
mod command;
mod context;

pub use self::bit_reader::BitReader;

use self::command::{InsertCopyCommand, read_insert_copy_command};
use self::context::{LiteralContextMode, distance_context_id, literal_context_id};

use super::sfnt::FontError;

const MAX_HUFFMAN_CODE_LENGTH: usize = 15;
const CODE_LENGTH_ALPHABET_WEIGHT: u32 = 32;
const PREFIX_CODE_WEIGHT: u32 = 32_768;

// RFC 7932, section 3.5: order of code-length code lengths.
const CODE_LENGTH_ALPHABET_ORDER: [usize; 18] =
    [1, 2, 3, 4, 0, 5, 17, 6, 16, 7, 8, 9, 10, 11, 12, 13, 14, 15];

const IMPLICIT_BLOCK_LENGTH: usize = 1 << 24;

// RFC 7932, section 6: block count codes.
const BLOCK_LENGTH_EXTRA_BITS: [usize; 26] = [
    2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 6, 6, 7, 8, 9, 10, 11, 12, 13, 24,
];

const BLOCK_LENGTH_OFFSETS: [usize; 26] = [
    1, 5, 9, 13, 17, 25, 33, 41, 49, 65, 81, 97, 113, 145, 177, 209, 241, 305, 369, 497, 753, 1265,
    2289, 4337, 8433, 16625,
];

const LITERAL_ALPHABET_SIZE: usize = 256;
const COMMAND_ALPHABET_SIZE: usize = 704;
const DISTANCE_SHORT_CODE_COUNT: usize = 16;
const DISTANCE_CODE_GROUP_COUNT: usize = 48;

pub fn decode_uncompressed_meta_blocks(
    data: &[u8],
    maximum_output_size: usize,
) -> Result<Vec<u8>, FontError> {
    let mut reader = BitReader::new(data);
    let _window_bits = read_window_bits(&mut reader)?;
    let mut output = Vec::new();

    loop {
        let is_last = reader.read_bit()?;
        let is_last_empty = is_last && reader.read_bit()?;

        if is_last_empty {
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

fn read_variable_length_u8(reader: &mut BitReader<'_>) -> Result<usize, FontError> {
    if !reader.read_bit()? {
        return Ok(0);
    }

    let bit_count = reader.read_bits(3)? as usize;

    if bit_count == 0 {
        return Ok(1);
    }

    let suffix = reader.read_bits(bit_count)? as usize;

    Ok((1_usize << bit_count) | suffix)
}

fn read_block_type_count(reader: &mut BitReader<'_>) -> Result<usize, FontError> {
    read_variable_length_u8(reader)?
        .checked_add(1)
        .ok_or_else(|| error(reader.bit_position(), "Brotli block type count overflows"))
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

#[derive(Clone, Debug)]
struct BlockPartition {
    block_type_count: usize,
    block_type_tree: Option<HuffmanTree>,
    block_length_tree: Option<HuffmanTree>,
    previous_type: usize,
    current_type: usize,
    remaining: usize,
}

impl BlockPartition {
    fn take_block_type(&mut self, reader: &mut BitReader<'_>) -> Result<usize, FontError> {
        if self.remaining == 0 {
            let block_type_tree = self.block_type_tree.as_ref().ok_or_else(|| {
                error(
                    reader.bit_position(),
                    "Brotli block partition is missing its type prerfix tree",
                )
            })?;

            let block_length_tree = self.block_length_tree.as_ref().ok_or_else(|| {
                error(
                    reader.bit_position(),
                    "Brotli block partition is missing its length prefix tree",
                )
            })?;

            let type_symbol = block_type_tree.decode(reader)?;
            let next_type = match type_symbol {
                0 => self.previous_type,
                1 => (self.current_type + 1) % self.block_type_count,
                symbol => symbol - 2,
            };

            if next_type >= self.block_type_count {
                return Err(error(
                    reader.bit_position(),
                    "Brotli block type symbol exceeds its partition",
                ));
            }

            let length_symbol = block_length_tree.decode(reader)?;
            let length = read_block_length(reader, length_symbol)?;

            self.previous_type = self.current_type;
            self.current_type = next_type;
            self.remaining = length;
        }

        self.remaining -= 1;

        Ok(self.current_type)
    }
}

#[derive(Clone, Debug)]
struct CompressedMetaBlockHeader {
    literal_partition: BlockPartition,
    command_partition: BlockPartition,
    distance_partition: BlockPartition,
    distance_postfix_bits: u8,
    direct_distance_code_count: usize,
    literal_context_modes: Vec<LiteralContextMode>,
    literal_tree_count: usize,
    literal_context_map: Vec<usize>,
    distance_tree_count: usize,
    distance_context_map: Vec<usize>,
    literal_trees: Vec<HuffmanTree>,
    command_trees: Vec<HuffmanTree>,
    distance_trees: Vec<HuffmanTree>,
}

impl CompressedMetaBlockHeader {
    fn read_command_symbol(&mut self, reader: &mut BitReader<'_>) -> Result<usize, FontError> {
        let block_type = self.command_partition.take_block_type(reader)?;

        let tree = self.command_trees.get(block_type).ok_or_else(|| {
            error(
                reader.bit_position(),
                "Brotli command block type has no Huffman tree",
            )
        })?;

        tree.decode(reader)
    }

    fn read_literal(
        &mut self,
        reader: &mut BitReader<'_>,
        previous: u8,
        before_previous: u8,
    ) -> Result<u8, FontError> {
        let block_type = self.literal_partition.take_block_type(reader)?;

        let mode = *self.literal_context_modes.get(block_type).ok_or_else(|| {
            error(
                reader.bit_position(),
                "Brotli literal block type has no context mode",
            )
        })?;

        let context = literal_context_id(mode, previous, before_previous);

        let map_index = block_type
            .checked_mul(64)
            .and_then(|index| index.checked_add(context))
            .ok_or_else(|| {
                error(
                    reader.bit_position(),
                    "Brotli literal context index overflow",
                )
            })?;

        let tree_index = *self.literal_context_map.get(map_index).ok_or_else(|| {
            error(
                reader.bit_position(),
                "Brotli literal context is missing from the context map",
            )
        })?;

        let tree = self.literal_trees.get(tree_index).ok_or_else(|| {
            error(
                reader.bit_position(),
                "Brotli literal context refers to a missing Huffman tree",
            )
        })?;

        let symbol = tree.decode(reader)?;

        u8::try_from(symbol).map_err(|_| {
            error(
                reader.bit_position(),
                "Brotli literal symbol is outside the byte range",
            )
        })
    }

    fn read_distance_symbol(
        &mut self,
        reader: &mut BitReader<'_>,
        copy_length: usize,
    ) -> Result<usize, FontError> {
        let block_type = self.distance_partition.take_block_type(reader)?;
        let context = distance_context_id(copy_length);

        let map_index = block_type
            .checked_mul(4)
            .and_then(|index| index.checked_add(context))
            .ok_or_else(|| {
                error(
                    reader.bit_position(),
                    "Brotli distance context index overflow",
                )
            })?;

        let tree_index = *self.distance_context_map.get(map_index).ok_or_else(|| {
            error(
                reader.bit_position(),
                "Brotli distance context is missing from the context map",
            )
        })?;

        let tree = self.distance_trees.get(tree_index).ok_or_else(|| {
            error(
                reader.bit_position(),
                "Brotli distance context refers to a missing Huffman tree",
            )
        })?;

        tree.decode(reader)
    }

    fn read_command(&mut self, reader: &mut BitReader<'_>) -> Result<InsertCopyCommand, FontError> {
        let symbol = self.read_command_symbol(reader)?;
        read_insert_copy_command(reader, symbol)
    }
}

fn read_compressed_meta_block_header_prefix(
    reader: &mut BitReader<'_>,
) -> Result<CompressedMetaBlockHeader, FontError> {
    let literal_partition = read_block_partition(reader)?;
    let command_partition = read_block_partition(reader)?;
    let distance_partition = read_block_partition(reader)?;

    let distance_postfix_bits = reader.read_bits(2)? as u8;
    let direct_distance_code_count =
        (reader.read_bits(4)? as usize) << usize::from(distance_postfix_bits);

    let mut literal_context_modes = Vec::with_capacity(literal_partition.block_type_count);

    for _ in 0..literal_partition.block_type_count {
        let mode = match reader.read_bits(2)? {
            0 => LiteralContextMode::Lsb6,
            1 => LiteralContextMode::Msb6,
            2 => LiteralContextMode::Utf8,
            3 => LiteralContextMode::Signed,
            _ => unreachable!(),
        };

        literal_context_modes.push(mode);
    }

    let literal_tree_count = read_block_type_count(reader)?;
    let literal_context_map_size = literal_partition
        .block_type_count
        .checked_mul(64)
        .ok_or_else(|| {
            error(
                reader.bit_position(),
                "Brotli literal context map is too large",
            )
        })?;
    let literal_context_map =
        read_context_map(reader, literal_context_map_size, literal_tree_count)?;

    let distance_tree_count = read_block_type_count(reader)?;
    let distance_context_map_size = distance_partition
        .block_type_count
        .checked_mul(4)
        .ok_or_else(|| {
            error(
                reader.bit_position(),
                "Brotli distance context map is too large",
            )
        })?;
    let distance_context_map =
        read_context_map(reader, distance_context_map_size, distance_tree_count)?;

    let literal_trees = read_prefix_code_array(reader, literal_tree_count, LITERAL_ALPHABET_SIZE)?;

    let command_trees = read_prefix_code_array(
        reader,
        command_partition.block_type_count,
        COMMAND_ALPHABET_SIZE,
    )?;

    let distance_code_group_count = DISTANCE_CODE_GROUP_COUNT
        .checked_shl(u32::from(distance_postfix_bits))
        .ok_or_else(|| {
            error(
                reader.bit_position(),
                "Brotli distance code-group count overflows",
            )
        })?;

    let distance_alphabet_size = DISTANCE_SHORT_CODE_COUNT
        .checked_add(direct_distance_code_count)
        .and_then(|count| count.checked_add(distance_code_group_count))
        .ok_or_else(|| {
            error(
                reader.bit_position(),
                "Brotli distance alphabet size overflows",
            )
        })?;

    let distance_trees =
        read_prefix_code_array(reader, distance_tree_count, distance_alphabet_size)?;

    Ok(CompressedMetaBlockHeader {
        literal_partition,
        command_partition,
        distance_partition,
        distance_postfix_bits,
        direct_distance_code_count,
        literal_context_modes,
        literal_tree_count,
        literal_context_map,
        distance_tree_count,
        distance_context_map,
        literal_trees,
        command_trees,
        distance_trees,
    })
}

fn read_context_map(
    reader: &mut BitReader<'_>,
    context_map_size: usize,
    tree_count: usize,
) -> Result<Vec<usize>, FontError> {
    if tree_count == 0 {
        return Err(error(
            reader.bit_position(),
            "Brotli context map has no prefix trees",
        ));
    }

    if tree_count == 1 {
        return Ok(vec![0; context_map_size]);
    }

    let rle_max = if reader.read_bit()? {
        reader.read_bits(4)? as usize + 1
    } else {
        0
    };

    let tree = read_prefix_code(reader, tree_count + rle_max)?;
    let mut context_map = Vec::with_capacity(context_map_size);

    while context_map.len() < context_map_size {
        let symbol = tree.decode(reader)?;

        if symbol == 0 {
            context_map.push(0);
            continue;
        }

        if symbol <= rle_max {
            let repeat_count = (1_usize << symbol)
                .checked_add(reader.read_bits(symbol)? as usize)
                .ok_or_else(|| {
                    error(
                        reader.bit_position(),
                        "Brotli context-map zero run overflows",
                    )
                })?;

            let end = context_map
                .len()
                .checked_add(repeat_count)
                .ok_or_else(|| error(reader.bit_position(), "Brotli context-map size overflows"))?;

            if end > context_map_size {
                return Err(error(
                    reader.bit_position(),
                    "Brotli context-map zero run exceeds its declared size",
                ));
            }

            context_map.resize(end, 0);
            continue;
        }

        let tree_index = symbol - rle_max;

        if tree_index >= tree_count {
            return Err(error(
                reader.bit_position(),
                "Brotli context-map tree index exceeds its tree count",
            ));
        }

        context_map.push(tree_index);
    }

    if reader.read_bit()? {
        inverse_move_to_front(&mut context_map);
    }

    let mut seen = vec![false; tree_count];

    for &tree_index in &context_map {
        seen[tree_index] = true;
    }

    if seen.iter().any(|present| !present) {
        return Err(error(
            reader.bit_position(),
            "Brotli context map does not reference every prefix tree",
        ));
    }

    Ok(context_map)
}

fn read_prefix_code_array(
    reader: &mut BitReader<'_>,
    count: usize,
    alphabet_size: usize,
) -> Result<Vec<HuffmanTree>, FontError> {
    let mut trees = Vec::with_capacity(count);

    for _ in 0..count {
        trees.push(read_prefix_code(reader, alphabet_size)?);
    }

    Ok(trees)
}

fn inverse_move_to_front(values: &mut [usize]) {
    let mut entries = Vec::with_capacity(256);

    for value in 0..256 {
        entries.push(value);
    }

    for index in values {
        let value = entries[*index];

        for position in (1..=*index).rev() {
            entries[position] = entries[position - 1];
        }

        entries[0] = value;
        *index = value;
    }
}

fn read_block_partition(reader: &mut BitReader<'_>) -> Result<BlockPartition, FontError> {
    let block_type_count = read_block_type_count(reader)?;

    if block_type_count == 1 {
        return Ok(BlockPartition {
            block_type_count,
            block_type_tree: None,
            block_length_tree: None,
            previous_type: 1,
            current_type: 0,
            remaining: IMPLICIT_BLOCK_LENGTH,
        });
    }

    let block_type_tree = read_prefix_code(reader, block_type_count + 2)?;
    let block_length_tree = read_prefix_code(reader, BLOCK_LENGTH_OFFSETS.len())?;
    let block_length_symbol = block_length_tree.decode(reader)?;
    let remaining = read_block_length(reader, block_length_symbol)?;

    Ok(BlockPartition {
        block_type_count,
        block_type_tree: Some(block_type_tree),
        block_length_tree: Some(block_length_tree),
        previous_type: 1,
        current_type: 0,
        remaining,
    })
}

fn read_block_length(reader: &mut BitReader<'_>, symbol: usize) -> Result<usize, FontError> {
    let Some(&extra_bit_count) = BLOCK_LENGTH_EXTRA_BITS.get(symbol) else {
        return Err(error(
            reader.bit_position(),
            "Brotli block length symbol exceeds its alphabet",
        ));
    };

    let extra = reader.read_bits(extra_bit_count)? as usize;

    BLOCK_LENGTH_OFFSETS[symbol]
        .checked_add(extra)
        .ok_or_else(|| error(reader.bit_position(), "Brotli block length overflows"))
}

pub fn read_prefix_code(
    reader: &mut BitReader<'_>,
    alphabet_size: usize,
) -> Result<HuffmanTree, FontError> {
    let header = reader.read_bits(2)?;

    if header == 1 {
        return read_simple_prefix_code_body(reader, alphabet_size);
    }

    read_complex_prefix_code(reader, alphabet_size, header)
}

pub fn read_simple_prefix_code(
    reader: &mut BitReader<'_>,
    alphabet_size: usize,
) -> Result<HuffmanTree, FontError> {
    if reader.read_bits(2)? != 1 {
        return Err(error(
            reader.bit_position(),
            "expected a Brotli simple prefix code",
        ));
    }

    read_simple_prefix_code_body(reader, alphabet_size)
}

fn read_simple_prefix_code_body(
    reader: &mut BitReader<'_>,
    alphabet_size: usize,
) -> Result<HuffmanTree, FontError> {
    if alphabet_size == 0 {
        return Err(error(
            reader.bit_position(),
            "Brotli prefix-code alphabet must not be empty",
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

fn read_complex_prefix_code(
    reader: &mut BitReader<'_>,
    alphabet_size: usize,
    hskip: u32,
) -> Result<HuffmanTree, FontError> {
    if alphabet_size == 0 {
        return Err(error(
            reader.bit_position(),
            "Brotli prefix-code alphabet must not be empty",
        ));
    }

    let code_length_tree = read_code_length_tree(reader, hskip)?;
    let mut code_lengths = Vec::with_capacity(alphabet_size);
    let mut last_non_zero_length = None;
    let mut non_zero_count = 0_usize;
    let mut weight = 0_u32;
    let mut pending_symbol = None;
    let mut last_encoded_symbol = None;

    while code_lengths.len() < alphabet_size {
        let symbol = match pending_symbol.take() {
            Some(symbol) => symbol,
            None => code_length_tree.decode(reader)?,
        };

        last_encoded_symbol = Some(symbol);

        match symbol {
            0..=15 => append_code_lengths(
                &mut code_lengths,
                symbol as u8,
                1,
                alphabet_size,
                &mut last_non_zero_length,
                &mut non_zero_count,
                &mut weight,
                reader.bit_position(),
            )?,

            16 | 17 => {
                let repeated_length = if symbol == 16 {
                    last_non_zero_length.unwrap_or(8)
                } else {
                    0
                };

                let mut repeat_count = if symbol == 16 {
                    reader.read_bits(2)? as usize + 3
                } else {
                    reader.read_bits(3)? as usize + 3
                };

                loop {
                    let completes = repetition_completes_prefix_code(
                        code_lengths.len(),
                        repeated_length,
                        repeat_count,
                        alphabet_size,
                        non_zero_count,
                        weight,
                        reader.bit_position(),
                    )?;

                    if completes {
                        append_code_lengths(
                            &mut code_lengths,
                            repeated_length,
                            repeat_count,
                            alphabet_size,
                            &mut last_non_zero_length,
                            &mut non_zero_count,
                            &mut weight,
                            reader.bit_position(),
                        )?;
                        break;
                    }

                    let next_symbol = code_length_tree.decode(reader)?;

                    if next_symbol != symbol {
                        append_code_lengths(
                            &mut code_lengths,
                            repeated_length,
                            repeat_count,
                            alphabet_size,
                            &mut last_non_zero_length,
                            &mut non_zero_count,
                            &mut weight,
                            reader.bit_position(),
                        )?;
                        pending_symbol = Some(next_symbol);
                        break;
                    }

                    last_encoded_symbol = Some(next_symbol);

                    let extra_count = if symbol == 16 {
                        reader.read_bits(2)? as usize + 3
                    } else {
                        reader.read_bits(3)? as usize + 3
                    };

                    let multiplier: usize = if symbol == 16 { 4 } else { 8 };

                    repeat_count = multiplier
                        .checked_mul(repeat_count.checked_sub(2).ok_or_else(|| {
                            error(reader.bit_position(), "invalid Brotli repeat count")
                        })?)
                        .and_then(|count| count.checked_add(extra_count))
                        .ok_or_else(|| {
                            error(reader.bit_position(), "Brotli repeat count overflows")
                        })?;
                }
            }

            _ => {
                return Err(error(
                    reader.bit_position(),
                    "Brotli code-length symbol exceeds its alphabet",
                ));
            }
        }

        if non_zero_count >= 2 && weight == PREFIX_CODE_WEIGHT {
            code_lengths.resize(alphabet_size, 0);
            break;
        }
    }

    if matches!(last_encoded_symbol, Some(0 | 17)) {
        return Err(error(
            reader.bit_position(),
            "Brotli prefix-code lengths end with an omitted zero run",
        ));
    }

    match non_zero_count {
        0 => Err(error(
            reader.bit_position(),
            "Brotli prefix code has no symbols",
        )),

        1 => {
            let symbol = code_lengths
                .iter()
                .position(|length| *length != 0)
                .expect("non-zero code length must exist");

            Ok(HuffmanTree::single(symbol))
        }

        _ if weight == PREFIX_CODE_WEIGHT => HuffmanTree::from_code_lengths(&code_lengths),

        _ => Err(error(
            reader.bit_position(),
            "Brotli prefix-code lengths do not form a complete tree",
        )),
    }
}

fn read_code_length_tree(reader: &mut BitReader<'_>, hskip: u32) -> Result<HuffmanTree, FontError> {
    let start = match hskip {
        0 => 0,
        2 => 2,
        3 => 3,
        _ => {
            return Err(error(
                reader.bit_position(),
                "invalid Brotli complex prefix-code header",
            ));
        }
    };

    let mut code_lengths = [0_u8; 18];
    let mut non_zero_count = 0_usize;
    let mut weight = 0_u32;

    for &symbol in CODE_LENGTH_ALPHABET_ORDER.iter().skip(start) {
        let length = read_code_length_code_length(reader)?;
        code_lengths[symbol] = length;

        if length == 0 {
            continue;
        }

        non_zero_count += 1;
        weight = weight
            .checked_add(CODE_LENGTH_ALPHABET_WEIGHT >> length)
            .ok_or_else(|| error(reader.bit_position(), "Brotli code-length weight overflows"))?;

        if weight > CODE_LENGTH_ALPHABET_WEIGHT {
            return Err(error(
                reader.bit_position(),
                "Brotli code-length tree is oversubscribed",
            ));
        }

        if non_zero_count >= 2 && weight == CODE_LENGTH_ALPHABET_WEIGHT {
            break;
        }
    }

    match non_zero_count {
        0 => Err(error(
            reader.bit_position(),
            "Brotli code-length tree has no symbols",
        )),

        1 => {
            let symbol = code_lengths
                .iter()
                .position(|length| *length != 0)
                .expect("non-zero code length must exist");

            Ok(HuffmanTree::single(symbol))
        }

        _ if weight == CODE_LENGTH_ALPHABET_WEIGHT => HuffmanTree::from_code_lengths(&code_lengths),

        _ => Err(error(
            reader.bit_position(),
            "Brotli code-length tree is incomplete",
        )),
    }
}

fn read_code_length_code_length(reader: &mut BitReader) -> Result<u8, FontError> {
    if !reader.read_bit()? {
        return Ok(if reader.read_bit()? { 3 } else { 0 });
    }

    if !reader.read_bit()? {
        return Ok(4);
    }

    if !reader.read_bit()? {
        return Ok(2);
    }

    Ok(if reader.read_bit()? { 5 } else { 1 })
}

fn repetition_completes_prefix_code(
    current_length: usize,
    repeated_length: u8,
    repeat_count: usize,
    alphabet_size: usize,
    non_zero_count: usize,
    weight: u32,
    offset: usize,
) -> Result<bool, FontError> {
    let length_after_repeat = current_length
        .checked_add(repeat_count)
        .ok_or_else(|| error(offset, "Brotli code-length count overflows"))?;

    if length_after_repeat > alphabet_size {
        return Err(error(
            offset,
            "Brotli repeated code lengths exceed the alphabet",
        ));
    }

    if repeated_length == 0 {
        return Ok(length_after_repeat == alphabet_size);
    }

    let repeated_weight = (PREFIX_CODE_WEIGHT >> repeated_length)
        .checked_mul(
            u32::try_from(repeat_count)
                .map_err(|_| error(offset, "Brotli repeat count is too large"))?,
        )
        .ok_or_else(|| error(offset, "Brotli prefix-code weight overflows"))?;

    let weight_after_repeat = weight
        .checked_add(repeated_weight)
        .ok_or_else(|| error(offset, "Brotli prefix-code weight overflows"))?;

    if weight_after_repeat > PREFIX_CODE_WEIGHT {
        return Err(error(offset, "Brotli prefix code is oversubscribed"));
    }

    let non_zero_after_repeat = non_zero_count
        .checked_add(repeat_count)
        .ok_or_else(|| error(offset, "Brotli non-zero code count overflows"))?;

    Ok(length_after_repeat == alphabet_size
        || (non_zero_after_repeat >= 2 && weight_after_repeat == PREFIX_CODE_WEIGHT))
}

fn append_code_lengths(
    code_lengths: &mut Vec<u8>,
    length: u8,
    count: usize,
    alphabet_size: usize,
    last_non_zero_length: &mut Option<u8>,
    non_zero_count: &mut usize,
    weight: &mut u32,
    offset: usize,
) -> Result<(), FontError> {
    let new_length = code_lengths
        .len()
        .checked_add(count)
        .ok_or_else(|| error(offset, "Brotli code-length count overflows"))?;

    if new_length > alphabet_size {
        return Err(error(
            offset,
            "Brotli repeated code lengths exceed the alphabet",
        ));
    }

    if length != 0 {
        let additional_weight = (PREFIX_CODE_WEIGHT >> length)
            .checked_mul(
                u32::try_from(count)
                    .map_err(|_| error(offset, "Brotli repeat count is too large"))?,
            )
            .ok_or_else(|| error(offset, "Brotli prefix-code weight overflows"))?;

        *weight = weight
            .checked_add(additional_weight)
            .ok_or_else(|| error(offset, "Brotli prefix-code weight overflows"))?;

        if *weight > PREFIX_CODE_WEIGHT {
            return Err(error(offset, "Brotli prefix code is oversubscribed"));
        }

        *non_zero_count = non_zero_count
            .checked_add(count)
            .ok_or_else(|| error(offset, "Brotli non-zero code count overflows"))?;

        *last_non_zero_length = Some(length);
    }

    code_lengths.resize(new_length, length);

    Ok(())
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

    #[test]
    fn decodes_a_complex_prefix_code() {
        let mut data = Vec::new();
        let mut bit_position = 0;

        append_bits(&mut data, &mut bit_position, 0, 2);
        append_bits(&mut data, &mut bit_position, 7, 4);

        for _ in 0..17 {
            append_bits(&mut data, &mut bit_position, 0, 2);
        }

        append_bits(&mut data, &mut bit_position, 0, 1);
        append_bits(&mut data, &mut bit_position, 1, 1);

        let mut reader = BitReader::new(&data);
        let tree = read_prefix_code(&mut reader, 2).unwrap();

        assert_eq!(tree.decode(&mut reader).unwrap(), 0);
        assert_eq!(tree.decode(&mut reader).unwrap(), 1);
    }

    #[test]
    fn expands_repeated_complex_code_lengths() {
        let mut data = Vec::new();
        let mut bit_position = 0;

        append_bits(&mut data, &mut bit_position, 0, 2);
        append_bits(&mut data, &mut bit_position, 0, 2);
        append_bits(&mut data, &mut bit_position, 0, 2);
        append_bits(&mut data, &mut bit_position, 7, 4);

        for _ in 0..5 {
            append_bits(&mut data, &mut bit_position, 0, 2);
        }

        append_bits(&mut data, &mut bit_position, 7, 4);

        append_bits(&mut data, &mut bit_position, 0, 1);
        append_bits(&mut data, &mut bit_position, 1, 1);
        append_bits(&mut data, &mut bit_position, 0, 2);
        append_bits(&mut data, &mut bit_position, 1, 1);
        append_bits(&mut data, &mut bit_position, 0, 2);
        append_bits(&mut data, &mut bit_position, 5, 3);

        let mut reader = BitReader::new(&data);
        let tree = read_prefix_code(&mut reader, 8).unwrap();

        assert_eq!(tree.decode(&mut reader).unwrap(), 5);
    }

    #[test]
    fn decodes_brotli_block_type_counts() {
        for expected in [
            1_usize, 2, 3, 4, 5, 8, 9, 16, 17, 32, 33, 64, 65, 128, 129, 256,
        ] {
            let mut data = Vec::new();
            let mut bit_position = 0;

            append_variable_length_u8(&mut data, &mut bit_position, expected - 1);

            let mut reader = BitReader::new(&data);

            assert_eq!(read_block_type_count(&mut reader).unwrap(), expected);
        }
    }

    #[test]
    fn reads_a_single_type_brotli_block_partition() {
        let mut data = Vec::new();
        let mut bit_position = 0;

        append_variable_length_u8(&mut data, &mut bit_position, 0);

        let mut reader = BitReader::new(&data);
        let partition = read_block_partition(&mut reader).unwrap();

        assert_eq!(partition.block_type_count, 1);
        assert_eq!(partition.current_type, 0);
        assert_eq!(partition.previous_type, 1);
        assert_eq!(partition.remaining, IMPLICIT_BLOCK_LENGTH);
        assert!(partition.block_type_tree.is_none());
        assert!(partition.block_length_tree.is_none());
    }

    #[test]
    fn reads_a_multi_type_brotli_block_partition() {
        let mut data = Vec::new();
        let mut bit_position = 0;

        append_variable_length_u8(&mut data, &mut bit_position, 1);

        append_bits(&mut data, &mut bit_position, 1, 2);
        append_bits(&mut data, &mut bit_position, 0, 2);
        append_bits(&mut data, &mut bit_position, 0, 2);

        append_bits(&mut data, &mut bit_position, 1, 2);
        append_bits(&mut data, &mut bit_position, 0, 2);
        append_bits(&mut data, &mut bit_position, 0, 5);

        append_bits(&mut data, &mut bit_position, 3, 2);

        let mut reader = BitReader::new(&data);
        let partition = read_block_partition(&mut reader).unwrap();

        assert_eq!(partition.block_type_count, 2);
        assert_eq!(partition.current_type, 0);
        assert_eq!(partition.previous_type, 1);
        assert_eq!(partition.remaining, 4);
        assert!(partition.block_type_tree.is_some());
        assert!(partition.block_length_tree.is_some());
    }

    #[test]
    fn switches_brotli_block_types_after_the_current_block_expires() {
        let mut data = Vec::new();
        let mut bit_position = 0;

        append_bits(&mut data, &mut bit_position, 2, 2);

        let mut reader = BitReader::new(&data);
        let mut partition = BlockPartition {
            block_type_count: 3,
            block_type_tree: Some(HuffmanTree::single(1)),
            block_length_tree: Some(HuffmanTree::single(0)),
            previous_type: 1,
            current_type: 0,
            remaining: 0,
        };

        assert_eq!(partition.take_block_type(&mut reader).unwrap(), 1);
        assert_eq!(partition.previous_type, 0);
        assert_eq!(partition.current_type, 1);
        assert_eq!(partition.remaining, 2);

        assert_eq!(partition.take_block_type(&mut reader).unwrap(), 1);
        assert_eq!(partition.take_block_type(&mut reader).unwrap(), 1);
        assert_eq!(partition.remaining, 0);
    }

    #[test]
    fn reuses_the_previous_brotli_block_type() {
        let mut data = Vec::new();
        let mut bit_position = 0;

        append_bits(&mut data, &mut bit_position, 0, 2);

        let mut reader = BitReader::new(&data);
        let mut partition = BlockPartition {
            block_type_count: 3,
            block_type_tree: Some(HuffmanTree::single(0)),
            block_length_tree: Some(HuffmanTree::single(0)),
            previous_type: 1,
            current_type: 0,
            remaining: 0,
        };

        assert_eq!(partition.take_block_type(&mut reader).unwrap(), 1);
        assert_eq!(partition.previous_type, 0);
        assert_eq!(partition.current_type, 1);
        assert_eq!(partition.remaining, 0);
    }

    #[test]
    fn reads_brotli_compressed_meta_block_header_prefix() {
        let mut data = Vec::new();
        let mut bit_position = 0;

        append_variable_length_u8(&mut data, &mut bit_position, 0);
        append_variable_length_u8(&mut data, &mut bit_position, 0);
        append_variable_length_u8(&mut data, &mut bit_position, 0);

        append_bits(&mut data, &mut bit_position, 2, 2);
        append_bits(&mut data, &mut bit_position, 7, 4);
        append_bits(&mut data, &mut bit_position, 3, 2);

        append_variable_length_u8(&mut data, &mut bit_position, 0);
        append_variable_length_u8(&mut data, &mut bit_position, 0);

        append_single_symbol_prefix_code(&mut data, &mut bit_position, 256, 65);
        append_single_symbol_prefix_code(&mut data, &mut bit_position, 704, 17);
        append_single_symbol_prefix_code(&mut data, &mut bit_position, 236, 9);

        let mut reader = BitReader::new(&data);
        let header = read_compressed_meta_block_header_prefix(&mut reader).unwrap();

        assert_eq!(header.literal_partition.block_type_count, 1);
        assert_eq!(header.command_partition.block_type_count, 1);
        assert_eq!(header.distance_partition.block_type_count, 1);
        assert_eq!(header.distance_postfix_bits, 2);
        assert_eq!(header.direct_distance_code_count, 28);
        assert_eq!(
            header.literal_context_modes,
            vec![LiteralContextMode::Signed]
        );

        assert_eq!(header.literal_tree_count, 1);
        assert_eq!(header.distance_tree_count, 1);
        assert_eq!(header.literal_context_map.len(), 64);
        assert_eq!(header.distance_context_map.len(), 4);
        assert!(header.literal_context_map.iter().all(|index| *index == 0));
        assert!(header.distance_context_map.iter().all(|index| *index == 0));

        assert_eq!(header.literal_trees.len(), 1);
        assert_eq!(header.command_trees.len(), 1);
        assert_eq!(header.distance_trees.len(), 1);

        let mut empty_reader = BitReader::new(&[]);

        assert_eq!(
            header.literal_trees[0].decode(&mut empty_reader).unwrap(),
            65
        );
        assert_eq!(
            header.command_trees[0].decode(&mut empty_reader).unwrap(),
            17
        );
        assert_eq!(
            header.distance_trees[0].decode(&mut empty_reader).unwrap(),
            9
        );
    }

    #[test]
    fn decodes_brotli_context_map_with_zero_runs() {
        let mut data = Vec::new();
        let mut bit_position = 0;

        append_bits(&mut data, &mut bit_position, 1, 1);
        append_bits(&mut data, &mut bit_position, 0, 4);

        append_bits(&mut data, &mut bit_position, 1, 2);
        append_bits(&mut data, &mut bit_position, 1, 2);
        append_bits(&mut data, &mut bit_position, 1, 2);
        append_bits(&mut data, &mut bit_position, 2, 2);

        append_bits(&mut data, &mut bit_position, 0, 1);
        append_bits(&mut data, &mut bit_position, 1, 1);
        append_bits(&mut data, &mut bit_position, 1, 1);

        append_bits(&mut data, &mut bit_position, 0, 1);

        let mut reader = BitReader::new(&data);

        assert_eq!(
            read_context_map(&mut reader, 4, 2).unwrap(),
            vec![0, 0, 0, 1]
        );
    }

    #[test]
    fn applies_inverse_move_to_front_to_a_brotli_context_map() {
        let mut context_map = vec![1, 1, 0, 1];

        inverse_move_to_front(&mut context_map);

        assert_eq!(context_map, vec![1, 0, 0, 1]);
    }

    #[test]
    fn reads_brotli_block_lengths_from_the_length_tree() {
        let mut data = Vec::new();
        let mut bit_position = 0;

        append_variable_length_u8(&mut data, &mut bit_position, 1);
        append_single_symbol_prefix_code(&mut data, &mut bit_position, 4, 0);
        append_single_symbol_prefix_code(&mut data, &mut bit_position, 26, 1);
        append_bits(&mut data, &mut bit_position, 0, 2);

        let mut reader = BitReader::new(&data);
        let partition = read_block_partition(&mut reader).unwrap();

        assert_eq!(partition.block_type_count, 2);
        assert_eq!(partition.remaining, 5);
    }

    #[test]
    fn computes_brotli_literal_context_ids() {
        assert_eq!(literal_context_id(LiteralContextMode::Lsb6, 0xd5, 0), 0x15);
        assert_eq!(literal_context_id(LiteralContextMode::Msb6, 0xd5, 0), 0x35);
        assert_eq!(literal_context_id(LiteralContextMode::Utf8, b'A', b'a'), 51);
        assert_eq!(
            literal_context_id(LiteralContextMode::Signed, 0xf0, 0x10),
            50
        );
        assert_eq!(literal_context_id(LiteralContextMode::Utf8, b'U', 0), 48);
        assert_eq!(literal_context_id(LiteralContextMode::Utf8, b'V', 0), 52);
    }

    #[test]
    fn computes_brotli_distance_context_ids() {
        assert_eq!(distance_context_id(2), 0);
        assert_eq!(distance_context_id(3), 1);
        assert_eq!(distance_context_id(4), 2);
        assert_eq!(distance_context_id(5), 3);
        assert_eq!(distance_context_id(128), 3);
    }

    #[test]
    fn selects_brotli_trees_using_block_types_and_contexts() {
        fn partition(current_type: usize) -> BlockPartition {
            BlockPartition {
                block_type_count: 2,
                block_type_tree: None,
                block_length_tree: None,
                previous_type: 0,
                current_type,
                remaining: 2,
            }
        }

        let mut literal_context_map = vec![0; 128];
        literal_context_map[64 + 32] = 1;

        let mut distance_context_map = vec![0; 8];
        distance_context_map[4 + 1] = 1;

        let mut header = CompressedMetaBlockHeader {
            literal_partition: partition(1),
            command_partition: partition(1),
            distance_partition: partition(1),
            distance_postfix_bits: 0,
            direct_distance_code_count: 0,
            literal_context_modes: vec![LiteralContextMode::Lsb6, LiteralContextMode::Msb6],
            literal_tree_count: 2,
            literal_context_map,
            distance_tree_count: 2,
            distance_context_map,
            literal_trees: vec![HuffmanTree::single(65), HuffmanTree::single(66)],
            command_trees: vec![HuffmanTree::single(17), HuffmanTree::single(37)],
            distance_trees: vec![HuffmanTree::single(9), HuffmanTree::single(19)],
        };

        let mut reader = BitReader::new(&[]);

        assert_eq!(header.read_command_symbol(&mut reader).unwrap(), 37);

        // MSB6 context for 0x80 is 32.
        assert_eq!(header.read_literal(&mut reader, 0x80, 0).unwrap(), 66);

        // A copy length of 3 produces distance context 1.
        assert_eq!(header.read_distance_symbol(&mut reader, 3).unwrap(), 19);

        assert_eq!(header.command_partition.remaining, 1);
        assert_eq!(header.literal_partition.remaining, 1);
        assert_eq!(header.distance_partition.remaining, 1);
    }

    #[test]
    fn decodes_brotli_insert_copy_commands() {
        let mut reader = BitReader::new(&[]);

        assert_eq!(
            read_insert_copy_command(&mut reader, 0).unwrap(),
            InsertCopyCommand {
                insert_length: 0,
                copy_length: 2,
                implicit_distance: true,
            }
        );

        assert_eq!(
            read_insert_copy_command(&mut reader, 128).unwrap(),
            InsertCopyCommand {
                insert_length: 0,
                copy_length: 2,
                implicit_distance: false,
            }
        );

        let mut reader = BitReader::new(&[0b0000_0011]);

        // Symbol 64 uses insert code 0 and copy code 8.
        // Copy code 8 has one extra bit.
        assert_eq!(
            read_insert_copy_command(&mut reader, 64).unwrap(),
            InsertCopyCommand {
                insert_length: 0,
                copy_length: 11,
                implicit_distance: true,
            }
        );
        assert_eq!(reader.bit_position(), 1);
    }

    #[test]
    fn decodes_brotli_large_insert_copy_lengths() {
        let mut reader = BitReader::new(&[0xff; 6]);

        assert_eq!(
            read_insert_copy_command(&mut reader, 703).unwrap(),
            InsertCopyCommand {
                insert_length: 16_799_809,
                copy_length: 16_779_333,
                implicit_distance: false,
            }
        );
        assert_eq!(reader.bit_position(), 48);
    }

    #[test]
    fn rejects_invalid_or_truncated_brotli_commands() {
        let mut reader = BitReader::new(&[]);
        assert!(read_insert_copy_command(&mut reader, 704).is_err());

        let mut reader = BitReader::new(&[0; 5]);
        assert!(read_insert_copy_command(&mut reader, 703).is_err());
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

    fn append_variable_length_u8(data: &mut Vec<u8>, bit_position: &mut usize, value: usize) {
        if value == 0 {
            append_bits(data, bit_position, 0, 1);
            return;
        }

        append_bits(data, bit_position, 1, 1);

        let bit_count = (usize::BITS - value.leading_zeros() - 1) as usize;
        append_bits(data, bit_position, bit_count as u32, 3);
        append_bits(
            data,
            bit_position,
            (value - (1_usize << bit_count)) as u32,
            bit_count,
        );
    }

    fn append_single_symbol_prefix_code(
        data: &mut Vec<u8>,
        bit_position: &mut usize,
        alphabet_size: usize,
        symbol: usize,
    ) {
        append_bits(data, bit_position, 1, 2);
        append_bits(data, bit_position, 0, 2);
        append_bits(
            data,
            bit_position,
            symbol as u32,
            alphabet_bit_width(alphabet_size).unwrap(),
        );
    }
}
