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

fn append_chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());

    let start = png.len();

    png.extend_from_slice(kind);
    png.extend_from_slice(data);

    let crc = super::super::super::super::update_crc(0xffff_ffff, &png[start..]) ^ 0xffff_ffff;

    png.extend_from_slice(&crc.to_be_bytes());
}

fn make_png(width: usize, height: usize, color: u8, depth: u8, filtered: &[u8]) -> Vec<u8> {
    use std::io::Write;

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();

    header.extend_from_slice(&(width as u32).to_be_bytes());
    header.extend_from_slice(&(height as u32).to_be_bytes());

    // Compression 0, filter method 0, Adam7 interlace.
    header.extend_from_slice(&[depth, color, 0, 0, 1]);

    append_chunk(&mut png, b"IHDR", &header);

    if color == 3 {
        let mut palette = Vec::new();

        for i in 0..(1u16 << depth) {
            let i = i as u8;

            palette.extend_from_slice(&[i, i.wrapping_mul(3), 255 - i]);
        }

        append_chunk(&mut png, b"PLTE", &palette);
        append_chunk(&mut png, b"tRNS", &[0, 127]);
    }

    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());

    encoder.write_all(filtered).unwrap();

    let compressed = encoder.finish().unwrap();
    let split = compressed.len() / 2;

    // Exercise a zlib stream split across IDAT chunks.
    append_chunk(&mut png, b"IDAT", &compressed[..split]);
    append_chunk(&mut png, b"IDAT", &compressed[split..]);
    append_chunk(&mut png, b"IEND", &[]);

    png
}

fn independent_rgba(png: &[u8]) -> Vec<u8> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png));

    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);

    let mut reader = decoder.read_info().unwrap();
    let mut buffer = vec![0; reader.output_buffer_size()];
    let frame = reader.next_frame(&mut buffer).unwrap();

    let bytes = &buffer[..frame.buffer_size()];
    let mut output = Vec::new();

    match frame.color_type {
        png::ColorType::Grayscale => {
            for &gray in bytes {
                output.extend_from_slice(&[gray, gray, gray, 255]);
            }
        }

        png::ColorType::GrayscaleAlpha => {
            for pixel in bytes.chunks_exact(2) {
                output.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
        }

        png::ColorType::Rgb => {
            for pixel in bytes.chunks_exact(3) {
                output.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
        }

        png::ColorType::Rgba => {
            output.extend_from_slice(bytes);
        }

        png::ColorType::Indexed => {
            panic!("palette not expanded");
        }
    }

    output
}

#[test]
fn decodes_adam7_pngs_against_independent_decoder() {
    // (PNG color type, bit depth, bits per pixel)
    let formats = [
        (0, 1, 1),
        (0, 2, 2),
        (0, 4, 4),
        (0, 8, 8),
        (0, 16, 16),
        (3, 1, 1),
        (3, 2, 2),
        (3, 4, 4),
        (3, 8, 8),
        (2, 8, 24),
        (2, 16, 48),
        (4, 8, 16),
        (4, 16, 32),
        (6, 8, 32),
        (6, 16, 64),
    ];

    for (width, height) in [(1, 1), (2, 5), (7, 9), (17, 13)] {
        for (color, depth, bits) in formats {
            let (filtered, _) = reference(width, height, bits);

            let png = make_png(width, height, color, depth, &filtered);

            let expected = independent_rgba(&png);

            let decoded = crate::graphics::image::png_format::decode_rgba8(
                &png,
                crate::graphics::image::png_format::PngDecodeLimits::default(),
            )
            .unwrap_or_else(|error| {
                panic!(
                    "{width}x{height}, color={color}, \
                    depth={depth}: {error:?}"
                )
            });

            assert_eq!(
                (decoded.width, decoded.height),
                (width as u32, height as u32)
            );

            assert_eq!(
                decoded.pixels, expected,
                "{width}x{height}, color={color}, depth={depth}"
            );
        }
    }
}

#[test]
fn applies_filtered_limit_and_reports_bad_adam7_filter() {
    let (mut filtered, _) = reference(9, 9, 32);
    let png = make_png(9, 9, 6, 8, &filtered);

    let limits = crate::graphics::image::png_format::PngDecodeLimits {
        max_filtered_bytes: filtered.len() - 1,
        ..Default::default()
    };

    assert!(matches!(
        crate::graphics::image::png_format::decode_rgba8(&png, limits),
        Err(PngDecodeError::LimitExceeded)
    ));

    filtered[0] = 5;

    let png = make_png(9, 9, 6, 8, &filtered);

    assert!(matches!(
        crate::graphics::image::png_format::decode_rgba8(&png, Default::default()),
        Err(PngDecodeError::Adam7Filter {
            pass: 1,
            row: 0,
            ..
        })
    ));
}
