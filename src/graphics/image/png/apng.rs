//! APNG animation control metadata.
//!
//! PNG Third Edition, section 11.3.6.1:
//! <https://www.w3.org/TR/png-3/#acTL-chunk>

use super::Chunk;
use std::io::{self, Read};

mod composite;
mod index;
mod inspect;
mod player;

pub use self::composite::{AnimationCanvas, CompositeError};
pub use self::index::{AnimationIndex, FrameSource, IndexedFrame, index_animation};
pub use self::inspect::{AnimationError, AnimationLimits, AnimationSummary, inspect_animation};
pub use self::player::{AnimationPlayer, PlaybackError, PlaybackUpdate};

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

/// One fdAT payload. `from_chunk` checks its framing and CRC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameData<'a> {
    pub sequence_number: u32,
    pub compressed: &'a [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameDataError {
    WrongChunk,
    InvalidLength,
    CrcMismatch,
    InvalidSequenceNumber,
    MissingChunks,
    SequenceOverflow,
    UnexpectedSequence { expected: u32, actual: u32 },
}

impl std::fmt::Display for FrameDataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "APNG frame data error: {self:?}")
    }
}

impl std::error::Error for FrameDataError {}

impl<'a> FrameData<'a> {
    /// Parses one fdAT chunk. Cross-chunk order is not checked here.
    pub fn from_chunk(chunk: &Chunk<'a>) -> Result<Self, FrameDataError> {
        if chunk.kind != *b"fdAT" {
            return Err(FrameDataError::WrongChunk);
        }

        if chunk.data.len() < 4 {
            return Err(FrameDataError::InvalidLength);
        }

        if !chunk.crc_matches() {
            return Err(FrameDataError::CrcMismatch);
        }

        let sequence_number =
            u32::from_be_bytes([chunk.data[0], chunk.data[1], chunk.data[2], chunk.data[3]]);

        if sequence_number > 0x7fff_ffff {
            return Err(FrameDataError::InvalidSequenceNumber);
        }

        Ok(Self {
            sequence_number,
            compressed: &chunk.data[4..],
        })
    }
}

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

/// A zero-copy reader over fdAT data belonging to one frame.
///
/// The caller must supply all fdAT chunks of that frame in file order.
/// This reader does not inspect omitted chunks or validate the zlib stream.
pub struct FrameDataReader<'chunks, 'data> {
    chunks: &'chunks [FrameData<'data>],
    chunk_index: usize,
    offset: usize,
}

impl<'chunks, 'data> FrameDataReader<'chunks, 'data> {
    /// Checks that supplied fdAT sequence numbers immediately follow fcTL.
    pub fn new(
        frame: &FrameControl,
        chunks: &'chunks [FrameData<'data>],
    ) -> Result<Self, FrameDataError> {
        if chunks.is_empty() {
            return Err(FrameDataError::MissingChunks);
        }

        let mut previous = frame.sequence_number;

        for chunk in chunks {
            let expected = previous
                .checked_add(1)
                .filter(|&number| number <= 0x7fff_ffff)
                .ok_or(FrameDataError::SequenceOverflow)?;

            if chunk.sequence_number != expected {
                return Err(FrameDataError::UnexpectedSequence {
                    expected,
                    actual: chunk.sequence_number,
                });
            }

            previous = chunk.sequence_number;
        }

        Ok(Self {
            chunks,
            chunk_index: 0,
            offset: 0,
        })
    }
}

