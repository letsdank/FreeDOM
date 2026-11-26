//! WOFF2 transformed hmtx reconstruction.
//!
//! Specification: <https://www.w3.org/TR/WOFF2/#hmtx_table_format>

use super::super::{reader::FontReader, sfnt::FontError};
use super::error;

/// Reconstructs a TrueType hmtx table.
///
/// `x_mins` contains one value per glyph. Empty glyphs use zero.
pub(super) fn reconstruct(
    data: &[u8],
    num_metrics: u16,
    x_mins: &[i16],
    original_length: usize,
) -> Result<Vec<u8>, FontError> {
    let metrics = usize::from(num_metrics);

    if metrics == 0 || metrics > x_mins.len() {
        return Err(error(0, "invalid WOFF2 hmtx metric count"));
    }

    let flags = FontReader::new(data, 0).u8(0)?;

    // At least one bearing array must be omitted.
    // Bits 2-7 are reserved.
    if flags & !3 != 0 || flags & 3 == 0 {
        return Err(error(0, "invalid WOFF2 hmtx transform flags"));
    }

    let extra = x_mins.len() - metrics;
    let expected_original = metrics * 4 + extra * 2;

    if original_length != expected_original {
        return Err(error(
            0,
            "WOFF2 hmtx original length does not match metrics",
        ));
    }

    let advance_start = 1;
    let proportional_start = advance_start + metrics * 2;

    let trailing_start = proportional_start + if flags & 1 == 0 { metrics * 2 } else { 0 };

    let expected_transformed = trailing_start + if flags & 2 == 0 { extra * 2 } else { 0 };

    if data.len() != expected_transformed {
        return Err(error(0, "invalid WOFF2 hmtx transformed length"));
    }

    let reader = FontReader::new(data, 0);
    let mut output = Vec::with_capacity(expected_original);

    for index in 0..metrics {
        let advance = reader.u16(advance_start + index * 2)?;

        let lsb = if flags & 1 == 0 {
            reader.i16(proportional_start + index * 2)?
        } else {
            x_mins[index]
        };

        output.extend_from_slice(&advance.to_be_bytes());
        output.extend_from_slice(&lsb.to_be_bytes());
    }

    for index in 0..extra {
        let lsb = if flags & 2 == 0 {
            reader.i16(trailing_start + index * 2)?
        } else {
            x_mins[metrics + index]
        };

        output.extend_from_slice(&lsb.to_be_bytes());
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconstructs_all_valid_hmtx_flag_combinations() {
        let x_mins = [0, -20, 40, -5];

        let cases: &[(&[u8], &[u8])] = &[
            (
                &[1, 1, 244, 2, 88, 0, 7, 0xff, 0xf8],
                &[1, 244, 0, 0, 2, 88, 0xff, 0xec, 0, 7, 0xff, 0xf8],
            ),
            (
                &[2, 1, 244, 2, 88, 0xff, 0xfd, 0, 4],
                &[1, 244, 0xff, 0xfd, 2, 88, 0, 4, 0, 40, 0xff, 0xfb],
            ),
            (
                &[3, 1, 244, 2, 88],
                &[1, 244, 0, 0, 2, 88, 0xff, 0xec, 0, 40, 0xff, 0xfb],
            ),
        ];

        for &(source, expected) in cases {
            let result = reconstruct(source, 2, &x_mins, 12).unwrap();

            assert_eq!(result, expected, "flags: {}", source[0]);
        }
    }

    #[test]
    fn rejects_bad_flags_counts_and_lengths() {
        for flag in [0, 4, 5, 255] {
            assert!(reconstruct(&[flag], 1, &[0], 4).is_err());
        }

        assert!(reconstruct(&[3, 0, 10], 0, &[0], 4).is_err());
        assert!(reconstruct(&[3, 0, 10], 2, &[0], 4).is_err());
        assert!(reconstruct(&[3, 0], 1, &[0], 4).is_err());
        assert!(reconstruct(&[3, 0, 10, 99], 1, &[0], 4).is_err());
        assert!(reconstruct(&[3, 0, 10], 1, &[0], 6).is_err());
    }
}
