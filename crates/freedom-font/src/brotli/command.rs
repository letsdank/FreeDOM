//! Decoding of Brotli insert-and-copy commands.
//!
//! Specification: RFC 7932, section 5.
//! <https://www.rfc-editor.org/rfc/rfc7932.html#section-5>

use super::{BitReader, FontError, error};

#[derive(Debug, PartialEq, Eq)]
pub(super) struct InsertCopyCommand {
    pub(super) insert_length: usize,
    pub(super) copy_length: usize,
    pub(super) implicit_distance: bool,
}

// RFC 7932, section 5: insert-and-copy length codes.
pub(super) fn read_insert_copy_command(
    reader: &mut BitReader<'_>,
    symbol: usize,
) -> Result<InsertCopyCommand, FontError> {
    const INSERT_BASE: [usize; 24] = [
        0, 1, 2, 3, 4, 5, 6, 8, 10, 14, 18, 26, 34, 50, 66, 98, 130, 194, 322, 578, 1090, 2114,
        6210, 22594,
    ];

    const INSERT_EXTRA_BITS: [usize; 24] = [
        0, 0, 0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 7, 8, 9, 10, 12, 14, 24,
    ];

    const COPY_BASE: [usize; 24] = [
        2, 3, 4, 5, 6, 7, 8, 9, 10, 12, 14, 18, 22, 30, 38, 54, 70, 102, 134, 198, 326, 582, 1094,
        2118,
    ];

    const COPY_EXTRA_BITS: [usize; 24] = [
        0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 7, 8, 9, 10, 24,
    ];

    // The entries map each group of 64 symbols to insertion and copy code ranges.
    const CODE_GROUPS: [(usize, usize); 11] = [
        (0, 0),
        (0, 8),
        (0, 0),
        (0, 8),
        (8, 0),
        (8, 8),
        (0, 16),
        (16, 0),
        (8, 16),
        (16, 8),
        (16, 16),
    ];

    let &(insert_group, copy_group) = CODE_GROUPS.get(symbol / 64).ok_or_else(|| {
        error(
            reader.bit_position(),
            "Brotli insert-and-copy symbol is outside its alphabet",
        )
    })?;

    let insert_code = insert_group + ((symbol >> 3) & 7);
    let copy_code = copy_group + (symbol & 7);

    let insert_extra = reader.read_bits(INSERT_EXTRA_BITS[insert_code])? as usize;
    let copy_extra = reader.read_bits(COPY_EXTRA_BITS[copy_code])? as usize;

    Ok(InsertCopyCommand {
        insert_length: INSERT_BASE[insert_code] + insert_extra,
        copy_length: COPY_BASE[copy_code] + copy_extra,
        implicit_distance: symbol < 128,
    })
}
