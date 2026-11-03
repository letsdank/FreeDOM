use super::super::super::super::PngFilterError;
use super::*;

// Pass numbers from the PNG specification. This table is independent
// of the start/step table used by the implementation.
const GRID: [[usize; 8]; 8] = [
    [1, 6, 4, 6, 2, 6, 4, 6],
    [7, 7, 7, 7, 7, 7, 7, 7],
    [5, 6, 5, 6, 5, 6, 5, 6],
    [7, 7, 7, 7, 7, 7, 7, 7],
    [3, 6, 4, 6, 3, 6, 4, 6],
    [7, 7, 7, 7, 7, 7, 7, 7],
    [5, 6, 5, 6, 5, 6, 5, 6],
    [7, 7, 7, 7, 7, 7, 7, 7],
];

fn sample(x: usize, y: usize, channel: usize) -> u8 {
    (x * 23 + y * 17 + channel * 31 + 7) as u8
}

fn raw_row(xs: &[usize], y: usize, bits: usize) -> Vec<u8> {
    let mut row = vec![0u8; (xs.len() * bits).div_ceil(8)];

    for (index, &x) in xs.iter().enumerate() {
        if bits < 8 {
            let shift = 8 - bits - (index * bits) % 8;
            let mask = (1u8 << bits) - 1;

            row[index * bits / 8] |= (sample(x, y, 0) & mask) << shift;
        } else {
            let bytes_per_pixel = bits / 8;

            for channel in 0..bytes_per_pixel {
                row[index * bytes_per_pixel + channel] = sample(x, y, channel);
            }
        }
    }

    row
}

fn reference(width: usize, height: usize, bits: usize) -> (Vec<u8>, Vec<u8>) {
    let expected: Vec<_> = (0..height)
        .flat_map(|y| raw_row(&(0..width).collect::<Vec<_>>(), y, bits))
        .collect();

    let mut filtered = Vec::new();

    for pass in 1..=7 {
        let mut previous: Option<Vec<u8>> = None;

        for y in 0..height {
            let xs: Vec<_> = (0..width).filter(|x| GRID[y % 8][x % 8] == pass).collect();

            if xs.is_empty() {
                continue;
            }

            let row = raw_row(&xs, y, bits);

            // Alternate Sub and Up filters across passes and rows.
            let filter = if (pass + y) % 2 == 0 { 1 } else { 2 };

            filtered.push(filter);

            let bytes_per_pixel = bits.div_ceil(8);

            for i in 0..row.len() {
                let predictor = if filter == 1 {
                    if i >= bytes_per_pixel {
                        row[i - bytes_per_pixel]
                    } else {
                        0
                    }
                } else {
                    previous.as_ref().map_or(0, |row| row[i])
                };

                filtered.push(row[i].wrapping_sub(predictor));
            }

            previous = Some(row);
        }
    }

    (filtered, expected)
}

#[test]
fn reconstructs_packed_and_multibyte_pixels() {
    for width in [1, 2, 5, 8, 9, 17] {
        for height in [1, 2, 5, 8, 9, 17] {
            for bits in [1, 2, 4, 8, 16, 24, 32, 48, 64] {
                let (mut filtered, expected) = reference(width, height, bits);

                // Verify that the function initializes the output buffer.
                let mut output = vec![0xff; expected.len()];

                reconstruct(width, height, bits, &mut filtered, &mut output).unwrap();

                assert_eq!(output, expected, "{width}x{height}, {bits} bits per pixel");
            }
        }
    }
}

#[test]
fn validates_buffer_lengths_before_writing() {
    let (mut filtered, expected) = reference(8, 8, 32);
    let mut output = vec![0xa5; expected.len()];

    assert!(matches!(
        reconstruct(8, 8, 32, &mut filtered[..1], &mut output),
        Err(PngDecodeError::ScanlineLength { .. })
    ));

    assert!(output.iter().all(|byte| *byte == 0xa5));

    let len = output.len();

    assert!(matches!(
        reconstruct(8, 8, 32, &mut filtered, &mut output[..len - 1]),
        Err(PngDecodeError::Adam7OutputLength { .. })
    ));

    assert!(output.iter().all(|byte| *byte == 0xa5));
}

#[test]
fn reports_invalid_filter_with_pass_and_row() {
    let (mut filtered, expected) = reference(8, 8, 8);
    let mut output = vec![0; expected.len()];

    filtered[0] = 5;

    assert!(matches!(
        reconstruct(8, 8, 8, &mut filtered, &mut output),
        Err(PngDecodeError::Adam7Filter {
            pass: 1,
            row: 0,
            source: PngFilterError::UnsupportedFilter(5)
        })
    ));
}
