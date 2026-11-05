//! APNG animation control metadata.
//!
//! PNG Third Edition, section 11.3.6.1:
//! <https://www.w3.org/TR/png-3/#acTL-chunk>

use super::Chunk;

/// Declared animation control values, not proof that the frames are valid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnimationControl {
    pub num_frames: u32,

    /// Zero means the animation repeats indefinitely.
    pub num_plays: u32,
}

/// Disposal operation requested by an APNG frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisposeOp {
    None,
    Background,
    Previous,
}

/// Composition operation requested by an APNG frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlendOp {
    Source,
    Over,
}

/// Contents of one fcTL chunk. Cross-chunk animation rules are not checked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameControl {
    pub sequence_number: u32,

    pub width: u32,
    pub height: u32,
    pub x_offset: u32,
    pub y_offset: u32,

    pub delay_num: u16,
    pub delay_den: u16,

    pub dispose_op: DisposeOp,
    pub blend_op: BlendOp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameControlError {
    WrongChunk,
    InvalidLength,
    CrcMismatch,
    InvalidSequenceNumber,
    InvalidFrameBounds,
    InvalidDisposeOp,
    InvalidBlendOp,
}

impl std::fmt::Display for FrameControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "APNG frame control error: {self:?}")
    }
}

impl std::error::Error for FrameControlError {}

impl FrameControl {
    /// Parses one fcTL chunk relative to the IHDR canvas size.
    ///
    /// This does not validate chunk ordering, frame count or fdAT data.
    pub fn from_chunk(
        chunk: &Chunk<'_>,
        canvas_width: u32,
        canvas_height: u32,
    ) -> Result<Self, FrameControlError> {
        if chunk.kind != *b"fcTL" {
            return Err(FrameControlError::WrongChunk);
        }

        if chunk.data.len() != 26 {
            return Err(FrameControlError::InvalidLength);
        }

        if !chunk.crc_matches() {
            return Err(FrameControlError::CrcMismatch);
        }

        let data = chunk.data;

        let u32_at = |index| {
            u32::from_be_bytes([
                data[index],
                data[index + 1],
                data[index + 2],
                data[index + 3],
            ])
        };

        let sequence_number = u32_at(0);
        let width = u32_at(4);
        let height = u32_at(8);
        let x_offset = u32_at(12);
        let y_offset = u32_at(16);

        if sequence_number > 0x7fff_ffff {
            return Err(FrameControlError::InvalidSequenceNumber);
        }

        if width == 0
            || height == 0
            || [width, height, x_offset, y_offset]
                .iter()
                .any(|&value| value > 0x7fff_ffff)
            || x_offset
                .checked_add(width)
                .is_none_or(|right| right > canvas_width)
            || y_offset
                .checked_add(height)
                .is_none_or(|bottom| bottom > canvas_height)
        {
            return Err(FrameControlError::InvalidFrameBounds);
        }

        let dispose_op = match data[24] {
            0 => DisposeOp::None,
            1 => DisposeOp::Background,
            2 => DisposeOp::Previous,
            _ => return Err(FrameControlError::InvalidDisposeOp),
        };

        let blend_op = match data[25] {
            0 => BlendOp::Source,
            1 => BlendOp::Over,
            _ => return Err(FrameControlError::InvalidBlendOp),
        };

        Ok(Self {
            sequence_number,
            width,
            height,
            x_offset,
            y_offset,
            delay_num: u16::from_be_bytes([data[20], data[21]]),
            delay_den: u16::from_be_bytes([data[22], data[23]]),
            dispose_op,
            blend_op,
        })
    }

    /// Returns the frame delay denominator; zero in fcTL means 100.
    pub fn effective_delay_denominator(&self) -> u16 {
        if self.delay_den == 0 {
            100
        } else {
            self.delay_den
        }
    }
}

