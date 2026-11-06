//! Dequantization and a reference 8-bit inverse DCT.
//!
//! ITU-T T.81, sections A.3.3, A.3.4 and F.2.1.5:
//! <https://www.w3.org/Graphics/JPEG/itu-t81.pdf>
//!
//! This direct floating-point implementation favors clarity over speed and
//! does not claim a bit-exact match with other IDCT implementations.

use std::{
    f64::consts::{FRAC_1_SQRT_2, PI},
    sync::OnceLock,
};

use super::QuantizationTable;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DequantizeError {
    InvalidPrecision(u8),
    InvalidValue { index: usize, value: u16 },
}

impl std::fmt::Display for DequantizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JPEG dequantization error: {self:?}")
    }
}

impl std::error::Error for DequantizeError {}

/// Multiplies row-major quantized coefficients by a row-major DQT table.
pub fn dequantize(
    block: &[i32; 64],
    table: &QuantizationTable,
) -> Result<[i64; 64], DequantizeError> {
    if table.precision != 8 && table.precision != 16 {
        return Err(DequantizeError::InvalidPrecision(table.precision));
    }

    let mut result = [0i64; 64];

    for index in 0..64 {
        let value = table.values[index];

        if value == 0 || (table.precision == 8 && value > 255) {
            return Err(DequantizeError::InvalidValue { index, value });
        }

        result[index] = i64::from(block[index]) * i64::from(value);
    }

    Ok(result)
}

/// Produces one component's unsigned 8x8 sample block in row-major order.
pub fn idct_8bit(block: &[i64; 64]) -> [u8; 64] {
    static BASIS: OnceLock<[[f64; 8]; 8]> = OnceLock::new();

    let basis = BASIS.get_or_init(|| {
        std::array::from_fn(|position| {
            std::array::from_fn(|frequency| {
                let scale = if frequency == 0 { FRAC_1_SQRT_2 } else { 1.0 };
                let angle = (2 * position + 1) as f64 * frequency as f64 * PI / 16.0;

                scale * angle.cos()
            })
        })
    });

    let mut samples = [0u8; 64];

    for y in 0..8 {
        for x in 0..8 {
            let mut sum = 0.0;

            for v in 0..8 {
                for u in 0..8 {
                    sum += block[v * 8 + u] as f64 * basis[x][u] * basis[y][v];
                }
            }

            // Undo the 8-bit sample level shift and clamp the result.
            samples[y * 8 + x] = (sum / 4.0 + 128.0).round().clamp(0.0, 255.0) as u8;
        }
    }

    samples
}

#[cfg(test)]
mod tests {
    use super::super::{
        EntropyReader, HuffmanDecoder, decode_baseline_block, read_first_scan_header,
        read_frame_header, read_huffman_tables, read_quantization_tables,
    };
    use super::*;

    #[test]
    fn dequantizes_in_natural_order_and_checks_table_values() {
        let mut table = QuantizationTable {
            precision: 8,
            values: [1; 64],
        };

        table.values[8] = 7;

        let mut block = [0; 64];
        block[0] = -8;
        block[8] = 3;

        let result = dequantize(&block, &table).unwrap();

        assert_eq!((result[0], result[8]), (-8, 21));

        table.values[8] = 0;

        assert_eq!(
            dequantize(&block, &table),
            Err(DequantizeError::InvalidValue { index: 8, value: 0 })
        );

        table.values[8] = 256;

        assert_eq!(
            dequantize(&block, &table),
            Err(DequantizeError::InvalidValue {
                index: 8,
                value: 256
            })
        );

        table.precision = 7;

        assert_eq!(
            dequantize(&block, &table),
            Err(DequantizeError::InvalidPrecision(7))
        );
    }

    #[test]
    fn dc_only_blocks_are_constant_and_clamped() {
        let mut coefficients = [0i64; 64];

        assert_eq!(idct_8bit(&coefficients), [128; 64]);

        coefficients[0] = 80;
        assert_eq!(idct_8bit(&coefficients), [138; 64]);

        coefficients[0] = -1024;
        assert_eq!(idct_8bit(&coefficients), [0; 64]);

        coefficients[0] = 4096;
        assert_eq!(idct_8bit(&coefficients), [255; 64]);
    }

    #[test]
    fn matches_libjpeg_reference_with_nonzero_ac_coefficients() {
        // Reference samples were generated with libjpeg's JDCT_ISLOW.
        let coefficients = [
            -24, -49, -62, -4, 4, -2, -2, 1, -31, -57, 33, 3, -8, 2, 1, -2, -20, 24, 2, -12, 7, -2,
            -1, 2, -14, 11, -11, 8, -3, 0, 1, -2, 5, -5, 1, 0, -1, 1, -1, 1, -2, -1, 2, -3, 2, -1,
            0, 0, 0, 0, -1, 1, 0, -1, 1, -1, -1, 1, 0, 0, -1, 2, -2, 1,
        ];

        let table = QuantizationTable {
            precision: 8,
            values: [
                5, 4, 3, 5, 8, 14, 17, 21, 4, 4, 5, 6, 9, 20, 20, 19, 5, 4, 5, 8, 14, 19, 23, 19,
                5, 6, 7, 10, 17, 30, 27, 21, 6, 7, 13, 19, 23, 37, 35, 26, 8, 12, 19, 22, 28, 35,
                38, 31, 17, 22, 27, 30, 35, 41, 41, 34, 24, 31, 32, 33, 38, 34, 35, 34,
            ],
        };

        let expected: [u8; 64] = [
            0, 26, 33, 65, 77, 85, 115, 135, 20, 32, 58, 79, 100, 129, 153, 163, 25, 55, 70, 101,
            122, 144, 185, 193, 35, 63, 93, 123, 160, 186, 198, 241, 57, 88, 115, 153, 164, 203,
            237, 12, 62, 103, 122, 167, 213, 236, 12, 47, 79, 121, 153, 198, 209, 16, 50, 81, 93,
            126, 174, 210, 253, 37, 69, 114,
        ];

        let result = idct_8bit(&dequantize(&coefficients, &table).unwrap());

        for (index, (&actual, &reference)) in result.iter().zip(&expected).enumerate() {
            assert!(
                actual.abs_diff(reference) <= 1,
                "sample {index}: got {actual}, expected {reference} ±1"
            );
        }
    }

    #[test]
    fn reconstructs_first_luminance_block_from_wpt_jpeg() {
        // Expected black samples were independently checked with libjpeg.
        // This tests a WPT asset, not the original browser test.
        let bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/images/arrow-oriented-upright.jpg"
        ));

        let frame = read_frame_header(bytes).unwrap();
        let scan = read_first_scan_header(bytes).unwrap();
        let huffman = read_huffman_tables(bytes).unwrap();
        let quantization = read_quantization_tables(bytes).unwrap();

        let first = scan.components[0];

        let component = frame
            .components
            .iter()
            .find(|item| item.id == first.id)
            .unwrap();

        let dc =
            HuffmanDecoder::new(huffman.dc[first.dc_table as usize].as_ref().unwrap()).unwrap();

        let ac =
            HuffmanDecoder::new(huffman.ac[first.ac_table as usize].as_ref().unwrap()).unwrap();

        let mut reader = EntropyReader::new(&bytes[scan.entropy_offset..]);

        let block = decode_baseline_block(&mut reader, &dc, &ac, 0).unwrap();

        let table = quantization[component.quantization_table as usize]
            .as_ref()
            .unwrap();

        let samples = idct_8bit(&dequantize(&block, &table).unwrap());

        assert_eq!(samples, [0; 64]);
    }
}
