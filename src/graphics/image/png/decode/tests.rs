use std::io::Write;

use super::*;

use crate::graphics::{
    image::encode_png,
    raster::{Color, Surface},
};

fn append_chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());

    let start = png.len();

    png.extend_from_slice(kind);
    png.extend_from_slice(data);

    let crc = super::super::update_crc(0xffff_ffff, &png[start..]) ^ 0xffff_ffff;

    png.extend_from_slice(&crc.to_be_bytes());
}

pub(super) fn insert_chunk(png: &mut Vec<u8>, before: &[u8; 4], kind: &[u8; 4], data: &[u8]) {
    let mut reader = ChunkReader::new(png).unwrap();

    let offset = loop {
        let chunk = reader.next_chunk().unwrap().unwrap();

        if &chunk.kind == before {
            break chunk.offset;
        }
    };

    let mut chunk = Vec::new();
    append_chunk(&mut chunk, kind, data);

    png.splice(offset..offset, chunk);
}

fn png_with_filtered_data(filtered: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());

    encoder.write_all(filtered).unwrap();

    let compressed = encoder.finish().unwrap();
    let mut png = super::super::SIGNATURE.to_vec();

    png.extend_from_slice(&[
        0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15,
        0xc4, 0x89,
    ]);

    append_chunk(&mut png, b"IDAT", &compressed);
    append_chunk(&mut png, b"IEND", &[]);

    png
}

fn encode_with_filter(filter: png::FilterType) -> (Vec<u8>, Vec<u8>) {
    let pixels: Vec<u8> = (0..4 * 13 * 4).map(|i| (i * 37 + 11) as u8).collect();

    let mut png = Vec::new();

    {
        let mut encoder = png::Encoder::new(&mut png, 13, 4);

        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_filter(filter);
        encoder.set_compression(png::Compression::Default);

        encoder
            .write_header()
            .unwrap()
            .write_image_data(&pixels)
            .unwrap();
    }

    (png, pixels)
}

#[test]
fn decodes_rgba8_with_each_filter() {
    for filter in [
        png::FilterType::NoFilter,
        png::FilterType::Sub,
        png::FilterType::Up,
        png::FilterType::Avg,
        png::FilterType::Paeth,
    ] {
        let (png, pixels) = encode_with_filter(filter);

        let image = decode_rgba8(&png, PngDecodeLimits::default()).unwrap();

        assert_eq!((image.width, image.height), (13, 4));
        assert_eq!(image.pixels, pixels, "filter {filter:?}");
    }
}

#[test]
fn preserves_transparent_rgba_samples_from_existing_encoder() {
    let mut surface = Surface::new(2, 1, Color::TRANSPARENT);

    surface.set_pixel(0, 0, Color::rgba(230, 13, 45, 0));
    surface.set_pixel(1, 0, Color::rgba(10, 20, 30, 128));

    let png = encode_png(&surface).unwrap();

    let image = decode_rgba8(&png, PngDecodeLimits::default()).unwrap();

    assert_eq!(image.pixels, [230, 13, 45, 0, 10, 20, 30, 128]);
}

pub(super) fn corrupt_crc(png: &mut [u8], kind: &[u8; 4]) {
    let mut reader = ChunkReader::new(png).unwrap();

    let position = loop {
        let chunk = reader.next_chunk().unwrap().unwrap();

        if &chunk.kind == kind {
            break chunk.offset + 8 + chunk.data.len();
        }
    };

    png[position] ^= 1;
}

#[test]
fn rejects_bad_critical_crcs() {
    for kind in [*b"IDAT", *b"IEND"] {
        let (mut png, _) = encode_with_filter(png::FilterType::Sub);
        corrupt_crc(&mut png, &kind);

        assert!(matches!(
            decode_rgba8(&png, PngDecodeLimits::default()),
            Err(PngDecodeError::CriticalCrcMismatch {
                kind: found,
                ..
            }) if found == kind
        ));
    }
}

#[test]
fn enforces_all_limits() {
    let (png, _) = encode_with_filter(png::FilterType::Sub);
    let defaults = PngDecodeLimits::default();

    for limits in [
        PngDecodeLimits {
            max_width: 12,
            ..defaults
        },
        PngDecodeLimits {
            max_height: 3,
            ..defaults
        },
        PngDecodeLimits {
            max_pixels: 51,
            ..defaults
        },
        PngDecodeLimits {
            max_filtered_bytes: 211,
            ..defaults
        },
    ] {
        assert!(matches!(
            decode_rgba8(&png, limits),
            Err(PngDecodeError::LimitExceeded)
        ));
    }
}

