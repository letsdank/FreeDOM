use crate::graphics::raster::{Color, Surface};
use std::{error::Error, fmt};

#[path = "image/png/mod.rs"]
pub mod png_format;

#[path = "image/color.rs"]
mod color;

const MAX_IMAGE_DIMENSION: usize = 4_096;
const MAX_IMAGE_PIXELS: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageError {
    message: String,
}

impl ImageError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ImageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid image: {}", self.message)
    }
}

impl Error for ImageError {}

/// Decodes the PNG default image from IDAT.
pub fn decode_png(encoded: &[u8]) -> Result<Surface, ImageError> {
    decode_png_impl(encoded, false)
}

/// Decides the first animation frame when APNG is valid; otherwise uses IDAT.
pub fn decode_png_for_display(encoded: &[u8]) -> Result<Surface, ImageError> {
    decode_png_impl(encoded, true)
}

fn decode_png_impl(encoded: &[u8], animated_first_frame: bool) -> Result<Surface, ImageError> {
    use png_format::{
        AnimationLimits, AnimationPlayer, Cicp, PlaybackUpdate, PngDecodeLimits, decode_rgba8,
        decode_rgba16_be, read_ihdr,
    };

    let limits = PngDecodeLimits {
        max_width: MAX_IMAGE_DIMENSION as u32,
        max_height: MAX_IMAGE_DIMENSION as u32,
        max_pixels: MAX_IMAGE_PIXELS,

        // Account for 16-bit RGBA samples and Adam7 row overhead.
        max_filtered_bytes: MAX_IMAGE_PIXELS * 8 + MAX_IMAGE_DIMENSION * 14,
    };

    let header = read_ihdr(encoded).map_err(|error| ImageError::new(error.to_string()))?;

    let mut sixteen_bit = header.bit_depth == 16;

    let (width, height, mut pixels, cicp, animation_control) = if sixteen_bit {
        let image = decode_rgba16_be(encoded, limits)
            .map_err(|error| ImageError::new(error.to_string()))?;

        (
            image.width as usize,
            image.height as usize,
            image.pixels,
            image.cicp,
            image.animation_control,
        )
    } else {
        let image =
            decode_rgba8(encoded, limits).map_err(|error| ImageError::new(error.to_string()))?;

        (
            image.width as usize,
            image.height as usize,
            image.pixels,
            image.cicp,
            image.animation_control,
        )
    };

    if animated_first_frame && animation_control.is_some() {
        if let Ok(mut player) = AnimationPlayer::new(encoded, AnimationLimits::default(), limits) {
            if matches!(
                player.advance(std::time::Duration::ZERO),
                Ok(PlaybackUpdate::FrameChanged)
            ) {
                pixels = player.canvas().pixels().to_vec();
                sixteen_bit = false;
            }
        }
    }

    let display_p3 = matches!(
        cicp,
        Some(Cicp {
            color_primaries: 12,
            transfer_function: 13,
            matrix_coefficients: 0,
            video_full_range_flag: 1,
        })
    );

    let mut surface = Surface::new(width, height, Color::TRANSPARENT);
    let bytes_per_pixel = if sixteen_bit { 8 } else { 4 };

    for (index, pixel) in pixels.chunks_exact(bytes_per_pixel).enumerate() {
        let pixel = if sixteen_bit {
            if display_p3 {
                color::display_p3_16_to_srgb([
                    u16::from_be_bytes([pixel[0], pixel[1]]),
                    u16::from_be_bytes([pixel[2], pixel[3]]),
                    u16::from_be_bytes([pixel[4], pixel[5]]),
                    u16::from_be_bytes([pixel[6], pixel[7]]),
                ])
            } else {
                // Preserve the existing RGBA16 -> RGBA8 behavior.
                [pixel[0], pixel[2], pixel[4], pixel[6]]
            }
        } else {
            let pixel = [pixel[0], pixel[1], pixel[2], pixel[3]];

            if display_p3 {
                color::display_p3_to_srgb(pixel)
            } else {
                pixel
            }
        };

        let color = Color::rgba(pixel[0], pixel[1], pixel[2], pixel[3]);

        surface.set_pixel(index % width, index / width, color);
    }

    Ok(surface)
}

