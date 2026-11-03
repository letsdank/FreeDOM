//! Restores filtered Adam7 passes into ordinary packed PNG scanlines.
//!
//! PNG Third Edition, section 8 and 9:
//! <https://www.w3.org/TR/png-3/>

use super::super::{PngDecodeError, unfilter_row};

/// `bits_per_pixel` must be a valid PNG pixel depth.
///
/// `filtered` is modified in place, and `output` is replaced with
/// unfiltered scanlines without filter bytes. Both buffers must have
/// exactly the planned sizes.
pub(in super::super) fn reconstruct(
    width: usize,
    height: usize,
    bits_per_pixel: usize,
    filtered: &mut [u8],
    output: &mut [u8],
) -> Result<(), PngDecodeError> {
    let plan = super::plan(width, height, bits_per_pixel)?;

    if filtered.len() != plan.filtered_len {
        return Err(PngDecodeError::ScanlineLength {
            expected: plan.filtered_len,
            actual: filtered.len(),
        });
    }

    let row_bytes = width
        .checked_mul(bits_per_pixel)
        .and_then(|bits| bits.checked_add(7))
        .ok_or(PngDecodeError::SizeOverflow)?
        / 8;

    let output_len = row_bytes
        .checked_mul(height)
        .ok_or(PngDecodeError::SizeOverflow)?;

    if output.len() != output_len {
        return Err(PngDecodeError::Adam7OutputLength {
            expected: output_len,
            actual: output.len(),
        });
    }

    output.fill(0);

    let bytes_per_pixel = bits_per_pixel.div_ceil(8);
    let mut offset = 0;

    for (pass_index, pass) in plan.passes.iter().enumerate() {
        if pass.filtered_len == 0 {
            continue;
        }

        let stride = pass.row_bytes + 1;

        for row_index in 0..pass.height {
            let start = offset + row_index * stride;
            let (before, after) = filtered.split_at_mut(start);

            // The previous row belongs to the current pass only.
            let previous = if row_index == 0 {
                None
            } else {
                Some(&before[start - stride + 1..start])
            };

            let filter = after[0];
            let row = &mut after[1..stride];

            unfilter_row(filter, row, previous, bytes_per_pixel).map_err(|source| {
                PngDecodeError::Adam7Filter {
                    pass: pass_index + 1,
                    row: row_index,
                    source,
                }
            })?;

            let target_y = pass.y_start + row_index * pass.y_step;

            for x in 0..pass.width {
                let target_x = pass.x_start + x * pass.x_step;

                if bits_per_pixel < 8 {
                    let source_bit = x * bits_per_pixel;
                    let source_shift = 8 - bits_per_pixel - source_bit % 8;

                    let mask = ((1u16 << bits_per_pixel) - 1) as u8;
                    let sample = (row[source_bit / 8] >> source_shift) & mask;

                    let target_bit = target_x * bits_per_pixel;
                    let target_shift = 8 - bits_per_pixel - target_bit % 8;

                    let target = target_y * row_bytes + target_bit / 8;

                    output[target] |= sample << target_shift;
                } else {
                    let source = x * bytes_per_pixel;

                    let target = target_y * row_bytes + target_x * bytes_per_pixel;

                    output[target..target + bytes_per_pixel]
                        .copy_from_slice(&row[source..source + bytes_per_pixel]);
                }
            }
        }

        offset += pass.filtered_len;
    }

    Ok(())
}

#[cfg(test)]
mod tests;