#[test]
fn rejects_unsupported_depth_and_adam7() {
    let mut rgb = Vec::new();

    {
        let mut encoder = png::Encoder::new(&mut rgb, 1, 1);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Sixteen);

        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[0, 1, 0, 2, 0, 3])
            .unwrap();
    }

    assert!(matches!(
        decode_rgba8(&rgb, PngDecodeLimits::default()),
        Err(PngDecodeError::UnsupportedFormat(_))
    ));

    let (mut adam7, _) = encode_with_filter(png::FilterType::Sub);

    // Change IHDR's interlace method and update its CRC.
    adam7[28] = 1;

    let crc = super::super::update_crc(0xffff_ffff, &adam7[12..29]) ^ 0xffff_ffff;

    adam7[29..33].copy_from_slice(&crc.to_be_bytes());

    assert!(matches!(
        decode_rgba8(&adam7, PngDecodeLimits::default()),
        Err(PngDecodeError::UnsupportedFormat(_))
    ));
}

#[test]
fn handles_optional_palette_unknown_chunks_and_trns() {
    let (png, expected) = encode_with_filter(png::FilterType::Sub);

    let mut with_palette = png.clone();
    insert_chunk(&mut with_palette, b"IDAT", b"PLTE", &[1, 2, 3]);

    assert_eq!(
        decode_rgba8(&with_palette, PngDecodeLimits::default())
            .unwrap()
            .pixels,
        expected
    );

    insert_chunk(&mut with_palette, b"IDAT", b"PLTE", &[1, 2, 3]);

    assert!(matches!(
        decode_rgba8(&with_palette, PngDecodeLimits::default()),
        Err(PngDecodeError::InvalidPalette)
    ));

    let mut late_palette = png.clone();
    insert_chunk(&mut late_palette, b"IEND", b"PLTE", &[1, 2, 3]);

    assert!(matches!(
        decode_rgba8(&late_palette, PngDecodeLimits::default()),
        Err(PngDecodeError::InvalidPalette)
    ));

    let mut invalid_palette = png.clone();
    insert_chunk(&mut invalid_palette, b"IDAT", b"PLTE", &[1, 2]);

    assert!(matches!(
        decode_rgba8(&invalid_palette, PngDecodeLimits::default()),
        Err(PngDecodeError::InvalidPalette)
    ));

    let mut critical = png.clone();
    insert_chunk(&mut critical, b"IDAT", b"ABCD", &[]);

    assert!(matches!(
        decode_rgba8(&critical, PngDecodeLimits::default()),
        Err(PngDecodeError::UnknownCriticalChunk(kind))
            if kind == *b"ABCD"
    ));

    let mut ancillary = png.clone();
    insert_chunk(&mut ancillary, b"IDAT", b"aaAA", &[]);

    assert_eq!(
        decode_rgba8(&ancillary, PngDecodeLimits::default())
            .unwrap()
            .pixels,
        expected
    );

    let mut trns = png;
    insert_chunk(&mut trns, b"IDAT", b"tRNS", &[0, 0]);

    assert!(matches!(
        decode_rgba8(&trns, PngDecodeLimits::default()),
        Err(PngDecodeError::InvalidTransparencyChunk)
    ));
}

#[test]
fn rejects_invalid_filter_and_scanline_lengths() {
    let invalid_filter = png_with_filtered_data(&[5, 0, 0, 0, 0]);

    assert!(matches!(
        decode_rgba8(&invalid_filter, PngDecodeLimits::default()),
        Err(PngDecodeError::Filter {
            row: 0,
            source: PngFilterError::UnsupportedFilter(5),
        })
    ));

    let short = png_with_filtered_data(&[0, 0, 0, 0]);

    assert!(matches!(
        decode_rgba8(&short, PngDecodeLimits::default()),
        Err(PngDecodeError::ScanlineLength {
            expected: 5,
            actual: 4,
        })
    ));

    let long = png_with_filtered_data(&[0, 0, 0, 0, 0, 0]);

    assert!(matches!(
        decode_rgba8(&long, PngDecodeLimits::default()),
        Err(PngDecodeError::Inflate(_))
    ));
}

fn encode_rgb(width: u32, height: u32, pixels: &[u8], filter: png::FilterType) -> Vec<u8> {
    let mut output = Vec::new();

    {
        let mut encoder = png::Encoder::new(&mut output, width, height);

        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_filter(filter);
        encoder.set_compression(png::Compression::Default);

        encoder
            .write_header()
            .unwrap()
            .write_image_data(pixels)
            .unwrap();
    }

    output
}

