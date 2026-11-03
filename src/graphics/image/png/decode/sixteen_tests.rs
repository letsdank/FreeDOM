use super::tests::insert_chunk;
use super::{PngDecodeLimits, decode_rgba8};

fn encode(
    color: png::ColorType,
    samples: &[u16],
    width: u32,
    height: u32,
    filter: png::FilterType,
) -> Vec<u8> {
    let bytes: Vec<u8> = samples
        .iter()
        .flat_map(|sample| sample.to_be_bytes())
        .collect();

    let mut png = Vec::new();

    {
        let mut encoder = png::Encoder::new(&mut png, width, height);

        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Sixteen);
        encoder.set_filter(filter);

        encoder
            .write_header()
            .unwrap()
            .write_image_data(&bytes)
            .unwrap();
    }

    png
}

#[test]
fn decodes_all_sixteen_bit_color_types_and_filters() {
    for (color, channels) in [
        (png::ColorType::Grayscale, 1),
        (png::ColorType::GrayscaleAlpha, 2),
        (png::ColorType::Rgb, 3),
        (png::ColorType::Rgba, 4),
    ] {
        let samples: Vec<u16> = (0..5 * 3 * channels)
            .map(|i| ((i * 1877 + 291) % 65536) as u16)
            .collect();

        let mut expected = Vec::new();

        for pixel in samples.chunks_exact(channels) {
            let high = |value: u16| (value >> 8) as u8;

            match color {
                png::ColorType::Grayscale => {
                    let gray = high(pixel[0]);

                    expected.extend_from_slice(&[gray, gray, gray, 255]);
                }

                png::ColorType::GrayscaleAlpha => {
                    let gray = high(pixel[0]);

                    expected.extend_from_slice(&[gray, gray, gray, high(pixel[1])]);
                }

                png::ColorType::Rgb => {
                    expected.extend_from_slice(&[
                        high(pixel[0]),
                        high(pixel[1]),
                        high(pixel[2]),
                        255,
                    ]);
                }

                png::ColorType::Rgba => {
                    expected.extend_from_slice(&[
                        high(pixel[0]),
                        high(pixel[1]),
                        high(pixel[2]),
                        high(pixel[3]),
                    ]);
                }

                _ => unreachable!(),
            }
        }

        for filter in [
            png::FilterType::NoFilter,
            png::FilterType::Sub,
            png::FilterType::Up,
            png::FilterType::Avg,
            png::FilterType::Paeth,
        ] {
            let png = encode(color, &samples, 5, 3, filter);

            let image = decode_rgba8(&png, PngDecodeLimits::default()).unwrap();

            assert_eq!(image.pixels, expected, "color={color:?}, filter={filter:?}");
        }
    }
}

#[test]
fn compares_full_sixteen_bit_transparency_key() {
    let mut gray = encode(
        png::ColorType::Grayscale,
        &[0x1234, 0x1235, 0x1234],
        3,
        1,
        png::FilterType::Sub,
    );

    insert_chunk(&mut gray, b"IDAT", b"tRNS", &[0x12, 0x34]);

    let image = decode_rgba8(&gray, PngDecodeLimits::default()).unwrap();

    assert_eq!(
        image.pixels,
        [
            0x12, 0x12, 0x12, 0, 0x12, 0x12, 0x12, 255, 0x12, 0x12, 0x12, 0
        ]
    );

    let mut rgb = encode(
        png::ColorType::Rgb,
        &[0x1234, 0x5678, 0x9abc, 0x1235, 0x5678, 0x9abc],
        2,
        1,
        png::FilterType::Paeth,
    );

    insert_chunk(
        &mut rgb,
        b"IDAT",
        b"tRNS",
        &[0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc],
    );

    let image = decode_rgba8(&rgb, PngDecodeLimits::default()).unwrap();

    assert_eq!(image.pixels, [0x12, 0x56, 0x9a, 0, 0x12, 0x56, 0x9a, 255]);
}

#[test]
fn decodes_wpt_apng_16bit_default_image() {
    // This checks the IDAT default image, not APNG playback or the WPT reftest.
    let png = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/wpt/png/apng/support/033.png"
    ));

    let image = decode_rgba8(png, PngDecodeLimits::default()).unwrap();

    assert_eq!((image.width, image.height), (128, 64));
    assert_eq!(image.pixels.len(), 128 * 64 * 4);

    for (index, pixel) in image.pixels.chunks_exact(4).enumerate() {
        assert_eq!(pixel, [255, 0, 0, 255], "unexpected pixel at index {index}");
    }
}
