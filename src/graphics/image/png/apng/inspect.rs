//! Checks APNG chunk sequencing without decoding frame pixels.
//!
//! PNG Third Edition, sections 4.9 and 11.3.6:
//! <https://www.w3.org/TR/png-3/#apng-frame-based-animation>

use super::super::{ChunkReader, PngError, inspect_structure};
use super::{
    AnimationControl, FrameControl, FrameControlError, FrameData, FrameDataError, FrameSource,
    IndexedFrame, parse_animation_control,
};

#[derive(Clone, Copy, Debug)]
pub struct AnimationLimits {
    pub max_frames: u32,
    pub max_fdat_chunks: usize,
}

impl Default for AnimationLimits {
    fn default() -> Self {
        Self {
            max_frames: 4096,
            max_fdat_chunks: 65_536,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnimationSummary {
    pub control: AnimationControl,
    pub frame_count: u32,
    pub fdat_chunks: usize,
    pub first_frame_uses_idat: bool,
}

#[derive(Debug)]
pub enum AnimationError {
    Png(PngError),

    MissingAnimationControl,
    InvalidAnimationControl,
    DuplicateAnimationControl,
    AnimationControlAfterIdat,

    FrameControl(FrameControlError),
    FrameData(FrameDataError),

    UnexpectedSequence { expected: u64, actual: u32 },

    InvalidDefaultFrame,
    UnexpectedFrameData,
    MissingFrameData,

    FrameCountMismatch { declared: u32, actual: u32 },

    FrameLimit,
    ChunkLimit,
    AllocationFailed,
}

impl std::fmt::Display for AnimationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "APNG inspection error: {self:?}")
    }
}

impl std::error::Error for AnimationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Png(error) => Some(error),
            Self::FrameControl(error) => Some(error),
            Self::FrameData(error) => Some(error),
            _ => None,
        }
    }
}

fn advance_sequence(expected: &mut u64, actual: u32) -> Result<(), AnimationError> {
    if u64::from(actual) != *expected {
        return Err(AnimationError::UnexpectedSequence {
            expected: *expected,
            actual,
        });
    }

    *expected += 1;

    Ok(())
}

/// Checks animation chunk order and counts without allocating frame lists.
///
/// Returns None for a static PNG. Does not decode frames, validate image data,
/// check all PNG CRCs or enforce an input-size / CPU budget. The ordinary PNG
/// decoder remains usable separately when animation inspection fails.
pub fn inspect_animation(
    input: &[u8],
    limits: AnimationLimits,
) -> Result<Option<AnimationSummary>, AnimationError> {
    scan_animation(input, limits, None)
}