#[test]
fn decodes_rgb8_for_all_filters_and_single_pixel_rows() {
    for (width, height) in [(1u32, 3u32), (13, 4)] {
        let pixels: Vec<u8> = (0..width as usize * height as usize * 3)
            .map(|i| (i * 37 + 11) as u8)
            .collect();

        let expected: Vec<u8> = pixels
            .chunks_exact(3)
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
            .collect();

        for filter in [
            png::FilterType::NoFilter,
            png::FilterType::Sub,
            png::FilterType::Up,
            png::FilterType::Avg,
            png::FilterType::Paeth,
        ] {
            let png = encode_rgb(width, height, &pixels, filter);

            let image = decode_rgba8(&png, PngDecodeLimits::default()).unwrap();

            assert_eq!((image.width, image.height), (width, height));
            assert_eq!(image.pixels, expected, "filter {filter:?}");
        }
    }
}

#[test]
fn rgb_trns_matches_low_bits_and_preserves_transparent_rgb() {
    // Inspired by WPT png/trns-chunk.html; this fixture is locally generated.
    let mut png = encode_rgb(
        3,
        1,
        &[255, 0, 0, 254, 0, 0, 255, 0, 1],
        png::FilterType::Sub,
    );

    insert_chunk(
        &mut png,
        b"IDAT",
        b"tRNS",
        &[0xff, 0xff, 0xff, 0x00, 0xff, 0x00],
    );

    let image = decode_rgba8(&png, PngDecodeLimits::default()).unwrap();

    assert_eq!(image.pixels, [255, 0, 0, 0, 254, 0, 0, 255, 255, 0, 1, 255]);
}

#[test]
fn rejects_invalid_rgb_trns_and_bad_crc() {
    let png = encode_rgb(1, 1, &[1, 2, 3], png::FilterType::Sub);

    let trns = [0, 1, 0, 2, 0, 3];

    let mut short = png.clone();
    insert_chunk(&mut short, b"IDAT", b"tRNS", &[0, 1]);

    assert!(matches!(
        decode_rgba8(&short, PngDecodeLimits::default()),
        Err(PngDecodeError::InvalidTransparencyChunk)
    ));

    let mut duplicate = png.clone();

    insert_chunk(&mut duplicate, b"IDAT", b"tRNS", &trns);
    insert_chunk(&mut duplicate, b"IDAT", b"tRNS", &trns);

    assert!(matches!(
        decode_rgba8(&duplicate, PngDecodeLimits::default()),
        Err(PngDecodeError::InvalidTransparencyChunk)
    ));

    let mut late = png.clone();
    insert_chunk(&mut late, b"IEND", b"tRNS", &trns);

    assert!(matches!(
        decode_rgba8(&late, PngDecodeLimits::default()),
        Err(PngDecodeError::InvalidTransparencyChunk)
    ));

    let mut bad_crc = png.clone();
    insert_chunk(&mut bad_crc, b"IDAT", b"tRNS", &trns);
    corrupt_crc(&mut bad_crc, b"tRNS");

    assert!(matches!(
        decode_rgba8(&bad_crc, PngDecodeLimits::default()),
        Err(PngDecodeError::TransparencyCrcMismatch { .. })
    ));

    let mut palette_after = png;

    insert_chunk(&mut palette_after, b"IDAT", b"tRNS", &trns);
    insert_chunk(&mut palette_after, b"IDAT", b"PLTE", &[1, 2, 3]);

    assert!(matches!(
        decode_rgba8(&palette_after, PngDecodeLimits::default()),
        Err(PngDecodeError::InvalidPalette)
    ));
}

#[test]
fn wpt_fixture_trns_high_bits_are_ignored() {
    // Fixture:
    // https://github.com/web-platform-tests/wpt/blob/master/png/support/trns-high-bits-set.png
    //
    // Original browser test:
    // https://github.com/web-platform-tests/wpt/blob/master/png/trns-chunk.html
    //
    // This checks source alpha only; it does not run the browser/canvas test.
    let png = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/wpt/png/support/trns-high-bits-set.png"
    ));

    let image = decode_rgba8(png, PngDecodeLimits::default()).unwrap();

    assert_eq!((image.width, image.height), (100, 50));
    assert_eq!(image.pixels.len(), 100 * 50 * 4);

    let offset = (5usize * image.width as usize + 5) * 4;
    let alpha = image.pixels[offset + 3];

    assert!(
        alpha <= 2,
        "WPT fixture alpha at (5, 5): expected 0 ± 2, got {alpha}"
    );
}

