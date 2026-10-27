//! Brotli block partitions and block-switch commands.
//!
//! Specification: RFC 7932, section 6.
//! <https://www.rfc-editor.org/rfc/rfc7932.html#section-6>

use super::{BitReader, FontError, HuffmanTree, error, read_block_type_count, read_prefix_code};

pub(super) const IMPLICIT_BLOCK_LENGTH: usize = 1 << 24;

// RFC 7932, section 6: block count codes.
const BLOCK_LENGTH_EXTRA_BITS: [usize; 26] = [
    2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 6, 6, 7, 8, 9, 10, 11, 12, 13, 24,
];

const BLOCK_LENGTH_OFFSETS: [usize; 26] = [
    1, 5, 9, 13, 17, 25, 33, 41, 49, 65, 81, 97, 113, 145, 177, 209, 241, 305, 369, 497, 753, 1265,
    2289, 4337, 8433, 16625,
];

#[derive(Clone, Debug)]
pub(super) struct BlockPartition {
    pub(super) block_type_count: usize,
    pub(super) block_type_tree: Option<HuffmanTree>,
    pub(super) block_length_tree: Option<HuffmanTree>,
    pub(super) previous_type: usize,
    pub(super) current_type: usize,
    pub(super) remaining: usize,
}

impl BlockPartition {
    pub(super) fn take_block_type(
        &mut self,
        reader: &mut BitReader<'_>,
    ) -> Result<usize, FontError> {
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

pub(super) fn read_block_partition(
    reader: &mut BitReader<'_>,
) -> Result<BlockPartition, FontError> {
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
