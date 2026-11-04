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
    assert_eq!(&idat.data[..2], &[0x78, 0x01]);

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

fn idat_block_type(png: &[u8]) -> u8 {
    let mut reader = ChunkReader::new(png).unwrap();

    let _ihdr = reader.next_chunk().unwrap().unwrap();
    let idat = reader.next_chunk().unwrap().unwrap();

    assert_eq!(idat.kind, *b"IDAT");
    assert!(idat.crc_matches());

    (idat.data[2] >> 1) & 3
}

#[test]
fn chooses_compressed_for_repeated_pixels_and_stored_for_random() {
    let (width, height) = (128, 128);

    let repeated = [50, 100, 150, 120].repeat(width * height);

    let mut state = 0x1234_5678u32;

    let random: Vec<u8> = (0..width * height * 4)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;

            (state >> 24) as u8
        })
        .collect();

    for (pixels, expected_type) in [(&repeated, 1), (&random, 0)] {
        let png = encode_rgba8(width as u32, height as u32, pixels).unwrap();

        assert_eq!(idat_block_type(&png), expected_type);

        let decoded = decode_rgba8(&png, PngDecodeLimits::default()).unwrap();

        assert_eq!(decoded.pixels, *pixels);

        // Check the PNG with the existing independent decoder.
        let mut decoder = png::Decoder::new(png.as_slice()).read_info().unwrap();

        let mut buffer = vec![0; decoder.output_buffer_size()];
        let info = decoder.next_frame(&mut buffer).unwrap();

        assert_eq!(&buffer[..info.buffer_size()], pixels.as_slice());

        if expected_type == 1 {
            assert!(png.len() < pixels.len() / 10);
        }
    }
}

#[test]
fn compressed_candidate_stops_at_limit() {
    use std::io::Write;

    let mut candidate = CandidateWriter {
        bytes: Vec::new(),
        limit: 2,
        exceeded: false,
        allocation_failed: false,
    };

    candidate.write_all(&[1, 2]).unwrap();

    assert!(candidate.write_all(&[3]).is_err());
    assert!(candidate.exceeded);
    assert!(!candidate.allocation_failed);
    assert_eq!(candidate.bytes, [1, 2]);
}
