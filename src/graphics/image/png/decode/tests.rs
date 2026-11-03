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

fn insert_chunk(png: &mut Vec<u8>, before: &[u8; 4], kind: &[u8; 4], data: &[u8]) {
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

fn corrupt_crc(png: &mut [u8], kind: &[u8; 4]) {
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
fn rejects_other_formats_and_adam7() {
    let mut rgb = Vec::new();

    {
        let mut encoder = png::Encoder::new(&mut rgb, 1, 1);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);

        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[1, 2, 3])
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
