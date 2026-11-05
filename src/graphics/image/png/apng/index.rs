//! Borrowed APNG frame index.
//!
//! PNG Third Edition, section 4.9:
//! <https://www.w3.org/TR/png-3/#apng-frame-based-animation>

use super::super::{PngDecodeError, PngDecodeLimits, Rgba8Image, Rgba16BeImage};
use super::inspect::scan_animation;
use super::{AnimationError, AnimationLimits, AnimationSummary, FrameControl, FrameData};

/// Frame payloads borrow the original PNG; compressed bytes are not copied.
#[derive(Debug)]
pub enum FrameSource<'a> {
    /// The first animation frame uses the PNG's IDAT stream.
    DefaultImage,

    /// The frame uses these fdAT chunks, in file order.
    Fdat(Vec<FrameData<'a>>),
}

#[derive(Debug)]
pub struct IndexedFrame<'a> {
    pub control: FrameControl,
    pub source: FrameSource<'a>,
}

#[derive(Debug)]
pub struct AnimationIndex<'a> {
    input: &'a [u8],
    pub summary: AnimationSummary,
    pub frames: Vec<IndexedFrame<'a>>,
}

/// Indexes a PNG's animation, returning None for a static PNG.
///
/// This checks the same structural rules as `inspect_animation`, but does not
/// decompress frame data, validate all PNG CRCs or enforce an input-size limit.
pub fn index_animation<'a>(
    input: &'a [u8],
    limits: AnimationLimits,
) -> Result<Option<AnimationIndex<'a>>, AnimationError> {
    let mut frames = Vec::new();

    let Some(summary) = scan_animation(input, limits, Some(&mut frames))? else {
        return Ok(None);
    };

    Ok(Some(AnimationIndex {
        input,
        summary,
        frames,
    }))
}

