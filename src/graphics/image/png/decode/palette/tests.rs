use super::super::super::ChunkReader;
use super::super::tests::{corrupt_crc, insert_chunk};
use super::super::{PngDecodeError, PngDecodeLimits, decode_rgba8};

fn encode_indexed(
    width: u32,
    height: u32,
    indexes: &[u8],
    colors: &[u8],
    alpha: Option<&[u8]>,
    filter: png::FilterType,
) -> Vec<u8> {
    let mut data = Vec::new();

    {
        let mut encoder = png::Encoder::new(&mut data, width, height);

        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_palette(colors.to_vec());

        if let Some(alpha) = alpha {
            encoder.set_trns(alpha.to_vec());
        }

        encoder.set_filter(filter);
        encoder.set_compression(png::Compression::Default);

        encoder
            .write_header()
            .unwrap()
            .write_image_data(indexes)
            .unwrap();
    }

    data
}

fn remove_chunk(data: &mut Vec<u8>, kind: &[u8; 4]) {
    let mut reader = ChunkReader::new(data).unwrap();

    let range = loop {
        let chunk = reader.next_chunk().unwrap().unwrap();

        if &chunk.kind == kind {
            break chunk.offset..chunk.offset + 12 + chunk.data.len();
        }
    };

    data.drain(range);
}

#[test]
fn decodes_indexed8_and_short_alpha_table_for_all_filters() {
    let colors = [255, 0, 0, 0, 255, 0, 0, 0, 255, 200, 200, 100];

    let expected_colors = [
        [255, 0, 0, 0],
        [0, 255, 0, 127],
        [0, 0, 255, 255],
        [200, 200, 100, 255],
    ];

    for (width, height) in [(1u32, 3u32), (13, 4)] {
        let indexes: Vec<u8> = (0..width as usize * height as usize)
            .map(|i| (i % 4) as u8)
            .collect();

        let expected: Vec<u8> = indexes
            .iter()
            .flat_map(|&i| expected_colors[i as usize])
            .collect();

        for filter in [
            png::FilterType::NoFilter,
            png::FilterType::Sub,
            png::FilterType::Up,
            png::FilterType::Avg,
            png::FilterType::Paeth,
        ] {
            let png = encode_indexed(width, height, &indexes, &colors, Some(&[0, 127]), filter);

            let image = decode_rgba8(&png, PngDecodeLimits::default()).unwrap();

            assert_eq!((image.width, image.height), (width, height));
            assert_eq!(image.pixels, expected, "filter {filter:?}");
        }
    }
}

#[test]
fn recovers_out_of_range_palette_indices_as_opaque_black() {
    let png = encode_indexed(
        3,
        1,
        &[0, 1, 250],
        &[1, 2, 3, 4, 5, 6],
        Some(&[0]),
        png::FilterType::Sub,
    );

    let image = decode_rgba8(&png, PngDecodeLimits::default()).unwrap();

    assert_eq!(image.pixels, [1, 2, 3, 0, 4, 5, 6, 255, 0, 0, 0, 255]);
}

#[test]
fn handles_full_palette_and_no_trns() {
    let colors: Vec<u8> = (0..256).flat_map(|i| [i as u8, 0, 255]).collect();

    let png = encode_indexed(2, 1, &[0, 255], &colors, None, png::FilterType::NoFilter);

    assert_eq!(
        decode_rgba8(&png, PngDecodeLimits::default())
            .unwrap()
            .pixels,
        [0, 0, 255, 255, 255, 0, 255, 255]
    );

    let mut empty_alpha = png;
    insert_chunk(&mut empty_alpha, b"IDAT", b"tRNS", &[]);

    assert_eq!(
        decode_rgba8(&empty_alpha, PngDecodeLimits::default())
            .unwrap()
            .pixels,
        [0, 0, 255, 255, 255, 0, 255, 255]
    );
}

#[test]
fn rejects_missing_palette_and_invalid_indexed_trns() {
    let png = encode_indexed(1, 1, &[0], &[1, 2, 3, 4, 5, 6], None, png::FilterType::Sub);

    let mut missing = png.clone();
    remove_chunk(&mut missing, b"PLTE");

    assert!(matches!(
        decode_rgba8(&missing, PngDecodeLimits::default()),
        Err(PngDecodeError::MissingPalette)
    ));

    let mut early = png.clone();
    insert_chunk(&mut early, b"PLTE", b"tRNS", &[0]);

    assert!(matches!(
        decode_rgba8(&early, PngDecodeLimits::default()),
        Err(PngDecodeError::InvalidTransparencyChunk)
    ));

    let mut long = png.clone();
    insert_chunk(&mut long, b"IDAT", b"tRNS", &[0, 1, 2]);

    assert!(matches!(
        decode_rgba8(&long, PngDecodeLimits::default()),
        Err(PngDecodeError::InvalidTransparencyChunk)
    ));

    let mut duplicate = png.clone();
    insert_chunk(&mut duplicate, b"IDAT", b"tRNS", &[]);
    insert_chunk(&mut duplicate, b"IDAT", b"tRNS", &[]);

    assert!(matches!(
        decode_rgba8(&duplicate, PngDecodeLimits::default()),
        Err(PngDecodeError::InvalidTransparencyChunk)
    ));

    let mut late = png.clone();
    insert_chunk(&mut late, b"IEND", b"tRNS", &[0]);

    assert!(matches!(
        decode_rgba8(&late, PngDecodeLimits::default()),
        Err(PngDecodeError::InvalidTransparencyChunk)
    ));

    let mut corrupt = png;
    insert_chunk(&mut corrupt, b"IDAT", b"tRNS", &[0]);
    corrupt_crc(&mut corrupt, b"tRNS");

    assert!(matches!(
        decode_rgba8(&corrupt, PngDecodeLimits::default()),
        Err(PngDecodeError::TransparencyCrcMismatch { .. })
    ));
}

#[test]
fn rejects_corrupted_palette_and_unsupported_packed_depth() {
    let mut indexed = encode_indexed(1, 1, &[0], &[1, 2, 3], None, png::FilterType::Sub);

    corrupt_crc(&mut indexed, b"PLTE");

    assert!(matches!(
        decode_rgba8(&indexed, PngDecodeLimits::default()),
        Err(PngDecodeError::CriticalCrcMismatch { kind, .. })
            if kind == *b"PLTE"
    ));

    let mut png = Vec::new();

    {
        let mut encoder = png::Encoder::new(&mut png, 1, 1);

        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(png::BitDepth::One);
        encoder.set_palette(vec![0, 0, 0, 255, 255, 255]);

        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[0])
            .unwrap();
    }

    assert!(matches!(
        decode_rgba8(&png, PngDecodeLimits::default()),
        Err(PngDecodeError::UnsupportedFormat(_))
    ));
}
