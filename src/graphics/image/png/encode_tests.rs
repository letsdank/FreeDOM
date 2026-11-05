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

#[test]
fn scores_signed_residual_bytes() {
    assert_eq!(filter_score(&[0, 1, 127, 128, 255]), 257);
}

#[test]
fn adaptive_filtering_roundtrips_and_improves_a_gradient() {
    use crate::graphics::image::png_format::{IdatReader, decode_zlib, encode_zlib_fixed_lz77};

    let (width, height) = (128usize, 64usize);

    let row: Vec<u8> = (0..width)
        .flat_map(|x| [x as u8, (x * 2) as u8, (255 - x) as u8, 255])
        .collect();

    let pixels = row.repeat(height);

    let png = encode_rgba8(width as u32, height as u32, &pixels).unwrap();
    let idat = IdatReader::new(&png).unwrap();
    let stride = width * 4 + 1;

    let filtered = decode_zlib(idat, Vec::new(), stride * height)
        .unwrap()
        .output;

    // The second row is identical to the first, so Up produces zeros.
    assert_eq!(filtered[stride], 2);

    let decoded = decode_rgba8(&png, PngDecodeLimits::default()).unwrap();

    assert_eq!(decoded.pixels, pixels);

    let mut decoder = png::Decoder::new(png.as_slice()).read_info().unwrap();
    let mut buffer = vec![0; decoder.output_buffer_size()];
    let info = decoder.next_frame(&mut buffer).unwrap();

    assert_eq!(&buffer[..info.buffer_size()], pixels.as_slice());

    let mut none = Vec::new();

    for _ in 0..height {
        none.push(0);
        none.extend_from_slice(&row);
    }

    let mut unfiltered_zlib = Vec::new();
    encode_zlib_fixed_lz77(&none, &mut unfiltered_zlib).unwrap();

    // The PNG consists of 57 framing bytes plus the IDAT payload.
    assert!(png.len() - 57 < unfiltered_zlib.len());
}

#[test]
fn encodes_rgba16_big_endian_without_losing_sample_bits() {
    for (width, height) in [(1u32, 1u32), (7, 5)] {
        let samples: Vec<u16> = (0..width as usize * height as usize * 4)
            .map(|i| (i as u16).wrapping_mul(7919).wrapping_add(0x1234))
            .collect();

        let bytes: Vec<u8> = samples
            .iter()
            .flat_map(|value| value.to_be_bytes())
            .collect();

        let encoded = encode_rgba16_be(width, height, &bytes).unwrap();

        // The independent decoder must return every original byte.
        let mut reader = png::Decoder::new(encoded.as_slice()).read_info().unwrap();

        let mut output = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut output).unwrap();

        assert_eq!(
            (info.width, info.height, info.color_type, info.bit_depth),
            (width, height, png::ColorType::Rgba, png::BitDepth::Sixteen)
        );

        assert_eq!(&output[..info.buffer_size()], bytes.as_slice());

        // Our current decoder returns RGBA8, keeping the high byte.
        let image = decode_rgba8(&encoded, PngDecodeLimits::default()).unwrap();

        let expected: Vec<u8> = samples.iter().map(|&sample| (sample >> 8) as u8).collect();

        assert_eq!(image.pixels, expected);
    }
}

#[test]
fn rejects_invalid_rgba16_input() {
    assert!(matches!(
        encode_rgba16_be(0, 1, &[]),
        Err(PngEncodeError::InvalidDimensions)
    ));

    assert!(matches!(
        encode_rgba16_be(1, 1, &[0; 7]),
        Err(PngEncodeError::PixelLength {
            expected: 8,
            actual: 7
        })
    ));

    assert!(matches!(
        encode_rgba16_be(1, 1, &[0; 9]),
        Err(PngEncodeError::PixelLength {
            expected: 8,
            actual: 9
        })
    ));
}

#[test]
fn encodes_cicp_and_roundtrips_rgba8_and_rgba16() {
    let cicp = Cicp {
        color_primaries: 12,
        transfer_function: 13,
        matrix_coefficients: 0,
        video_full_range_flag: 1,
    };

    let samples16: [u16; 8] = [
        0x3301, 0x6602, 0x9903, 0x1234, 0x33ff, 0x66ff, 0x99ff, 0x12ff,
    ];

    let bytes16: Vec<u8> = samples16.into_iter().flat_map(u16::to_be_bytes).collect();

    let bytes8 = [51, 102, 153, 18, 51, 102, 133, 18];

    let encoded8 = encode_rgba8_with_cicp(2, 1, &bytes8, cicp).unwrap();
    let encoded16 = encode_rgba16_be_with_cicp(2, 1, &bytes16, cicp).unwrap();

    for png in [&encoded8, &encoded16] {
        let mut reader = ChunkReader::new(&png).unwrap();

        assert_eq!(reader.next_chunk().unwrap().unwrap().kind, *b"IHDR");

        let chunk = reader.next_chunk().unwrap().unwrap();

        assert_eq!(chunk.kind, *b"cICP");
        assert_eq!(chunk.data, [12, 13, 0, 1]);
        assert!(chunk.crc_matches());

        assert_eq!(reader.next_chunk().unwrap().unwrap().kind, *b"IDAT");
    }

    let decoded8 = decode_rgba8(&encoded8, PngDecodeLimits::default()).unwrap();

    assert_eq!(decoded8.cicp, Some(cicp));
    assert_eq!(decoded8.pixels, bytes8);

    let decoded16 = super::super::decode_rgba16_be(&encoded16, PngDecodeLimits::default()).unwrap();

    assert_eq!(decoded16.cicp, Some(cicp));
    assert_eq!(decoded16.pixels, bytes16);

    // Check the samples with an independent decoder.
    let mut independent = png::Decoder::new(encoded16.as_slice()).read_info().unwrap();

    let mut output = vec![0; independent.output_buffer_size()];
    let info = independent.next_frame(&mut output).unwrap();

    assert_eq!(
        (info.color_type, info.bit_depth),
        (png::ColorType::Rgba, png::BitDepth::Sixteen)
    );

    assert_eq!(&output[..info.buffer_size()], bytes16.as_slice());
}

#[test]
fn rejects_invalid_cicp_fields() {
    let mut cicp = Cicp {
        color_primaries: 12,
        transfer_function: 13,
        matrix_coefficients: 1,
        video_full_range_flag: 1,
    };

    assert!(matches!(
        encode_rgba8_with_cicp(1, 1, &[0; 4], cicp),
        Err(PngEncodeError::InvalidCicp)
    ));

    cicp.matrix_coefficients = 0;
    cicp.video_full_range_flag = 2;

    assert!(matches!(
        encode_rgba16_be_with_cicp(1, 1, &[0; 8], cicp),
        Err(PngEncodeError::InvalidCicp)
    ));
}

#[test]
fn reencodes_wpt_display_p3_without_losing_surface_color() {
    // This checks a WPT fixture, not the original Canvas-based test.
    let wpt = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/wpt/png/support/cicp-display-p3.png"
    ));

    let decoded = super::super::decode_rgba16_be(wpt, PngDecodeLimits::default()).unwrap();

    let encoded = encode_rgba16_be_with_cicp(
        decoded.width,
        decoded.height,
        &decoded.pixels,
        decoded.cicp.unwrap(),
    )
    .unwrap();

    let original = crate::graphics::image::decode_png(wpt).unwrap();
    let result = crate::graphics::image::decode_png(&encoded).unwrap();

    assert_eq!(
        (result.width(), result.height()),
        (original.width(), original.height())
    );

    assert_eq!(result.pixels(), original.pixels());
}