pub fn encode_png(surface: &Surface) -> Result<Vec<u8>, ImageError> {
    let width = u32::try_from(surface.width())
        .map_err(|_| ImageError::new("image width exceeds PNG limits"))?;
    let height = u32::try_from(surface.height())
        .map_err(|_| ImageError::new("image height exceeds PNG limits"))?;

    let capacity = surface
        .width()
        .checked_mul(surface.height())
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| ImageError::new("image pixel data is too large"))?;

    let mut pixels = Vec::new();

    pixels
        .try_reserve_exact(capacity)
        .map_err(|_| ImageError::new("image pixel allocation failed"))?;

    for color in surface.pixels() {
        pixels.extend_from_slice(&[color.red, color.green, color.blue, color.alpha]);
    }

    png_format::encode_rgba8(width, height, &pixels)
        .map_err(|error| ImageError::new(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_and_decodes_rgba_pixels() {
        let mut source = Surface::new(2, 2, Color::TRANSPARENT);
        source.set_pixel(0, 0, Color::rgba(255, 0, 0, 255));
        source.set_pixel(1, 0, Color::rgba(0, 255, 0, 128));
        source.set_pixel(0, 1, Color::rgba(0, 0, 255, 64));
        source.set_pixel(1, 1, Color::rgba(255, 255, 255, 0));

        let encoded = encode_png(&source).unwrap();

        assert_eq!(&encoded[..8], b"\x89PNG\r\n\x1a\n");

        let decoded = decode_png(&encoded).unwrap();

        assert_eq!(decoded.width(), 2);
        assert_eq!(decoded.height(), 2);
        assert_eq!(decoded.pixel(0, 0), Some(Color::rgba(255, 0, 0, 255)));
        assert_eq!(decoded.pixel(1, 0), Some(Color::rgba(0, 255, 0, 128)));
        assert_eq!(decoded.pixel(0, 1), Some(Color::rgba(0, 0, 255, 64)));
        assert_eq!(decoded.pixel(1, 1), Some(Color::rgba(255, 255, 255, 0)));
    }

    #[test]
    fn rejects_non_png_data() {
        assert!(decode_png(b"FreeDOM is not a PNG").is_err());
    }

    #[test]
    fn renders_wpt_apng_default_image() {
        // This tests the static IDAT image, not animation playback.
        let png = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/png/apng/support/033.png"
        ));

        let image = decode_png(png).unwrap();

        assert_eq!((image.width(), image.height()), (128, 64));

        assert!(
            image
                .pixels()
                .iter()
                .all(|pixel| *pixel == Color::rgba(255, 0, 0, 255))
        );
    }

    #[test]
    fn decodes_interlaced_png_with_native_decoder() {
        let mut source = Surface::new(1, 1, Color::TRANSPARENT);
        source.set_pixel(0, 0, Color::rgba(42, 123, 207, 181));

        let mut encoded = encode_png(&source).unwrap();

        // For a 1x1 image Adam7 has a single non-empty pass.
        encoded[28] = 1;

        // CRC of the modified 1x1 RGBA8 IHDR.
        encoded[29..33].copy_from_slice(&0x6812_f41fu32.to_be_bytes());

        let native =
            png_format::decode_rgba8(&encoded, png_format::PngDecodeLimits::default()).unwrap();

        assert_eq!(native.pixels, [42, 123, 207, 181]);

        let decoded = decode_png(&encoded).unwrap();

        assert_eq!(decoded.pixel(0, 0), source.pixel(0, 0));
    }

    #[test]
    fn reports_native_critical_crc_error_without_fallback() {
        let source = Surface::new(1, 1, Color::TRANSPARENT);
        let mut encoded = encode_png(&source).unwrap();

        let mut reader = png_format::ChunkReader::new(&encoded).unwrap();

        let crc_position = loop {
            let chunk = reader.next_chunk().unwrap().unwrap();

            if chunk.kind == *b"IDAT" {
                break chunk.offset + 8 + chunk.data.len();
            }
        };

        encoded[crc_position] ^= 1;

        let error = decode_png(&encoded).unwrap_err();

        assert!(error.to_string().contains("CriticalCrcMismatch"));
    }

    #[test]
    fn image_encoder_writes_compressed_zlib_blocks() {
        let surface = Surface::new(2, 2, Color::rgba(50, 100, 150, 120));

        let encoded = encode_png(&surface).unwrap();
        let mut reader = png_format::ChunkReader::new(&encoded).unwrap();

        let _ihdr = reader.next_chunk().unwrap().unwrap();
        let idat = reader.next_chunk().unwrap().unwrap();

        assert_eq!(idat.kind, *b"IDAT");
        assert!(idat.crc_matches());

        // zlib header followed by a final fixed-Huffman DEFLATE block.
        assert_eq!(&idat.data[..2], &[0x78, 0x01]);
        assert_eq!(idat.data[2] & 0b111, 0b011);
    }

    #[test]
    fn decodes_wpt_ancillary_images_into_surface() {
        // This checks the image API, not the original HTML reftests.
        let reference = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/png/errors/support/no-invalid-chunks.png"
        ));

        let expected = decode_png(reference).unwrap();

        let cases: [(&str, &[u8]); 2] = [
            (
                "before IDAT",
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/testdata/wpt/png/errors/support/invalid-unknown-ancillary.png"
                )),
            ),
            (
                "after IDAT",
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/testdata/wpt/png/errors/support/invalid-unknown-ancillary-after-IDAT.png"
                )),
            ),
        ];

        for (description, input) in cases {
            let actual = decode_png(input).unwrap_or_else(|error| panic!("{description}: {error}"));

            assert_eq!(
                (actual.width(), actual.height()),
                (expected.width(), expected.height()),
                "{description}: dimensions differ"
            );

            assert_eq!(
                actual.pixels(),
                expected.pixels(),
                "{description}: pixels differ"
            );
        }
    }

    #[test]
    fn converts_only_supported_cicp_to_the_surface() {
        fn png_with_cicp(cicp: Option<[u8; 4]>) -> Vec<u8> {
            let mut bytes = Vec::new();

            {
                let mut encoder = png::Encoder::new(&mut bytes, 1, 1);

                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);

                let mut writer = encoder.write_header().unwrap();

                if let Some(cicp) = cicp {
                    writer
                        .write_chunk(png::chunk::ChunkType(*b"cICP"), &cicp)
                        .unwrap();
                }

                writer.write_image_data(&[51, 102, 153, 117]).unwrap();
            }

            bytes
        }

        let p3 = png_with_cicp(Some([12, 13, 0, 1]));

        // The low-level decoder preserves the encoded values.
        let raw = png_format::decode_rgba8(&p3, png_format::PngDecodeLimits::default()).unwrap();

        assert_eq!(raw.pixels, [51, 102, 153, 117]);

        // The image surface receives sRGB values.
        assert_eq!(
            decode_png(&p3).unwrap().pixel(0, 0),
            Some(Color::rgba(27, 104, 157, 117))
        );

        // Untagged and currently unsupported color encodings stay unchanged.
        for cicp in [None, Some([9, 16, 0, 1])] {
            let png = png_with_cicp(cicp);

            assert_eq!(
                decode_png(&png).unwrap().pixel(0, 0),
                Some(Color::rgba(51, 102, 153, 117))
            );
        }
    }

    #[test]
    fn converts_wpt_cicp_display_p3_image_to_srgb_surface() {
        // This checks the source image and expected pixel from the WPT.
        // It does not execute the original Canvas test.
        let png = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/png/support/cicp-display-p3.png"
        ));

        let image = decode_png(png).unwrap();
        let pixel = image.pixel(5, 5).unwrap();

        let actual = [pixel.red, pixel.green, pixel.blue, pixel.alpha];
        let expected = [27u8, 104, 157, 255];

        for (index, (actual, expected)) in actual.into_iter().zip(expected).enumerate() {
            assert!(
                actual.abs_diff(expected) <= 2,
                "channel {index}: expected approximately {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn converts_16bit_p3_before_quantizing_to_surface() {
        fn image(tagged: bool) -> Vec<u8> {
            let mut bytes = Vec::new();

            {
                let mut encoder = png::Encoder::new(&mut bytes, 2, 1);

                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Sixteen);

                let mut writer = encoder.write_header().unwrap();

                if tagged {
                    writer
                        .write_chunk(png::chunk::ChunkType(*b"cICP"), &[12, 13, 0, 1])
                        .unwrap();
                }

                // The two pixels have identical high bytes, but different low bytes.
                let samples = [
                    0x3300u16, 0x6600, 0x9900, 0x1234, 0x33ff, 0x66ff, 0x99ff, 0x12ff,
                ];

                let pixels: Vec<u8> = samples.into_iter().flat_map(u16::to_be_bytes).collect();

                writer.write_image_data(&pixels).unwrap();
            }

            bytes
        }

        let surface = decode_png(&image(true)).unwrap();

        assert_eq!(surface.pixel(0, 0), Some(Color::rgba(26, 103, 157, 18)));
        assert_eq!(surface.pixel(1, 0), Some(Color::rgba(28, 104, 158, 18)));

        // Untagged images retain the previous high-byte conversion.
        let surface = decode_png(&image(false)).unwrap();

        assert_eq!(surface.pixel(0, 0), Some(Color::rgba(51, 102, 153, 18)));
        assert_eq!(surface.pixel(1, 0), Some(Color::rgba(51, 102, 153, 18)));
    }

    #[test]
    fn displays_first_apng_frame_instead_of_the_default_image() {
        // This checks the WPT image fixture, not the timed browser reftest.
        let png = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/png/apng/support/033.png"
        ));

        let displayed = decode_png_for_display(png).unwrap();

        assert_eq!((displayed.width(), displayed.height()), (128, 64));

        assert!(
            displayed
                .pixels()
                .iter()
                .all(|pixel| *pixel == Color::rgba(0, 0, 0, 255))
        );

        let fallback = decode_png(png).unwrap();

        assert!(
            fallback
                .pixels()
                .iter()
                .all(|pixel| *pixel == Color::rgba(255, 0, 0, 255))
        );
    }

    #[test]
    fn displays_default_image_if_animation_frame_control_is_corrupt() {
        let mut png = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/png/apng/support/033.png"
        ))
        .to_vec();

        let offset = {
            let mut chunks = png_format::ChunkReader::new(&png).unwrap();

            loop {
                let chunk = chunks.next_chunk().unwrap().unwrap();

                if chunk.kind == *b"fcTL" {
                    break chunk.offset;
                }
            }
        };

        // Corrupt fcTL without updating its CRC.
        png[offset + 8] ^= 1;

        let image = decode_png_for_display(&png).unwrap();

        assert!(
            image
                .pixels()
                .iter()
                .all(|pixel| *pixel == Color::rgba(255, 0, 0, 255))
        );
    }

    #[test]
    fn displays_static_png_without_changes() {
        let png = png_format::encode_rgba8(1, 1, &[42, 123, 207, 181]).unwrap();

        let image = decode_png_for_display(&png).unwrap();

        assert_eq!(image.pixel(0, 0), Some(Color::rgba(42, 123, 207, 181)));
    }
}