fn encode_gray(
    width: u32,
    height: u32,
    pixels: &[u8],
    alpha: bool,
    filter: png::FilterType,
) -> Vec<u8> {
    let mut png = Vec::new();

    {
        let mut encoder = png::Encoder::new(&mut png, width, height);

        encoder.set_color(if alpha {
            png::ColorType::GrayscaleAlpha
        } else {
            png::ColorType::Grayscale
        });

        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_filter(filter);
        encoder.set_compression(png::Compression::Default);

        encoder
            .write_header()
            .unwrap()
            .write_image_data(pixels)
            .unwrap();
    }

    png
}

#[test]
fn decodes_grayscale_and_gray_alpha_with_all_filters() {
    for (width, height) in [(1u32, 3u32), (13, 4)] {
        for alpha in [false, true] {
            let mut pixels = Vec::new();
            let mut expected = Vec::new();

            for i in 0..width as usize * height as usize {
                let gray = (i * 37 + 11) as u8;
                let a = if alpha { (i * 23) as u8 } else { 255 };

                pixels.push(gray);

                if alpha {
                    pixels.push(a);
                }

                expected.extend_from_slice(&[gray, gray, gray, a]);
            }

            for filter in [
                png::FilterType::NoFilter,
                png::FilterType::Sub,
                png::FilterType::Up,
                png::FilterType::Avg,
                png::FilterType::Paeth,
            ] {
                let png = encode_gray(width, height, &pixels, alpha, filter);
                let image = decode_rgba8(&png, PngDecodeLimits::default()).unwrap();

                assert_eq!((image.width, image.height), (width, height));
                assert_eq!(image.pixels, expected, "alpha={alpha}, filter={filter:?}");
            }
        }
    }
}

#[test]
fn grayscale_trns_uses_low_bits_and_preserves_samples() {
    let mut png = encode_gray(3, 1, &[0, 128, 255], false, png::FilterType::Sub);

    insert_chunk(&mut png, b"IDAT", b"tRNS", &[0xff, 0x80]);

    let image = decode_rgba8(&png, PngDecodeLimits::default()).unwrap();

    assert_eq!(
        image.pixels,
        [0, 0, 0, 255, 128, 128, 128, 0, 255, 255, 255, 255]
    );
}

#[test]
fn rejects_invalid_grayscale_transparency_and_palette() {
    let gray = encode_gray(1, 1, &[42], false, png::FilterType::Sub);

    let mut short = gray.clone();
    insert_chunk(&mut short, b"IDAT", b"tRNS", &[42]);

    assert!(matches!(
        decode_rgba8(&short, PngDecodeLimits::default()),
        Err(PngDecodeError::InvalidTransparencyChunk)
    ));

    let mut duplicate = gray.clone();

    insert_chunk(&mut duplicate, b"IDAT", b"tRNS", &[0, 42]);
    insert_chunk(&mut duplicate, b"IDAT", b"tRNS", &[0, 42]);

    assert!(matches!(
        decode_rgba8(&duplicate, PngDecodeLimits::default()),
        Err(PngDecodeError::InvalidTransparencyChunk)
    ));

    let mut late = gray.clone();
    insert_chunk(&mut late, b"IEND", b"tRNS", &[0, 42]);

    assert!(matches!(
        decode_rgba8(&late, PngDecodeLimits::default()),
        Err(PngDecodeError::InvalidTransparencyChunk)
    ));

    let mut bad_crc = gray.clone();
    insert_chunk(&mut bad_crc, b"IDAT", b"tRNS", &[0, 42]);
    corrupt_crc(&mut bad_crc, b"tRNS");

    assert!(matches!(
        decode_rgba8(&bad_crc, PngDecodeLimits::default()),
        Err(PngDecodeError::TransparencyCrcMismatch { .. })
    ));

    let mut palette = gray;
    insert_chunk(&mut palette, b"IDAT", b"PLTE", &[1, 2, 3]);

    assert!(matches!(
        decode_rgba8(&palette, PngDecodeLimits::default()),
        Err(PngDecodeError::InvalidPalette)
    ));

    let mut gray_alpha = encode_gray(1, 1, &[42, 128], true, png::FilterType::Sub);

    insert_chunk(&mut gray_alpha, b"IDAT", b"tRNS", &[0, 42]);

    assert!(matches!(
        decode_rgba8(&gray_alpha, PngDecodeLimits::default()),
        Err(PngDecodeError::InvalidTransparencyChunk)
    ));

    let mut gray_alpha = encode_gray(1, 1, &[42, 128], true, png::FilterType::Sub);

    insert_chunk(&mut gray_alpha, b"IDAT", b"PLTE", &[1, 2, 3]);

    assert!(matches!(
        decode_rgba8(&gray_alpha, PngDecodeLimits::default()),
        Err(PngDecodeError::InvalidPalette)
    ));
}