pub(super) fn scan_animation<'a>(
    input: &'a [u8],
    limits: AnimationLimits,
    mut frames: Option<&mut Vec<IndexedFrame<'a>>>,
) -> Result<Option<AnimationSummary>, AnimationError> {
    let structure = inspect_structure(input).map_err(AnimationError::Png)?;
    let mut reader = ChunkReader::new(input).map_err(AnimationError::Png)?;

    let mut control: Option<AnimationControl> = None;
    let mut seen_idat = false;

    let mut frame_count = 0u32;
    let mut fdat_chunks = 0usize;
    let mut expected_sequence = 0u64;

    let mut frame_open = false;
    let mut frame_uses_idat = false;
    let mut frame_has_data = false;
    let mut first_frame_uses_idat = false;

    while let Some(chunk) = reader.next_chunk().map_err(AnimationError::Png)? {
        match &chunk.kind {
            b"acTL" => {
                if control.is_some() {
                    return Err(AnimationError::DuplicateAnimationControl);
                }

                if seen_idat {
                    return Err(AnimationError::AnimationControlAfterIdat);
                }

                let value = parse_animation_control(&chunk)
                    .ok_or(AnimationError::InvalidAnimationControl)?;

                if value.num_frames > limits.max_frames {
                    return Err(AnimationError::FrameLimit);
                }

                control = Some(value);
            }

            b"IDAT" => {
                seen_idat = true;

                if frame_open && frame_uses_idat {
                    frame_has_data = true;
                }
            }

            b"fcTL" => {
                if control.is_none() {
                    return Err(AnimationError::MissingAnimationControl);
                }

                if frame_open && !frame_has_data {
                    return Err(AnimationError::MissingFrameData);
                }

                let frame = FrameControl::from_chunk(
                    &chunk,
                    structure.header.width,
                    structure.header.height,
                )
                .map_err(AnimationError::FrameControl)?;

                advance_sequence(&mut expected_sequence, frame.sequence_number)?;

                if frame_count >= limits.max_frames {
                    return Err(AnimationError::FrameLimit);
                }

                frame_uses_idat = !seen_idat;

                if frame_uses_idat {
                    if frame_count != 0
                        || frame.x_offset != 0
                        || frame.y_offset != 0
                        || frame.width != structure.header.width
                        || frame.height != structure.header.height
                    {
                        return Err(AnimationError::InvalidDefaultFrame);
                    }

                    first_frame_uses_idat = true;
                }

                if let Some(frames) = frames.as_deref_mut() {
                    frames
                        .try_reserve(1)
                        .map_err(|_| AnimationError::AllocationFailed)?;

                    frames.push(IndexedFrame {
                        control: frame,
                        source: if frame_uses_idat {
                            FrameSource::DefaultImage
                        } else {
                            FrameSource::Fdat(Vec::new())
                        },
                    });
                }

                frame_count += 1;
                frame_open = true;
                frame_has_data = false;
            }

            b"fdAT" => {
                if control.is_none() {
                    return Err(AnimationError::MissingAnimationControl);
                }

                if !seen_idat || !frame_open || frame_uses_idat {
                    return Err(AnimationError::UnexpectedFrameData);
                }

                let data = FrameData::from_chunk(&chunk).map_err(AnimationError::FrameData)?;

                advance_sequence(&mut expected_sequence, data.sequence_number)?;

                fdat_chunks = fdat_chunks
                    .checked_add(1)
                    .ok_or(AnimationError::ChunkLimit)?;

                if fdat_chunks > limits.max_fdat_chunks {
                    return Err(AnimationError::ChunkLimit);
                }

                if let Some(frames) = frames.as_deref_mut() {
                    let Some(IndexedFrame {
                        source: FrameSource::Fdat(parts),
                        ..
                    }) = frames.last_mut()
                    else {
                        return Err(AnimationError::UnexpectedFrameData);
                    };

                    parts
                        .try_reserve(1)
                        .map_err(|_| AnimationError::AllocationFailed)?;

                    parts.push(data);
                }

                frame_has_data = true;
            }

            b"IEND" => break,

            _ => {}
        }
    }

    let Some(control) = control else {
        return Ok(None);
    };

    if frame_open && !frame_has_data {
        return Err(AnimationError::MissingFrameData);
    }

    if frame_count != control.num_frames {
        return Err(AnimationError::FrameCountMismatch {
            declared: control.num_frames,
            actual: frame_count,
        });
    }

    Ok(Some(AnimationSummary {
        control,
        frame_count,
        fdat_chunks,
        first_frame_uses_idat,
    }))
}

#[cfg(test)]
mod tests {
    use super::super::super::{encode_rgba8, update_crc};
    use super::*;

    fn insert(png: &mut Vec<u8>, before: &[u8; 4], kind: &[u8; 4], data: &[u8]) {
        let mut reader = ChunkReader::new(png).unwrap();

        let offset = loop {
            let chunk = reader.next_chunk().unwrap().unwrap();

            if &chunk.kind == before {
                break chunk.offset;
            }
        };

        let mut bytes = Vec::new();

        bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(data);

        let crc = update_crc(update_crc(0xffff_ffff, kind), data) ^ 0xffff_ffff;

        bytes.extend_from_slice(&crc.to_be_bytes());
        png.splice(offset..offset, bytes);
    }

    fn frame(sequence: u32, width: u32, height: u32) -> [u8; 26] {
        let mut data = [0; 26];

        data[0..4].copy_from_slice(&sequence.to_be_bytes());
        data[4..8].copy_from_slice(&width.to_be_bytes());
        data[8..12].copy_from_slice(&height.to_be_bytes());

        data
    }

