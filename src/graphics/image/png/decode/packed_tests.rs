use super::tests::insert_chunk;
use super::{PngDecodeError, PngDecodeLimits, decode_rgba8};

fn encode(
    depth: u8,
    indexed: bool,
    width: usize,
    height: usize,
    filter: png::FilterType,
) -> (Vec<u8>, Vec<u8>) {
    let max = (1u8 << depth) - 1;

    let mut packed = Vec::new();
    let mut expected = Vec::new();
    let mut palette = Vec::new();

    let row_bytes = (width * usize::from(depth)).div_ceil(8);

    for index in 0..=max {
        palette.extend_from_slice(&[index * 13, 255 - index * 7, index * 5]);
    }

    for y in 0..height {
        let start = packed.len();

        // Set unused bits to verify that row padding is ignored.
        packed.resize(start + row_bytes, 0xff);

        for x in 0..width {
            let sample = ((x + y * 3) % (usize::from(max) + 1)) as u8;

            let bit = x * usize::from(depth);
            let shift = 8 - usize::from(depth) - bit % 8;

            packed[start + bit / 8] &= !(max << shift);
            packed[start + bit / 8] |= sample << shift;

            if indexed {
                let position = usize::from(sample) * 3;

                expected.extend_from_slice(&palette[position..position + 3]);

                expected.push(match sample {
                    0 => 0,
                    1 => 127,
                    _ => 255,
                });
            } else {
                let gray = (u16::from(sample) * 255 / u16::from(max)) as u8;

                expected.extend_from_slice(&[gray, gray, gray, 255]);
            }
        }
    }

    let mut output = Vec::new();

    {
        let mut encoder = png::Encoder::new(&mut output, width as u32, height as u32);

        encoder.set_color(if indexed {
            png::ColorType::Indexed
        } else {
            png::ColorType::Grayscale
        });

        encoder.set_depth(match depth {
            1 => png::BitDepth::One,
            2 => png::BitDepth::Two,
            4 => png::BitDepth::Four,
            _ => unreachable!(),
        });

        encoder.set_filter(filter);

        if indexed {
            encoder.set_palette(palette);
            encoder.set_trns(vec![0, 127]);
        }

        encoder
            .write_header()
            .unwrap()
            .write_image_data(&packed)
            .unwrap();
    }

    (output, expected)
}

#[test]
fn decodes_packed_rows_for_all_filters() {
    for depth in [1, 2, 4] {
        for indexed in [false, true] {
            for width in [1, 3, 5, 9, 13] {
                for filter in [
                    png::FilterType::NoFilter,
                    png::FilterType::Sub,
                    png::FilterType::Up,
                    png::FilterType::Avg,
                    png::FilterType::Paeth,
                ] {
                    let (png, expected) = encode(depth, indexed, width, 3, filter);

                    let image = decode_rgba8(&png, PngDecodeLimits::default()).unwrap();

                    assert_eq!(
                        image.pixels, expected,
                        "depth={depth}, indexed={indexed}, \
                         width={width}, filter={filter:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn masks_high_bits_of_packed_grayscale_transparency() {
    let (mut png, _) = encode(2, false, 5, 1, png::FilterType::NoFilter);

    insert_chunk(&mut png, b"IDAT", b"tRNS", &[0xff, 0xfe]);

    let image = decode_rgba8(&png, PngDecodeLimits::default()).unwrap();

    assert_eq!(
        image.pixels,
        [
            0, 0, 0, 255, 85, 85, 85, 255, 170, 170, 170, 0, 255, 255, 255, 255, 0, 0, 0, 255
        ]
    );
}

#[test]
fn rejects_palette_larger_than_bit_depth_allows() {
    let (mut png, _) = encode(1, false, 1, 1, png::FilterType::NoFilter);

    // Change IHDR to indexed color and update the CRC.
    png[25] = 3;

    let crc = super::super::update_crc(0xffff_ffff, &png[12..29]) ^ 0xffff_ffff;

    png[29..33].copy_from_slice(&crc.to_be_bytes());

    insert_chunk(&mut png, b"IDAT", b"PLTE", &[0; 9]);

    assert!(matches!(
        decode_rgba8(&png, PngDecodeLimits::default()),
        Err(PngDecodeError::InvalidPalette)
    ));
}
