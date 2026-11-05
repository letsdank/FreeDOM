//! Borrowed APNG frame index.
//!
//! PNG Third Edition, section 4.9:
//! <https://www.w3.org/TR/png-3/#apng-frame-based-animation>

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

    Ok(Some(AnimationIndex { summary, frames }))
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
}
