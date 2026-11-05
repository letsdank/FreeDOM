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
fn decodes_rgb16_and_rejects_invalid_adam7_data() {
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

    let image = decode_rgba8(&rgb, PngDecodeLimits::default()).unwrap();

    assert_eq!(image.pixels, [0, 0, 0, 255]);

    let (mut adam7, _) = encode_with_filter(png::FilterType::Sub);

    // Change IHDR's interlace method and update its CRC.
    adam7[28] = 1;

    let crc = super::super::update_crc(0xffff_ffff, &adam7[12..29]) ^ 0xffff_ffff;

    adam7[29..33].copy_from_slice(&crc.to_be_bytes());

    assert!(decode_rgba8(&adam7, PngDecodeLimits::default()).is_err());
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

#[test]
fn decodes_wpt_pngsuite_interlaced_palette_image() {
    let interlaced = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/wpt/css/css-ui/support/PTS/basi3p02.png"
    ));
    let non_interlaced = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/wpt/css/css-ui/support/PTS/basn3p02.png"
    ));

    let interlaced_header = IdatReader::new(interlaced).unwrap().structure().header;
    let plain_header = IdatReader::new(non_interlaced).unwrap().structure().header;

    assert_eq!(interlaced_header.interlace, InterlaceMethod::Adam7);
    assert_eq!(plain_header.interlace, InterlaceMethod::None);
    assert_eq!(interlaced_header.bit_depth, 2);
    assert_eq!(interlaced_header.color_type, ColorType::Indexed);

    let actual = decode_rgba8(interlaced, PngDecodeLimits::default()).unwrap();
    let expected = decode_rgba8(non_interlaced, PngDecodeLimits::default()).unwrap();

    assert_eq!((actual.width, actual.height), (32, 32));
    assert_eq!((expected.width, expected.height), (32, 32));
    assert_eq!(actual.pixels.len(), 32 * 32 * 4);

    assert_eq!(actual.pixels, expected.pixels);

    let known_pixels = [
        (0, 0, [0, 0, 255, 255]),
        (4, 0, [255, 0, 0, 255]),
        (8, 0, [255, 255, 0, 255]),
        (12, 0, [0, 255, 0, 255]),
        (0, 4, [0, 255, 0, 255]),
        (4, 4, [0, 0, 255, 255]),
        (16, 16, [0, 0, 255, 255]),
        (31, 31, [0, 0, 255, 255]),
    ];

    for (x, y, color) in known_pixels {
        let offset = (y * 32 + x) * 4;

        assert_eq!(
            &actual.pixels[offset..offset + 4],
            &color,
            "unexpected pixel at ({x}, {y})"
        );
    }
}

#[test]
fn ignores_unknown_ancillary_chunks_in_wpt_images() {
    // WPT browser reftests:
    // https://github.com/web-platform-tests/wpt/blob/master/png/errors/unknown-ancillary-error-recovery.html
    // https://github.com/web-platform-tests/wpt/blob/master/png/errors/unknown-ancillary-error-recovery-2.html

    let reference = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/wpt/png/errors/support/no-invalid-chunks.png"
    ));

    let expected = decode_rgba8(reference, PngDecodeLimits::default())
        .expect("failed to decode WPT reference PNG");

    assert!(
        expected.pixels.chunks_exact(4).any(|pixel| pixel[3] != 0),
        "WPT reference PNG must contain visible pixels"
    );

    let cases: [(&str, &[u8]); 2] = [
        (
            "unknown ancillary chunk before IDAT",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/testdata/wpt/png/errors/support/invalid-unknown-ancillary.png"
            )),
        ),
        (
            "unknown ancillary chunk after IDAT",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/testdata/wpt/png/errors/support/invalid-unknown-ancillary-after-IDAT.png"
            )),
        ),
    ];

    for (description, input) in cases {
        let actual = decode_rgba8(input, PngDecodeLimits::default())
            .unwrap_or_else(|error| panic!("{description}: {error}"));

        assert_eq!(
            (actual.width, actual.height),
            (expected.width, expected.height),
            "{description}: image dimensions differ"
        );

        assert_eq!(
            actual.pixels, expected.pixels,
            "{description}: decoded pixels differ"
        );
    }
}

#[test]
fn exposes_cicp_without_changing_pixels() {
    let surface = Surface::new(1, 1, Color::rgba(51, 102, 153, 255));
    let mut png = encode_png(&surface).unwrap();

    // Display P3 primaries and transfer function, RGB, full range.
    insert_chunk(&mut png, b"IDAT", b"cICP", &[12, 13, 0, 1]);

    let expected = Some(Cicp {
        color_primaries: 12,
        transfer_function: 13,
        matrix_coefficients: 0,
        video_full_range_flag: 1,
    });

    let image8 = decode_rgba8(&png, PngDecodeLimits::default()).unwrap();
    let image16 = decode_rgba16_be(&png, PngDecodeLimits::default()).unwrap();

    assert_eq!(image8.cicp, expected);
    assert_eq!(image16.cicp, expected);

    // Parsing metadata alone must not transform the samples.
    assert_eq!(image8.pixels, [51, 102, 153, 255]);
    assert_eq!(image16.pixels, [51, 51, 102, 102, 153, 153, 255, 255]);
}

