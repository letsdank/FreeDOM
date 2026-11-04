use super::*;

use crate::graphics::image::png_format::{
    ChunkReader, ColorType, Ihdr, InterlaceMethod, PngDecodeLimits, decode_rgba8,
};

#[test]
fn encodes_rgba8_against_independent_decoder() {
    for (width, height) in [(1u32, 1u32), (5, 7), (257, 65)] {
        let input: Vec<u8> = (0..width as usize * height as usize * 4)
            .map(|i| (i * 37 + i / 17) as u8)
            .collect();

        let png = encode_rgba8(width, height, &input).unwrap();
        let actual = decode_rgba8(&png, PngDecodeLimits::default()).unwrap();

        assert_eq!((actual.width, actual.height), (width, height));
        assert_eq!(actual.pixels, input);

        // Check the encoded file with the existing independent decoder.
        let mut reader = png::Decoder::new(png.as_slice()).read_info().unwrap();

        let mut output = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut output).unwrap();

        assert_eq!(
            (info.width, info.height, info.color_type, info.bit_depth),
            (width, height, png::ColorType::Rgba, png::BitDepth::Eight)
        );

        assert_eq!(&output[..info.buffer_size()], input.as_slice());
    }
}

#[test]
fn writes_required_chunks_with_valid_crc() {
    let png = encode_rgba8(1, 1, &[1, 2, 3, 0]).unwrap();
    let mut reader = ChunkReader::new(&png).unwrap();

    let ihdr = reader.next_chunk().unwrap().unwrap();

    assert_eq!(ihdr.kind, *b"IHDR");
    assert!(ihdr.crc_matches());

    let parsed = Ihdr::from_chunk(&ihdr).unwrap();

    assert_eq!(
        (
            parsed.width,
            parsed.height,
            parsed.bit_depth,
            parsed.color_type,
            parsed.interlace
        ),
        (1, 1, 8, ColorType::TruecolorAlpha, InterlaceMethod::None)
    );

    let idat = reader.next_chunk().unwrap().unwrap();

    assert_eq!(idat.kind, *b"IDAT");
    assert!(idat.crc_matches());
    assert_eq!(&idat.data[..3], &[0x78, 0x01, 0x01]);

    let iend = reader.next_chunk().unwrap().unwrap();

    assert_eq!(iend.kind, *b"IEND");
    assert!(iend.data.is_empty());
    assert!(iend.crc_matches());

    assert!(reader.next_chunk().unwrap().is_none());
}

#[test]
fn rejects_invalid_dimensions_and_pixel_lengths() {
    for (width, height) in [(0, 1), (1, 0), (0x8000_0000, 1), (1, 0x8000_0000)] {
        assert!(matches!(
            encode_rgba8(width, height, &[]),
            Err(PngEncodeError::InvalidDimensions)
        ));
    }

    assert!(matches!(
        encode_rgba8(1, 1, &[1, 2, 3]),
        Err(PngEncodeError::PixelLength {
            expected: 4,
            actual: 3
        })
    ));

    assert!(matches!(
        encode_rgba8(1, 1, &[0; 5]),
        Err(PngEncodeError::PixelLength {
            expected: 4,
            actual: 5
        })
    ));
}
