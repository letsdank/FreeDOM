//! Parsing of Brotli simple and complex prefix codes.
//!
//! Specification: RFC 7932, sections 3.4 and 3.5.
//! <https://www.rfc-editor.org/rfc/rfc7932.html#section-3>

use super::{BitReader, FontError, HuffmanTree, error};

const CODE_LENGTH_ALPHABET_WEIGHT: u32 = 32;
const PREFIX_CODE_WEIGHT: u32 = 32_768;

// RFC 7932, section 3.5: order of code-length code lengths.
const CODE_LENGTH_ALPHABET_ORDER: [usize; 18] =
    [1, 2, 3, 4, 0, 5, 17, 6, 16, 7, 8, 9, 10, 11, 12, 13, 14, 15];

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

pub(super) fn alphabet_bit_width(alphabet_size: usize) -> Result<usize, FontError> {
    if alphabet_size == 0 {
        return Err(error(0, "Brotli prefix-code alphabet must not be empty"));
    }

    Ok((usize::BITS - (alphabet_size - 1).leading_zeros()) as usize)
}