#[test]
fn ignores_invalid_duplicate_late_and_corrupt_cicp() {
    let surface = Surface::new(1, 1, Color::rgba(1, 2, 3, 255));
    let original = encode_png(&surface).unwrap();

    for data in [&[1, 2, 0][..], &[1, 2, 1, 1][..], &[1, 2, 0, 2][..]] {
        let mut png = original.clone();

        insert_chunk(&mut png, b"IDAT", b"cICP", data);

        let image = decode_rgba8(&png, PngDecodeLimits::default()).unwrap();

        assert_eq!(image.cicp, None);
        assert_eq!(image.pixels, [1, 2, 3, 255]);
    }

    let mut duplicate = original.clone();

    insert_chunk(&mut duplicate, b"IDAT", b"cICP", &[12, 13, 0, 1]);
    insert_chunk(&mut duplicate, b"IDAT", b"cICP", &[9, 16, 0, 1]);

    assert_eq!(
        decode_rgba8(&duplicate, PngDecodeLimits::default())
            .unwrap()
            .cicp,
        None
    );

    let mut late = original.clone();

    insert_chunk(&mut late, b"IEND", b"cICP", &[12, 13, 0, 1]);

    assert_eq!(
        decode_rgba8(&late, PngDecodeLimits::default())
            .unwrap()
            .cicp,
        None
    );

    let mut after_palette = original.clone();

    insert_chunk(&mut after_palette, b"IDAT", b"PLTE", &[1, 2, 3]);
    insert_chunk(&mut after_palette, b"IDAT", b"cICP", &[12, 13, 0, 1]);

    assert_eq!(
        decode_rgba8(&after_palette, PngDecodeLimits::default())
            .unwrap()
            .cicp,
        None
    );

    let mut corrupt = original.clone();

    insert_chunk(&mut corrupt, b"IDAT", b"cICP", &[12, 13, 0, 1]);
    corrupt_crc(&mut corrupt, b"cICP");

    assert_eq!(
        decode_rgba8(&corrupt, PngDecodeLimits::default())
            .unwrap()
            .cicp,
        None
    );

    assert_eq!(
        decode_rgba8(&original, PngDecodeLimits::default())
            .unwrap()
            .cicp,
        None
    );
}

#[test]
fn keeps_unknown_cicp_code_points_without_claiming_support() {
    let surface = Surface::new(1, 1, Color::rgba(1, 2, 3, 255));
    let mut png = encode_png(&surface).unwrap();

    insert_chunk(&mut png, b"IDAT", b"cICP", &[250, 251, 0, 1]);

    let image = decode_rgba8(&png, PngDecodeLimits::default()).unwrap();

    assert_eq!(
        image.cicp,
        Some(Cicp {
            color_primaries: 250,
            transfer_function: 251,
            matrix_coefficients: 0,
            video_full_range_flag: 1,
        })
    );
}

#[test]
fn reads_actl_from_wpt_apng_default_image() {
    // This checks only the declaration and default image, not APNG playback.
    let png = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/wpt/png/apng/support/033.png"
    ));

    let control = Some(AnimationControl {
        num_frames: 2,
        num_plays: 1,
    });

    let image8 = decode_rgba8(png, PngDecodeLimits::default()).unwrap();
    let image16 = decode_rgba16_be(png, PngDecodeLimits::default()).unwrap();

    assert_eq!(image8.animation_control, control);
    assert_eq!(image16.animation_control, control);
    assert_eq!((image8.width, image8.height), (128, 64));

    assert!(
        image8
            .pixels
            .chunks_exact(4)
            .all(|pixel| pixel == [255, 0, 0, 255])
    );
}

#[test]
fn invalid_actl_does_not_prevent_default_image_decoding() {
    let surface = Surface::new(1, 1, Color::rgba(1, 2, 3, 255));
    let original = encode_png(&surface).unwrap();

    let good = [0, 0, 0, 2, 0, 0, 0, 0];
    let mut valid = original.clone();

    insert_chunk(&mut valid, b"IDAT", b"acTL", &good);

    assert_eq!(
        decode_rgba8(&valid, PngDecodeLimits::default())
            .unwrap()
            .animation_control,
        Some(AnimationControl {
            num_frames: 2,
            num_plays: 0,
        })
    );

    let cases: &[&[u8]] = &[
        &[],
        &[0; 7],
        &[0; 8],
        &[0, 0, 0, 1, 0, 0, 0, 0, 0],
        &[0x80, 0, 0, 0, 0, 0, 0, 0],
        &[0, 0, 0, 1, 0x80, 0, 0, 0],
    ];

    for &data in cases {
        let mut png = original.clone();

        insert_chunk(&mut png, b"IDAT", b"acTL", data);

        let decoded = decode_rgba8(&png, PngDecodeLimits::default()).unwrap();

        assert_eq!(decoded.animation_control, None, "data={data:?}");
        assert_eq!(decoded.pixels, [1, 2, 3, 255]);
    }

    let mut duplicate = valid.clone();
    insert_chunk(&mut duplicate, b"IDAT", b"acTL", &good);

    let mut late = original.clone();
    insert_chunk(&mut late, b"IEND", b"acTL", &good);

    let mut corrupt = valid;
    corrupt_crc(&mut corrupt, b"acTL");

    for png in [&duplicate, &late, &corrupt] {
        let decoded = decode_rgba8(png, PngDecodeLimits::default()).unwrap();

        assert_eq!(decoded.animation_control, None);
        assert_eq!(decoded.pixels, [1, 2, 3, 255]);
    }
}
