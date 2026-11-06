//! JPEG frame header parsing.
//!
//! ITU-T T.81, Annex B, sections B.1 and B.2:
//! <https://www.w3.org/Graphics/JPEG/itu-t81.pdf>
//!
//! This only reads up to the first frame header. It does not validate or
//! decode scan data and does not prove that the JPEG is complete.

use std::{error::Error, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JpegFrameKind {
    Baseline,
    ExtendedSequential,
    Progressive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JpegComponent {
    pub id: u8,
    pub horizontal_sampling: u8,
    pub vertical_sampling: u8,
    pub quantization_table: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JpegFrameHeader {
    pub kind: JpegFrameKind,
    pub width: u16,
    pub height: u16,
    pub precision: u8,
    pub components: Vec<JpegComponent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JpegError {
    pub offset: usize,
    pub kind: JpegErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JpegErrorKind {
    MissingStartOfImage,
    Truncated,
    ExpectedMarker,
    InvalidMarker,
    UnexpectedMarker(u8),
    InvalidSegmentLength,
    MissingFrameHeader,
    UnsupportedFrame(u8),
    InvalidFrameHeader,
    UnsupportedDeferredHeight,
}

impl fmt::Display for JpegError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JPEG error at byte {}: {:?}", self.offset, self.kind)
    }
}

impl Error for JpegError {}

/// Reads the first supported frame header without decoding the image.
pub fn read_frame_header(input: &[u8]) -> Result<JpegFrameHeader, JpegError> {
    let error = |offset, kind| JpegError { offset, kind };

    if !input.starts_with(&[0xff, 0xd8]) {
        return Err(error(0, JpegErrorKind::MissingStartOfImage));
    }

    let mut cursor = 2;

    loop {
        let marker_offset = cursor;

        if input.get(cursor) != Some(&0xff) {
            return Err(error(
                cursor,
                if cursor == input.len() {
                    JpegErrorKind::Truncated
                } else {
                    JpegErrorKind::ExpectedMarker
                },
            ));
        }

        while input.get(cursor) == Some(&0xff) {
            cursor += 1;
        }

        let marker = *input
            .get(cursor)
            .ok_or_else(|| error(cursor, JpegErrorKind::Truncated))?;

        cursor += 1;

        match marker {
            0x00 => {
                return Err(error(marker_offset, JpegErrorKind::InvalidMarker));
            }

            // TEM is a standalone marker.
            0x01 => continue,

            0xd8 | 0xd0..=0xd7 => {
                return Err(error(
                    marker_offset,
                    JpegErrorKind::UnexpectedMarker(marker),
                ));
            }

            // A frame header must appear before the scan or end of image.
            0xd9 | 0xda => {
                return Err(error(marker_offset, JpegErrorKind::MissingFrameHeader));
            }

            _ => {}
        }

        let length_bytes = input
            .get(cursor..cursor.saturating_add(2))
            .ok_or_else(|| error(cursor, JpegErrorKind::Truncated))?;

        let length = usize::from(u16::from_be_bytes([length_bytes[0], length_bytes[1]]));

        if length < 2 {
            return Err(error(cursor, JpegErrorKind::InvalidSegmentLength));
        }

        let end = cursor
            .checked_add(length)
            .ok_or_else(|| error(cursor, JpegErrorKind::InvalidSegmentLength))?;

        let data = input
            .get(cursor + 2..end)
            .ok_or_else(|| error(cursor, JpegErrorKind::Truncated))?;

        cursor = end;

        let kind = match marker {
            0xc0 => JpegFrameKind::Baseline,
            0xc1 => JpegFrameKind::ExtendedSequential,
            0xc2 => JpegFrameKind::Progressive,

            0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                return Err(error(
                    marker_offset,
                    JpegErrorKind::UnsupportedFrame(marker),
                ));
            }

            _ => continue,
        };

        if data.len() < 6 {
            return Err(error(marker_offset, JpegErrorKind::InvalidFrameHeader));
        }

        let precision = data[0];
        let height = u16::from_be_bytes([data[1], data[2]]);
        let width = u16::from_be_bytes([data[3], data[4]]);
        let count = usize::from(data[5]);

        if count == 0 || data.len() != 6 + count * 3 || width == 0 {
            return Err(error(marker_offset, JpegErrorKind::InvalidFrameHeader));
        }

        if height == 0 {
            return Err(error(
                marker_offset,
                JpegErrorKind::UnsupportedDeferredHeight,
            ));
        }

        let valid_precision = match kind {
            JpegFrameKind::Baseline => precision == 8,
            _ => precision == 8 || precision == 12,
        };

        if !valid_precision {
            return Err(error(marker_offset, JpegErrorKind::InvalidFrameHeader));
        }

        let mut components = Vec::with_capacity(count);

        for bytes in data[6..].chunks_exact(3) {
            let horizontal_sampling = bytes[1] >> 4;
            let vertical_sampling = bytes[1] & 15;

            if !(1..=4).contains(&horizontal_sampling)
                || !(1..=4).contains(&vertical_sampling)
                || bytes[2] > 3
                || components
                    .iter()
                    .any(|component: &JpegComponent| component.id == bytes[0])
            {
                return Err(error(marker_offset, JpegErrorKind::InvalidFrameHeader));
            }

            components.push(JpegComponent {
                id: bytes[0],
                horizontal_sampling,
                vertical_sampling,
                quantization_table: bytes[2],
            });
        }

        return Ok(JpegFrameHeader {
            kind,
            width,
            height,
            precision,
            components,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = &[
        0xff, 0xd8, 0xff, 0xe0, 0, 4, 0xff, 0x00, 0xff, 0xc0, 0, 17, 8, 0, 16, 0, 32, 3, 1, 0x22,
        0, 2, 0x11, 1, 3, 0x11, 1,
    ];

    #[test]
    fn reads_baseline_frame_and_skips_metadata() {
        let frame = read_frame_header(SAMPLE).unwrap();

        assert_eq!(
            (frame.kind, frame.width, frame.height, frame.precision),
            (JpegFrameKind::Baseline, 32, 16, 8)
        );

        assert_eq!(
            frame.components[0],
            JpegComponent {
                id: 1,
                horizontal_sampling: 2,
                vertical_sampling: 2,
                quantization_table: 0
            }
        );
    }

    #[test]
    fn distinguishes_progressive_and_rejects_unsupported_frames() {
        let mut bytes = SAMPLE.to_vec();

        bytes[9] = 0xc2;

        assert_eq!(
            read_frame_header(&bytes).unwrap().kind,
            JpegFrameKind::Progressive
        );

        bytes[9] = 0xc3;

        assert_eq!(
            read_frame_header(&bytes).unwrap_err().kind,
            JpegErrorKind::UnsupportedFrame(0xc3)
        );
    }

    #[test]
    fn rejects_bad_inputs_without_panicking() {
        for length in 0..SAMPLE.len() {
            assert!(read_frame_header(&SAMPLE[..length]).is_err());
        }

        let mut bytes = SAMPLE.to_vec();

        bytes[10] = 0;
        bytes[11] = 1;

        assert_eq!(
            read_frame_header(&bytes).unwrap_err().kind,
            JpegErrorKind::InvalidSegmentLength
        );

        let mut bytes = SAMPLE.to_vec();

        bytes[19] = 0;

        assert_eq!(
            read_frame_header(&bytes).unwrap_err().kind,
            JpegErrorKind::InvalidFrameHeader
        );
    }

    #[test]
    fn permits_marker_fill_bytes_and_tem() {
        let mut bytes = vec![0xff, 0xd8, 0xff, 0x01, 0xff];

        bytes.extend_from_slice(&SAMPLE[8..]);

        assert_eq!(read_frame_header(&bytes).unwrap().width, 32);
    }
}