pub(super) fn parse_animation_control(chunk: &Chunk<'_>) -> Option<AnimationControl> {
    if chunk.kind != *b"acTL" || !chunk.crc_matches() {
        return None;
    }

    let [a, b, c, d, e, f, g, h] = chunk.data else {
        return None;
    };

    let num_frames = u32::from_be_bytes([*a, *b, *c, *d]);
    let num_plays = u32::from_be_bytes([*e, *f, *g, *h]);

    // PNG four-byte unsigned integers are limited to 2^31 - 1.
    if num_frames == 0 || num_frames > 0x7fff_ffff || num_plays > 0x7fff_ffff {
        return None;
    }

    Some(AnimationControl {
        num_frames,
        num_plays,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{ChunkReader, update_crc};
    use super::*;

    fn chunk<'a>(kind: [u8; 4], data: &'a [u8]) -> Chunk<'a> {
        Chunk {
            offset: 0,
            kind,
            data,
            stored_crc: update_crc(update_crc(0xffff_ffff, &kind), data) ^ 0xffff_ffff,
        }
    }

    fn valid_data() -> [u8; 26] {
        let mut data = [0; 26];

        data[4..8].copy_from_slice(&2u32.to_be_bytes());
        data[8..12].copy_from_slice(&3u32.to_be_bytes());

        data
    }

    #[test]
    fn reads_frame_controls_from_wpt_fixture() {
        // This checks fcTL chunks, not the original reftest or frame pixels.
        let png = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/png/apng/support/033.png"
        ));

        let mut reader = ChunkReader::new(png).unwrap();
        let mut frames = Vec::new();

        while let Some(chunk) = reader.next_chunk().unwrap() {
            if chunk.kind == *b"fcTL" {
                frames.push(FrameControl::from_chunk(&chunk, 128, 64).unwrap());
            }

            if chunk.kind == *b"IEND" {
                break;
            }
        }

        assert_eq!(frames.len(), 2);

        assert_eq!(
            frames[0],
            FrameControl {
                sequence_number: 0,
                width: 128,
                height: 64,
                x_offset: 0,
                y_offset: 0,
                delay_num: 10,
                delay_den: 100,
                dispose_op: DisposeOp::None,
                blend_op: BlendOp::Over,
            }
        );

        assert_eq!(
            frames[1],
            FrameControl {
                sequence_number: 2,
                width: 128,
                height: 64,
                x_offset: 0,
                y_offset: 0,
                delay_num: 100,
                delay_den: 100,
                dispose_op: DisposeOp::None,
                blend_op: BlendOp::Over,
            }
        );
    }

    #[test]
    fn checks_frame_geometry_and_delay() {
        let mut data = valid_data();

        data[12..16].copy_from_slice(&3u32.to_be_bytes());
        data[16..20].copy_from_slice(&4u32.to_be_bytes());
        data[20..22].copy_from_slice(&7u16.to_be_bytes());
        data[24] = 2;
        data[25] = 1;

        let frame = FrameControl::from_chunk(&chunk(*b"fcTL", &data), 5, 7).unwrap();

        assert_eq!(
            (frame.width, frame.height, frame.x_offset, frame.y_offset),
            (2, 3, 3, 4)
        );

        assert_eq!(
            (
                frame.delay_num,
                frame.delay_den,
                frame.effective_delay_denominator()
            ),
            (7, 0, 100)
        );

        assert_eq!(
            (frame.dispose_op, frame.blend_op),
            (DisposeOp::Previous, BlendOp::Over)
        );

        assert_eq!(
            FrameControl::from_chunk(&chunk(*b"fcTL", &data), 4, 7),
            Err(FrameControlError::InvalidFrameBounds)
        );

        data[12..16].copy_from_slice(&u32::MAX.to_be_bytes());

        assert_eq!(
            FrameControl::from_chunk(&chunk(*b"fcTL", &data), u32::MAX, 7),
            Err(FrameControlError::InvalidFrameBounds)
        );

        data = valid_data();
        data[4..8].fill(0);

        assert_eq!(
            FrameControl::from_chunk(&chunk(*b"fcTL", &data), 5, 7),
            Err(FrameControlError::InvalidFrameBounds)
        );
    }

    #[test]
    fn rejects_bad_frame_control_chunks() {
        let mut data = valid_data();

        assert_eq!(
            FrameControl::from_chunk(&chunk(*b"fdAT", &data), 5, 7),
            Err(FrameControlError::WrongChunk)
        );

        assert_eq!(
            FrameControl::from_chunk(&chunk(*b"fcTL", &data[..25]), 5, 7),
            Err(FrameControlError::InvalidLength)
        );

        let mut corrupt = chunk(*b"fcTL", &data);
        corrupt.stored_crc ^= 1;

        assert_eq!(
            FrameControl::from_chunk(&corrupt, 5, 7),
            Err(FrameControlError::CrcMismatch)
        );

        data[0..4].copy_from_slice(&0x8000_0000u32.to_be_bytes());

        assert_eq!(
            FrameControl::from_chunk(&chunk(*b"fcTL", &data), 5, 7),
            Err(FrameControlError::InvalidSequenceNumber)
        );

        data = valid_data();
        data[24] = 3;

        assert_eq!(
            FrameControl::from_chunk(&chunk(*b"fcTL", &data), 5, 7),
            Err(FrameControlError::InvalidDisposeOp)
        );

        data = valid_data();
        data[25] = 2;

        assert_eq!(
            FrameControl::from_chunk(&chunk(*b"fcTL", &data), 5, 7),
            Err(FrameControlError::InvalidBlendOp)
        );
    }
}