impl Read for FrameDataReader<'_, '_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }

        while let Some(chunk) = self.chunks.get(self.chunk_index) {
            let remaining = &chunk.compressed[self.offset..];

            if remaining.is_empty() {
                self.chunk_index += 1;
                self.offset = 0;
                continue;
            }

            let count = remaining.len().min(output.len());

            output[..count].copy_from_slice(&remaining[..count]);
            self.offset += count;

            return Ok(count);
        }

        Ok(0)
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

    #[test]
    fn decodes_split_frame_data_without_concatenation() {
        let frame = FrameControl::from_chunk(&chunk(*b"fcTL", &valid_data()), 5, 7).unwrap();

        let mut zlib = Vec::new();

        super::super::encode_zlib_stored(b"hello", &mut zlib).unwrap();

        // The first fdAT has an empty frame_data field.
        let first = 1u32.to_be_bytes().to_vec();

        let mut second = 2u32.to_be_bytes().to_vec();
        second.extend_from_slice(&zlib[..1]);

        let mut third = 3u32.to_be_bytes().to_vec();
        third.extend_from_slice(&zlib[1..]);

        let parts = [
            FrameData::from_chunk(&chunk(*b"fdAT", &first)).unwrap(),
            FrameData::from_chunk(&chunk(*b"fdAT", &second)).unwrap(),
            FrameData::from_chunk(&chunk(*b"fdAT", &third)).unwrap(),
        ];

        let mut reader = FrameDataReader::new(&frame, &parts).unwrap();

        assert_eq!(reader.read(&mut []).unwrap(), 0);

        let result = super::super::decode_zlib(reader, Vec::new(), 5).unwrap();

        assert_eq!(result.output, b"hello");
        assert_eq!(result.bytes_written, 5);
    }

    #[test]
    fn rejects_invalid_frame_data_and_sequence() {
        let data = [0, 0, 0, 1, 42];

        assert_eq!(
            FrameData::from_chunk(&chunk(*b"IDAT", &data)),
            Err(FrameDataError::WrongChunk)
        );

        assert_eq!(
            FrameData::from_chunk(&chunk(*b"fdAT", &data[..3])),
            Err(FrameDataError::InvalidLength)
        );

        let mut corrupt = chunk(*b"fdAT", &data);
        corrupt.stored_crc ^= 1;

        assert_eq!(
            FrameData::from_chunk(&corrupt),
            Err(FrameDataError::CrcMismatch)
        );

        assert_eq!(
            FrameData::from_chunk(&chunk(*b"fdAT", &[0x80, 0, 0, 0])),
            Err(FrameDataError::InvalidSequenceNumber)
        );

        let mut frame = FrameControl::from_chunk(&chunk(*b"fcTL", &valid_data()), 5, 7).unwrap();

        assert!(matches!(
            FrameDataReader::new(&frame, &[]),
            Err(FrameDataError::MissingChunks)
        ));

        let parts = [FrameData {
            sequence_number: 2,
            compressed: &[],
        }];

        assert!(matches!(
            FrameDataReader::new(&frame, &parts),
            Err(FrameDataError::UnexpectedSequence {
                expected: 1,
                actual: 2
            })
        ));

        frame.sequence_number = 0x7fff_ffff;

        assert!(matches!(
            FrameDataReader::new(&frame, &parts),
            Err(FrameDataError::SequenceOverflow)
        ));
    }

    #[test]
    fn inflates_wpt_fdat_frame_streams() {
        // This checks compressed frame streams, not animation playback.
        let png = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/png/apng/support/033.png"
        ));

        let mut reader = ChunkReader::new(png).unwrap();
        let mut current = None;
        let mut sequence_numbers = Vec::new();

        while let Some(chunk) = reader.next_chunk().unwrap() {
            if chunk.kind == *b"fcTL" {
                current = Some(FrameControl::from_chunk(&chunk, 128, 64).unwrap());
            }

            if chunk.kind == *b"fdAT" {
                let frame = current.take().expect("fdAT without fcTL");
                let data = FrameData::from_chunk(&chunk).unwrap();

                sequence_numbers.push(data.sequence_number);

                // This particular fixture has one fdAT per animation frame.
                let parts = [data];
                let stream = FrameDataReader::new(&frame, &parts).unwrap();

                let raw = super::super::decode_zlib(stream, Vec::new(), (128 * 8 + 1) * 64)
                    .unwrap()
                    .output;

                let mut independent = Vec::new();

                flate2::read::ZlibDecoder::new(data.compressed)
                    .read_to_end(&mut independent)
                    .unwrap();

                assert_eq!(raw.len(), (128 * 8 + 1) * 64);
                assert_eq!(raw, independent);
            }

            if chunk.kind == *b"IEND" {
                break;
            }
        }

        assert_eq!(sequence_numbers, [1, 3]);
    }
}