impl AnimationIndex<'_> {
    /// Decodes one uncomposited frame, not the animation canvas.
    pub fn decode_frame_rgba8(
        &self,
        index: usize,
        limits: PngDecodeLimits,
    ) -> Result<Rgba8Image, PngDecodeError> {
        let frame = self
            .frames
            .get(index)
            .ok_or(PngDecodeError::FrameIndexOutOfBounds {
                index,
                frame_count: self.frames.len(),
            })?;

        super::super::decode::decode_indexed_frame_rgba8(self.input, frame, limits)
    }

    /// Decodes one uncomposited frame as big-endian RGBA16.
    pub fn decode_frame_rgba16_be(
        &self,
        index: usize,
        limits: PngDecodeLimits,
    ) -> Result<Rgba16BeImage, PngDecodeError> {
        let frame = self
            .frames
            .get(index)
            .ok_or(PngDecodeError::FrameIndexOutOfBounds {
                index,
                frame_count: self.frames.len(),
            })?;

        super::super::decode::decode_indexed_frame_rgba16_be(self.input, frame, limits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::image::png_format::{
        FrameDataReader, decode_zlib, encode_rgba8, inspect_animation,
    };
    use std::io::Read;

    #[test]
    fn indexes_wpt_fdat_frames_as_borrowed_streams() {
        // This checks frame streams, not the original browser reftest.
        let png = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/png/apng/support/033.png"
        ));

        let index = index_animation(png, AnimationLimits::default())
            .unwrap()
            .unwrap();

        assert_eq!(
            Some(index.summary),
            inspect_animation(png, AnimationLimits::default()).unwrap()
        );

        assert_eq!(index.frames.len(), 2);
        assert!(!index.summary.first_frame_uses_idat);

        for (i, frame) in index.frames.iter().enumerate() {
            let FrameSource::Fdat(parts) = &frame.source else {
                panic!("expected fdAT");
            };

            assert_eq!(frame.control.sequence_number, (i * 2) as u32);
            assert_eq!(parts.len(), 1);
            assert_eq!(parts[0].sequence_number, (i * 2 + 1) as u32);

            let input = FrameDataReader::new(&frame.control, parts).unwrap();

            let actual = decode_zlib(input, Vec::new(), (128 * 8 + 1) * 64)
                .unwrap()
                .output;

            let mut expected = Vec::new();

            flate2::read::ZlibDecoder::new(parts[0].compressed)
                .read_to_end(&mut expected)
                .unwrap();

            assert_eq!(actual, expected);
            assert_eq!(actual.len(), (128 * 8 + 1) * 64);
        }
    }

    fn sample(default_frame: bool, bad_sequence: bool) -> Vec<u8> {
        let mut bytes = Vec::new();

        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);

            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);

            let mut writer = encoder.write_header().unwrap();

            writer
                .write_chunk(png::chunk::ChunkType(*b"acTL"), &[0, 0, 0, 1, 0, 0, 0, 0])
                .unwrap();

            let mut fctl = [0; 26];

            fctl[4..8].copy_from_slice(&1u32.to_be_bytes());
            fctl[8..12].copy_from_slice(&1u32.to_be_bytes());

            if default_frame {
                writer
                    .write_chunk(png::chunk::ChunkType(*b"fcTL"), &fctl)
                    .unwrap();
            }

            writer.write_image_data(&[1, 2, 3, 4]).unwrap();

            if !default_frame {
                writer
                    .write_chunk(png::chunk::ChunkType(*b"fcTL"), &fctl)
                    .unwrap();

                writer
                    .write_chunk(png::chunk::ChunkType(*b"fdAT"), &[0, 0, 0, 1, 42])
                    .unwrap();

                writer
                    .write_chunk(
                        png::chunk::ChunkType(*b"fdAT"),
                        &[0, 0, 0, if bad_sequence { 3 } else { 2 }, 43],
                    )
                    .unwrap();
            }
        }

        bytes
    }

    #[test]
    fn indexes_default_image_or_multiple_fdat_chunks() {
        let static_png = encode_rgba8(1, 1, &[1, 2, 3, 4]).unwrap();

        assert!(
            index_animation(&static_png, AnimationLimits::default())
                .unwrap()
                .is_none()
        );

        let default = sample(true, false);

        let index = index_animation(&default, AnimationLimits::default())
            .unwrap()
            .unwrap();

        assert!(matches!(index.frames[0].source, FrameSource::DefaultImage));

        let split = sample(false, false);

        let index = index_animation(&split, AnimationLimits::default())
            .unwrap()
            .unwrap();

        let FrameSource::Fdat(parts) = &index.frames[0].source else {
            panic!("expected fdAT");
        };

        let mut reader = FrameDataReader::new(&index.frames[0].control, parts).unwrap();

        let mut bytes = Vec::new();

        reader.read_to_end(&mut bytes).unwrap();

        assert_eq!(bytes, [42, 43]);
    }

    #[test]
    fn rejects_bad_sequence_and_limits_without_returning_index() {
        let bad = sample(false, true);

        assert!(matches!(
            index_animation(&bad, AnimationLimits::default()),
            Err(AnimationError::UnexpectedSequence {
                expected: 2,
                actual: 3,
            })
        ));

        let good = sample(false, false);

        assert!(matches!(
            index_animation(
                &good,
                AnimationLimits {
                    max_frames: 0,
                    ..AnimationLimits::default()
                },
            ),
            Err(AnimationError::FrameLimit)
        ));

        assert!(matches!(
            index_animation(
                &good,
                AnimationLimits {
                    max_fdat_chunks: 1,
                    ..AnimationLimits::default()
                }
            ),
            Err(AnimationError::ChunkLimit)
        ));
    }

    #[test]
    fn decodes_wpt_animation_frames_as_uncomposited_pixels() {
        // This checks frame pixels, not animation playback or the browser reftest.
        let png = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/png/apng/support/033.png"
        ));

        let index = index_animation(png, AnimationLimits::default())
            .unwrap()
            .unwrap();

        let limits = PngDecodeLimits::default();

        let black = index.decode_frame_rgba8(0, limits).unwrap();
        let blue = index.decode_frame_rgba8(1, limits).unwrap();

        assert_eq!((black.width, black.height), (128, 64));

        assert!(
            black
                .pixels
                .chunks_exact(4)
                .all(|pixel| pixel == [0, 0, 0, 255])
        );

        assert!(
            blue.pixels
                .chunks_exact(4)
                .all(|pixel| pixel == [0, 0, 255, 128])
        );

        let blue16 = index.decode_frame_rgba16_be(1, limits).unwrap();

        assert!(
            blue16
                .pixels
                .chunks_exact(8)
                .all(|pixel| pixel == [0, 0, 0, 0, 255, 255, 128, 0])
        );

        // Compare both uncomposited frames with an independent decoder.
        let mut reference = png::Decoder::new(&png[..]).read_info().unwrap();
        let mut buffer = vec![0; reference.output_buffer_size()];

        // This fixture's default image is not part of the animation.
        reference.next_frame(&mut buffer).unwrap();

        for frame_buffer in 0..2 {
            let info = reference.next_frame(&mut buffer).unwrap();
            let actual = index.decode_frame_rgba16_be(frame_buffer, limits).unwrap();

            assert_eq!(&buffer[..info.buffer_size()], actual.pixels.as_slice());
        }

        assert!(matches!(
            index.decode_frame_rgba8(2, limits),
            Err(PngDecodeError::FrameIndexOutOfBounds {
                index: 2,
                frame_count: 2
            })
        ));

        assert!(matches!(
            index.decode_frame_rgba8(
                0,
                PngDecodeLimits {
                    max_pixels: 1,
                    ..limits
                }
            ),
            Err(PngDecodeError::LimitExceeded)
        ));
    }

    #[test]
    fn decodes_default_animation_frame_from_idat() {
        let png = sample(true, false);

        let index = index_animation(&png, AnimationLimits::default())
            .unwrap()
            .unwrap();

        let image = index
            .decode_frame_rgba8(0, PngDecodeLimits::default())
            .unwrap();

        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!(image.pixels, [1, 2, 3, 4]);
    }

    #[test]
    fn decodes_smaller_indexed_frame_using_original_palette() {
        use crate::graphics::image::png_format::encode_zlib_stored;

        let mut compressed = Vec::new();

        // One 2-bit palette index: 01.
        encode_zlib_stored(&[0, 0b0100_0000], &mut compressed).unwrap();

        let mut png = Vec::new();

        {
            let mut encoder = png::Encoder::new(&mut png, 2, 2);

            encoder.set_color(png::ColorType::Indexed);
            encoder.set_depth(png::BitDepth::Two);
            encoder.set_palette(vec![255, 0, 0, 0, 255, 0]);
            encoder.set_trns(vec![255, 42]);

            let mut writer = encoder.write_header().unwrap();

            writer
                .write_chunk(png::chunk::ChunkType(*b"acTL"), &[0, 0, 0, 1, 0, 0, 0, 0])
                .unwrap();

            writer.write_image_data(&[0, 0]).unwrap();

            let mut fctl = [0; 26];

            fctl[4..8].copy_from_slice(&1u32.to_be_bytes());
            fctl[8..12].copy_from_slice(&1u32.to_be_bytes());
            fctl[12..16].copy_from_slice(&1u32.to_be_bytes());
            fctl[16..20].copy_from_slice(&1u32.to_be_bytes());

            writer
                .write_chunk(png::chunk::ChunkType(*b"fcTL"), &fctl)
                .unwrap();

            let mut fdat = 1u32.to_be_bytes().to_vec();
            fdat.extend_from_slice(&compressed);

            writer
                .write_chunk(png::chunk::ChunkType(*b"fdAT"), &fdat)
                .unwrap();
        }

        let index = index_animation(&png, AnimationLimits::default())
            .unwrap()
            .unwrap();

        let frame = index
            .decode_frame_rgba8(0, PngDecodeLimits::default())
            .unwrap();

        assert_eq!((frame.width, frame.height), (1, 1));

        assert_eq!(
            (
                index.frames[0].control.x_offset,
                index.frames[0].control.y_offset
            ),
            (1, 1)
        );

        assert_eq!(frame.pixels, [0, 255, 0, 42]);
    }
}