    #[test]
    fn inspects_wpt_animation_with_separate_default_image() {
        // This checks chunk structure, not the original browser reftest.
        let png = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/png/apng/support/033.png"
        ));

        assert_eq!(
            inspect_animation(png, AnimationLimits::default()).unwrap(),
            Some(AnimationSummary {
                control: AnimationControl {
                    num_frames: 2,
                    num_plays: 1,
                },
                frame_count: 2,
                fdat_chunks: 2,
                first_frame_uses_idat: false,
            })
        );
    }

    #[test]
    fn inspects_static_and_animation_with_default_frame() {
        let mut png = encode_rgba8(2, 2, &[255; 16]).unwrap();

        assert_eq!(
            inspect_animation(&png, AnimationLimits::default()).unwrap(),
            None
        );

        insert(&mut png, b"IDAT", b"acTL", &[0, 0, 0, 2, 0, 0, 0, 0]);
        insert(&mut png, b"IDAT", b"fcTL", &frame(0, 2, 2));
        insert(&mut png, b"IEND", b"fcTL", &frame(1, 1, 1));
        insert(&mut png, b"IEND", b"fdAT", &2u32.to_be_bytes());

        let result = inspect_animation(&png, AnimationLimits::default())
            .unwrap()
            .unwrap();

        assert_eq!(
            (
                result.frame_count,
                result.fdat_chunks,
                result.first_frame_uses_idat
            ),
            (2, 1, true)
        );
    }

    #[test]
    fn rejects_animation_sequence_errors_and_limits() {
        let mut png = encode_rgba8(2, 2, &[255; 16]).unwrap();

        insert(&mut png, b"IDAT", b"acTL", &[0, 0, 0, 2, 0, 0, 0, 0]);
        insert(&mut png, b"IDAT", b"fcTL", &frame(0, 2, 2));
        insert(&mut png, b"IEND", b"fcTL", &frame(2, 1, 1));
        insert(&mut png, b"IEND", b"fdAT", &3u32.to_be_bytes());

        assert!(matches!(
            inspect_animation(&png, AnimationLimits::default()),
            Err(AnimationError::UnexpectedSequence {
                expected: 1,
                actual: 2,
            })
        ));

        assert!(matches!(
            inspect_animation(
                &png,
                AnimationLimits {
                    max_frames: 1,
                    ..AnimationLimits::default()
                }
            ),
            Err(AnimationError::FrameLimit)
        ));

        let mut missing = encode_rgba8(2, 2, &[255; 16]).unwrap();

        insert(&mut missing, b"IDAT", b"acTL", &[0, 0, 0, 1, 0, 0, 0, 0]);
        insert(&mut missing, b"IEND", b"fcTL", &frame(0, 1, 1));

        assert!(matches!(
            inspect_animation(&missing, AnimationLimits::default()),
            Err(AnimationError::MissingFrameData)
        ));

        let mut wrong = encode_rgba8(2, 2, &[255; 16]).unwrap();

        insert(&mut wrong, b"IDAT", b"acTL", &[0, 0, 0, 1, 0, 0, 0, 0]);
        insert(&mut wrong, b"IDAT", b"fcTL", &frame(0, 1, 1));

        assert!(matches!(
            inspect_animation(&wrong, AnimationLimits::default()),
            Err(AnimationError::InvalidDefaultFrame)
        ));
    }

    #[test]
    fn rejects_unannounced_fdat_and_incorrect_count() {
        let mut png = encode_rgba8(2, 2, &[255; 16]).unwrap();

        insert(&mut png, b"IDAT", b"fdAT", &0u32.to_be_bytes());

        assert!(matches!(
            inspect_animation(&png, AnimationLimits::default()),
            Err(AnimationError::MissingAnimationControl)
        ));

        let mut png = encode_rgba8(2, 2, &[255; 16]).unwrap();

        insert(&mut png, b"IDAT", b"acTL", &[0, 0, 0, 2, 0, 0, 0, 0]);
        insert(&mut png, b"IEND", b"fcTL", &frame(0, 1, 1));
        insert(&mut png, b"IEND", b"fdAT", &1u32.to_be_bytes());

        assert!(matches!(
            inspect_animation(&png, AnimationLimits::default()),
            Err(AnimationError::FrameCountMismatch {
                declared: 2,
                actual: 1,
            })
        ));

        assert!(matches!(
            inspect_animation(
                &png,
                AnimationLimits {
                    max_fdat_chunks: 0,
                    ..AnimationLimits::default()
                }
            ),
            Err(AnimationError::ChunkLimit)
        ));
    }
}
