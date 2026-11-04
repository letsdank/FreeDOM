//! PNG filter method 0 scanline reconstruction.
//!
//! PNG Third Edition, section 9:
//! <https://www.w3.org/TR/png-3/#9Filtering>

use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PngFilterError {
    UnsupportedFilter(u8),
    InvalidBytesPerPixel,
    PreviousRowLength { expected: usize, actual: usize },
    OutputRowLength { expected: usize, actual: usize },
}

impl fmt::Display for PngFilterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid PNG scanline filter: {self:?}")
    }
}

impl Error for PngFilterError {}

fn validate_filter(
    filter: u8,
    row_len: usize,
    previous: Option<&[u8]>,
    bytes_per_pixel: usize,
) -> Result<(), PngFilterError> {
    if filter > 4 {
        return Err(PngFilterError::UnsupportedFilter(filter));
    }

    if bytes_per_pixel == 0 {
        return Err(PngFilterError::InvalidBytesPerPixel);
    }

    if let Some(previous) = previous {
        if previous.len() != row_len {
            return Err(PngFilterError::PreviousRowLength {
                expected: row_len,
                actual: previous.len(),
            });
        }
    }

    Ok(())
}

fn predictor(filter: u8, left: u8, above: u8, upper_left: u8) -> u8 {
    match filter {
        0 => 0,
        1 => left,
        2 => above,
        3 => ((u16::from(left) + u16::from(above)) / 2) as u8,
        4 => paeth(left, above, upper_left),
        _ => unreachable!(),
    }
}

/// Reconstructs a filtered scanline in place, without the filter-type byte.
///
/// `previous`, when present, is the reconstructed previous row of the same
/// image or Adam7 pass. `bytes_per_pixel` is the byte distance to the previous
/// pixel, rounded up to at least one for packed samples.
///
/// Invalid arguments are rejected before the row is modified. Pixel unpacking
/// is not performed.
pub fn unfilter_row(
    filter: u8,
    row: &mut [u8],
    previous: Option<&[u8]>,
    bytes_per_pixel: usize,
) -> Result<(), PngFilterError> {
    validate_filter(filter, row.len(), previous, bytes_per_pixel)?;

    for i in 0..row.len() {
        let left = if i >= bytes_per_pixel {
            row[i - bytes_per_pixel]
        } else {
            0
        };

        let above = previous.map_or(0, |prev| prev[i]);

        let upper_left = if i >= bytes_per_pixel {
            previous.map_or(0, |prev| prev[i - bytes_per_pixel])
        } else {
            0
        };

        row[i] = row[i].wrapping_add(predictor(filter, left, above, upper_left));
    }

    Ok(())
}

/// Filters a raw scanline into `output`, excluding the filter-type byte.
///
/// `previous` is an unfiltered row. Invalid arguments are rejected before
/// modifying `output`.
pub fn filter_row(
    filter: u8,
    row: &[u8],
    previous: Option<&[u8]>,
    bytes_per_pixel: usize,
    output: &mut [u8],
) -> Result<(), PngFilterError> {
    validate_filter(filter, row.len(), previous, bytes_per_pixel)?;

    if output.len() != row.len() {
        return Err(PngFilterError::OutputRowLength {
            expected: row.len(),
            actual: output.len(),
        });
    }

    for i in 0..row.len() {
        let left = if i >= bytes_per_pixel {
            row[i - bytes_per_pixel]
        } else {
            0
        };

        let above = previous.map_or(0, |prev| prev[i]);

        let upper_left = if i >= bytes_per_pixel {
            previous.map_or(0, |prev| prev[i - bytes_per_pixel])
        } else {
            0
        };

        output[i] = row[i].wrapping_sub(predictor(filter, left, above, upper_left));
    }

    Ok(())
}

