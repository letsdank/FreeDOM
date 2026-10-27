//! Parsing and symbol selection for compressed Brotli meta-blocks.
//!
//! Specifications:
//! - Context maps: RFC 7932, section 7.3.
//! - Meta-block headers: RFC 7932, section 9.2.
//! - Meta-block data: RFC 7932, section 9.3.
//! <https://www.rfc-editor.org/rfc/rfc7932.html>

use super::block::{BlockPartition, read_block_partition};
use super::command::{InsertCopyCommand, read_insert_copy_command};
use super::context::{LiteralContextMode, distance_context_id, literal_context_id};
use super::dictionary::expand_word;
use super::{BitReader, FontError, HuffmanTree, error, read_block_type_count, read_prefix_code};
use crate::graphics::font::brotli::distance::{DistanceHistory, read_distance};

const LITERAL_ALPHABET_SIZE: usize = 256;
const COMMAND_ALPHABET_SIZE: usize = 704;
const DISTANCE_SHORT_CODE_COUNT: usize = 16;
const DISTANCE_CODE_GROUP_COUNT: usize = 48;

#[derive(Clone, Debug)]
pub(super) struct CompressedMetaBlockHeader {
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

pub(super) fn read_compressed_meta_block_header_prefix(
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

/// Decodes a compressed Brotli meta-block.
///
/// Static dictionary references are not supported yet.
pub(super) fn decode_compressed_meta_block(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
    block_length: usize,
    window_size: usize,
    history: &mut DistanceHistory,
) -> Result<(), FontError> {
    let block_end = output
        .len()
        .checked_add(block_length)
        .ok_or_else(|| error(reader.bit_position(), "Brotli output size overflows"))?;

    let mut header = read_compressed_meta_block_header_prefix(reader)?;

    while output.len() < block_end {
        let command = header.read_command(reader)?;
        let remaining = block_end - output.len();

        if command.insert_length > remaining {
            return Err(error(
                reader.bit_position(),
                "Brotli literal insertion exceeds the meta-block length",
            ));
        }

        for _ in 0..command.insert_length {
            let previous = output.last().copied().unwrap_or(0);
            let before_previous = output
                .len()
                .checked_sub(2)
                .and_then(|index| output.get(index))
                .copied()
                .unwrap_or(0);

            let literal = header.read_literal(reader, previous, before_previous)?;
            output.push(literal);
        }

        // The final command can end after its literals.
        if output.len() == block_end {
            return Ok(());
        }

        let symbol = if command.implicit_distance {
            0
        } else {
            header.read_distance_symbol(reader, command.copy_length)?
        };

        let distance = read_distance(
            reader,
            symbol,
            header.distance_postfix_bits,
            header.direct_distance_code_count,
            history,
        )?;

        let maximum_backward_distance = output.len().min(window_size);

        if distance > maximum_backward_distance {
            let word = expand_word(
                command.copy_length,
                distance,
                maximum_backward_distance,
                reader.bit_position(),
            )?;

            if word.len() > block_end - output.len() {
                return Err(error(
                    reader.bit_position(),
                    "Brotli dictionary word exceeds the meta-block length",
                ));
            }

            output.extend_from_slice(&word);

            // Dictionary references do not update the distance history.
            continue;
        }

        if command.copy_length > block_end - output.len() {
            return Err(error(
                reader.bit_position(),
                "Brotli backward copy exceeds the meta-block length",
            ));
        }

        history.record(symbol, distance, false, reader.bit_position())?;

        // The source can overlap the bytes being appended.
        for _ in 0..command.copy_length {
            let byte = output[output.len() - distance];
            output.push(byte);
        }
    }

    Ok(())
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

#[cfg(test)]
mod tests {
    use super::super::tests::{
        append_bits, append_single_symbol_prefix_code, append_variable_length_u8,
    };
    use super::*;

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
}