fn paeth(left: u8, above: u8, upper_left: u8) -> u8 {
    let a = i32::from(left);
    let b = i32::from(above);
    let c = i32::from(upper_left);

    let prediction = a + b - c;

    let pa = (prediction - a).abs();
    let pb = (prediction - b).abs();
    let pc = (prediction - c).abs();

    if pa <= pb && pa <= pc {
        left
    } else if pb <= pc {
        above
    } else {
        upper_left
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::graphics::{
        image::{
            encode_png,
            png_format::{IdatReader, decode_zlib},
        },
        raster::{Color, Surface},
    };

    #[test]
    fn reconstructs_all_five_filters() {
        let previous = [3, 5, 7];

        let cases = [
            (0, [10, 20, 30]),
            (1, [10, 10, 10]),
            (2, [7, 15, 23]),
            (3, [9, 13, 17]),
            (4, [7, 10, 10]),
        ];

        for (filter, mut row) in cases {
            unfilter_row(filter, &mut row, Some(&previous), 1).unwrap();

            assert_eq!(row, [10, 20, 30], "filter {filter}");
        }
    }

    #[test]
    fn handles_first_row_pixel_stride_and_wrapping() {
        let mut row = [10, 20, 20, 20];

        unfilter_row(1, &mut row, None, 2).unwrap();
        assert_eq!(row, [10, 20, 30, 40]);

        let mut row = [250, 20];

        unfilter_row(1, &mut row, None, 1).unwrap();
        assert_eq!(row, [250, 14]);

        let mut row = [10, 15, 20];

        unfilter_row(3, &mut row, None, 1).unwrap();
        assert_eq!(row, [10, 20, 30]);

        let mut row = [10, 10, 10];

        unfilter_row(4, &mut row, None, 1).unwrap();
        assert_eq!(row, [10, 20, 30]);
    }

    #[test]
    fn paeth_uses_specified_tie_order() {
        assert_eq!(paeth(0, 3, 2), 0);
        assert_eq!(paeth(0, 3, 1), 3);
        assert_eq!(paeth(255, 0, 255), 0);
    }

    #[test]
    fn rejects_invalid_arguments_without_modifying_row() {
        let mut row = [7, 8];

        assert_eq!(
            unfilter_row(5, &mut row, None, 1),
            Err(PngFilterError::UnsupportedFilter(5))
        );

        assert_eq!(
            unfilter_row(0, &mut row, None, 0),
            Err(PngFilterError::InvalidBytesPerPixel)
        );

        assert_eq!(
            unfilter_row(0, &mut row, Some(&[1]), 1),
            Err(PngFilterError::PreviousRowLength {
                expected: 2,
                actual: 1
            })
        );

        assert_eq!(row, [7, 8]);
    }

    #[test]
    fn reconstructs_pixels_from_existing_encoder_via_out_zlib_decoder() {
        let (width, height) = (17, 9);
        let mut surface = Surface::new(width, height, Color::TRANSPARENT);

        for y in 0..height {
            for x in 0..width {
                surface.set_pixel(
                    x,
                    y,
                    Color::rgba(
                        (x * 11) as u8,
                        (y * 23) as u8,
                        (x ^ y) as u8,
                        (x * 7 + y) as u8,
                    ),
                );
            }
        }

        let png = encode_png(&surface).unwrap();
        let idat = IdatReader::new(&png).unwrap();

        let row_bytes = width * 4;
        let expected_len = (row_bytes + 1) * height;

        let mut filtered = decode_zlib(idat, Vec::new(), expected_len).unwrap().output;

        assert_eq!(filtered.len(), expected_len);

        let mut previous = vec![0; row_bytes];

        for (y, line) in filtered.chunks_exact_mut(row_bytes + 1).enumerate() {
            let (&mut filter, row) = line.split_first_mut().unwrap();

            let prev = if y == 0 {
                None
            } else {
                Some(previous.as_slice())
            };

            unfilter_row(filter, row, prev, 4).unwrap();

            for (x, rgba) in row.chunks_exact(4).enumerate() {
                let color = surface.pixels()[y * width + x];

                assert_eq!(
                    rgba,
                    [color.red, color.green, color.blue, color.alpha],
                    "pixel ({x}, {y})"
                );
            }

            previous.copy_from_slice(row);
        }
    }

    #[test]
    fn matches_independent_encoder_for_all_filters_and_sample_depths() {
        let filters = [
            png::FilterType::NoFilter,
            png::FilterType::Sub,
            png::FilterType::Up,
            png::FilterType::Avg,
            png::FilterType::Paeth,
        ];

        let formats = [
            // Color type, bit depth, width, row bytes, filter stride.
            (
                png::ColorType::Rgba,
                png::BitDepth::Sixteen,
                3u32,
                24usize,
                8usize,
            ),
            (
                png::ColorType::Grayscale,
                png::BitDepth::One,
                13u32,
                2usize,
                1usize,
            ),
        ];

        for (color, depth, width, row_bytes, bpp) in formats {
            let raw: Vec<u8> = (0..row_bytes * 3).map(|i| (i * 37 + 11) as u8).collect();

            for filter in filters {
                let mut png_data = Vec::new();

                {
                    let mut encoder = png::Encoder::new(&mut png_data, width, 3);

                    encoder.set_color(color);
                    encoder.set_depth(depth);
                    encoder.set_filter(filter);
                    encoder.set_compression(png::Compression::Default);

                    let mut writer = encoder.write_header().unwrap();
                    writer.write_image_data(&raw).unwrap();
                }

                let idat = IdatReader::new(&png_data).unwrap();

                let mut bytes = decode_zlib(idat, Vec::new(), (row_bytes + 1) * 3)
                    .unwrap()
                    .output;

                assert_eq!(bytes.len(), (row_bytes + 1) * 3);

                let mut previous = vec![0; row_bytes];

                for (y, line) in bytes.chunks_exact_mut(row_bytes + 1).enumerate() {
                    let (&mut kind, row) = line.split_first_mut().unwrap();

                    assert_eq!(kind, filter as u8);

                    let prev = if y == 0 {
                        None
                    } else {
                        Some(previous.as_slice())
                    };

                    unfilter_row(kind, row, prev, bpp).unwrap();

                    assert_eq!(row, &raw[y * row_bytes..(y + 1) * row_bytes]);

                    previous.copy_from_slice(row);
                }
            }
        }
    }

    #[test]
    fn filters_known_vectors() {
        let row = [10, 20, 30];
        let previous = [3, 5, 7];

        let cases = [
            (0, [10, 20, 30]),
            (1, [10, 10, 10]),
            (2, [7, 15, 23]),
            (3, [9, 13, 17]),
            (4, [7, 10, 10]),
        ];

        for (filter, expected) in cases {
            let mut output = [0; 3];

            filter_row(filter, &row, Some(&previous), 1, &mut output).unwrap();

            assert_eq!(output, expected, "filter {filter}");
        }
    }

    #[test]
    fn filter_and_unfilter_roundtrip() {
        for len in [0, 1, 2, 7, 32, 129] {
            let row: Vec<u8> = (0..len).map(|i| (i * 173 + 250) as u8).collect();

            let previous_row: Vec<u8> = (0..len).map(|i| (i * 59 + 33) as u8).collect();

            for bytes_per_pixel in [1, 2, 3, 4, 8] {
                for filter in 0..=4 {
                    for previous in [None, Some(previous_row.as_slice())] {
                        let mut result = vec![0; len];

                        filter_row(filter, &row, previous, bytes_per_pixel, &mut result).unwrap();
                        unfilter_row(filter, &mut result, previous, bytes_per_pixel).unwrap();

                        assert_eq!(
                            result, row,
                            "len={len}, bpp={bytes_per_pixel}, filter={filter}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn filter_rejects_invalid_inputs_without_modifying_output() {
        let row = [1, 2];
        let mut output = [0xaa; 2];

        assert_eq!(
            filter_row(5, &row, None, 1, &mut output),
            Err(PngFilterError::UnsupportedFilter(5))
        );

        assert_eq!(
            filter_row(0, &row, None, 0, &mut output),
            Err(PngFilterError::InvalidBytesPerPixel)
        );

        assert_eq!(
            filter_row(0, &row, Some(&[1]), 1, &mut output),
            Err(PngFilterError::PreviousRowLength {
                expected: 2,
                actual: 1
            })
        );

        assert_eq!(output, [0xaa; 2]);

        let mut short = [0xbb; 1];

        assert_eq!(
            filter_row(0, &row, None, 1, &mut short),
            Err(PngFilterError::OutputRowLength {
                expected: 2,
                actual: 1
            })
        );

        assert_eq!(short, [0xbb]);
    }
}
