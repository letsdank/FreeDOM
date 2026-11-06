//! JPEG frame header parsing.
//!
//! ITU-T T.81, Annex B, sections B.1 and B.2:
//! <https://www.w3.org/Graphics/JPEG/itu-t81.pdf>
//!
//! This only reads up to the first frame header. It does not validate or
//! decode scan data and does not prove that the JPEG is complete.

use std::{error::Error, fmt};

#[path = "jpeg/huffman.rs"]
mod huffman;

pub use self::huffman::{HuffmanTable, HuffmanTables, parse_dht_segment, read_huffman_tables};

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
    InvalidQuantizationTable,
    InvalidQuantizationPrecision(u8),
    InvalidQuantizationTableId(u8),
    ZeroQuantizationValue,
    InvalidHuffmanTable,
    InvalidHuffmanClass(u8),
    InvalidHuffmanTableId(u8),
    TooManyHuffmanSymbols,
    InvalidHuffmanCodeLengths,
}

impl fmt::Display for JpegError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JPEG error at byte {}: {:?}", self.offset, self.kind)
    }
}

impl Error for JpegError {}

/// Segment parsing stops at SOS or EOI; entropy-coded data is not inspected.
#[derive(Debug)]
struct HeaderSegment<'a> {
    marker: u8,
    marker_offset: usize,
    data_offset: usize,
    data: &'a [u8],
}

struct HeaderSegments<'a> {
    input: &'a [u8],
    cursor: usize,
    end_marker_offset: Option<usize>,
}

impl<'a> HeaderSegments<'a> {
    fn new(input: &'a [u8]) -> Result<Self, JpegError> {
        if !input.starts_with(&[0xff, 0xd8]) {
            return Err(JpegError {
                offset: 0,
                kind: JpegErrorKind::MissingStartOfImage,
            });
        }

        Ok(Self {
            input,
            cursor: 2,
            end_marker_offset: None,
        })
    }

    fn next_segment(&mut self) -> Result<Option<HeaderSegment<'a>>, JpegError> {
        if self.end_marker_offset.is_some() {
            return Ok(None);
        }

        let error = |offset, kind| JpegError { offset, kind };

        loop {
            let marker_offset = self.cursor;

            if self.input.get(self.cursor) != Some(&0xff) {
                return Err(error(
                    self.cursor,
                    if self.cursor == self.input.len() {
                        JpegErrorKind::Truncated
                    } else {
                        JpegErrorKind::ExpectedMarker
                    },
                ));
            }

            while self.input.get(self.cursor) == Some(&0xff) {
                self.cursor += 1;
            }

            let marker = *self
                .input
                .get(self.cursor)
                .ok_or_else(|| error(self.cursor, JpegErrorKind::Truncated))?;

            self.cursor += 1;

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

                // Scan data is not parsed by this header.
                0xd9 | 0xda => {
                    self.end_marker_offset = Some(marker_offset);
                    return Ok(None);
                }

                _ => {}
            }

            let bytes = self
                .input
                .get(self.cursor..self.cursor.saturating_add(2))
                .ok_or_else(|| error(self.cursor, JpegErrorKind::Truncated))?;

            let length = usize::from(u16::from_be_bytes([bytes[0], bytes[1]]));

            if length < 2 {
                return Err(error(self.cursor, JpegErrorKind::InvalidSegmentLength));
            }

            let end = self
                .cursor
                .checked_add(length)
                .ok_or_else(|| error(self.cursor, JpegErrorKind::InvalidSegmentLength))?;

            let data_offset = self.cursor + 2;

            let data = self
                .input
                .get(data_offset..end)
                .ok_or_else(|| error(self.cursor, JpegErrorKind::Truncated))?;

            self.cursor = end;

            return Ok(Some(HeaderSegment {
                marker,
                marker_offset,
                data_offset,
                data,
            }));
        }
    }
}

/// Reads the first supported frame header without decoding the image.
pub fn read_frame_header(input: &[u8]) -> Result<JpegFrameHeader, JpegError> {
    let error = |offset, kind| JpegError { offset, kind };
    let mut segments = HeaderSegments::new(input)?;

    loop {
        let Some(segment) = segments.next_segment()? else {
            return Err(error(
                segments
                    .end_marker_offset
                    .expect("SOS or EOI was encountered"),
                JpegErrorKind::MissingFrameHeader,
            ));
        };

        let HeaderSegment {
            marker,
            marker_offset,
            data,
            ..
        } = segment;

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

/// Table entries in natural row-major coefficient order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuantizationTable {
    /// Precision in bits: 8 or 16.
    pub precision: u8,
    pub values: [u16; 64],
}

// ITU-T T.81, Figure A.6, zig-zag index to row-major index.
const ZIGZAG_TO_NATURAL: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// Parses the payload of a DQT segment, excluding the marker and length.
///
/// Error offsets are relative to `data`. Duplicate identifiers in a segment
/// use the last definition. Tables from other segments are not included.
pub fn parse_dqt_segment(data: &[u8]) -> Result<[Option<QuantizationTable>; 4], JpegError> {
    let error = |offset, kind| JpegError { offset, kind };

    if data.is_empty() {
        return Err(error(0, JpegErrorKind::InvalidQuantizationTable));
    }

    let mut tables: [Option<QuantizationTable>; 4] = [None, None, None, None];

    let mut cursor = 0;

    while cursor < data.len() {
        let info_offset = cursor;
        let info = data[cursor];

        cursor += 1;

        let precision = info >> 4;
        let id = info & 0x0f;

        if precision > 1 {
            return Err(error(
                info_offset,
                JpegErrorKind::InvalidQuantizationPrecision(precision),
            ));
        }

        if id > 3 {
            return Err(error(
                info_offset,
                JpegErrorKind::InvalidQuantizationTableId(id),
            ));
        }

        let entry_size = if precision == 0 { 1 } else { 2 };
        let len = 64 * entry_size;

        let end = cursor
            .checked_add(len)
            .ok_or_else(|| error(cursor, JpegErrorKind::Truncated))?;

        let source = data
            .get(cursor..end)
            .ok_or_else(|| error(cursor, JpegErrorKind::Truncated))?;

        let mut values = [0; 64];

        for (index, &natural_index) in ZIGZAG_TO_NATURAL.iter().enumerate() {
            let offset = index * entry_size;

            let value = if entry_size == 1 {
                u16::from(source[offset])
            } else {
                u16::from_be_bytes([source[offset], source[offset + 1]])
            };

            if value == 0 {
                return Err(error(cursor + offset, JpegErrorKind::ZeroQuantizationValue));
            }

            values[natural_index] = value;
        }

        tables[id as usize] = Some(QuantizationTable {
            precision: if precision == 0 { 8 } else { 16 },
            values,
        });

        cursor = end;
    }

    Ok(tables)
}

/// Reads DQT definitions before the first SOS or EOI marker.
/// Does not validate scan data or prove that the JPEG is complete.
pub fn read_quantization_tables(input: &[u8]) -> Result<[Option<QuantizationTable>; 4], JpegError> {
    let mut segments = HeaderSegments::new(input)?;

    let mut tables: [Option<QuantizationTable>; 4] = [None, None, None, None];

    while let Some(segment) = segments.next_segment()? {
        if segment.marker != 0xdb {
            continue;
        }

        let parsed = parse_dqt_segment(segment.data).map_err(|mut error| {
            error.offset += segment.data_offset;
            error
        })?;

        for (destination, table) in tables.iter_mut().zip(parsed) {
            if table.is_some() {
                *destination = table;
            }
        }
    }

    Ok(tables)
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

    #[test]
    fn reads_eight_and_sixteen_bit_quantization_tables_in_natural_order() {
        let mut bytes = vec![0];

        bytes.extend(1u8..=64);

        // Precision 1, table identifier 1.
        bytes.push(0x11);

        for value in 256u16..320 {
            bytes.extend_from_slice(&value.to_be_bytes());
        }

        let tables = parse_dqt_segment(&bytes).unwrap();

        let first = tables[0].as_ref().unwrap();

        assert_eq!(first.precision, 8);

        assert_eq!(
            [
                first.values[0],
                first.values[1],
                first.values[8],
                first.values[16]
            ],
            [1, 2, 3, 4]
        );

        let second = tables[1].as_ref().unwrap();

        assert_eq!(second.precision, 16);

        assert_eq!(
            [second.values[0], second.values[1], second.values[8]],
            [256, 257, 258]
        );

        assert!(tables[2].is_none());
    }

    #[test]
    fn last_quantization_table_definition_wins() {
        let mut bytes = vec![0];

        bytes.extend([1; 64]);

        bytes.push(0);
        bytes.extend([2; 64]);

        let tables = parse_dqt_segment(&bytes).unwrap();

        assert_eq!(tables[0].as_ref().unwrap().values, [2; 64]);
    }

    #[test]
    fn rejects_invalid_quantization_data() {
        assert_eq!(
            parse_dqt_segment(&[]).unwrap_err().kind,
            JpegErrorKind::InvalidQuantizationTable
        );

        assert_eq!(
            parse_dqt_segment(&[0x20]).unwrap_err().kind,
            JpegErrorKind::InvalidQuantizationPrecision(2)
        );

        assert_eq!(
            parse_dqt_segment(&[0x04]).unwrap_err().kind,
            JpegErrorKind::InvalidQuantizationTableId(4)
        );

        assert_eq!(
            parse_dqt_segment(&[0x10, 0x01]).unwrap_err().kind,
            JpegErrorKind::Truncated
        );

        let mut bytes = vec![0];

        bytes.extend([1; 64]);
        bytes[2] = 0;

        assert_eq!(
            parse_dqt_segment(&bytes).unwrap_err().kind,
            JpegErrorKind::ZeroQuantizationValue
        );
    }

    #[test]
    fn reads_wpt_progressive_jpeg_header_and_quantization_tables() {
        // The WPT asset is tested here, not the original browser test.
        let image = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/images/computer.jpg"
        ));

        let frame = read_frame_header(image).unwrap();

        assert_eq!(
            (
                frame.kind,
                frame.width,
                frame.height,
                frame.precision,
                frame.components.len()
            ),
            (JpegFrameKind::Progressive, 320, 240, 8, 3)
        );

        let tables = read_quantization_tables(image).unwrap();

        assert_eq!(tables[0].as_ref().unwrap().precision, 16);
        assert_eq!(tables[1].as_ref().unwrap().precision, 16);

        assert_eq!(tables[0].as_ref().unwrap().values[0], 100);
        assert_eq!(tables[0].as_ref().unwrap().values[9], 106);

        assert!(
            frame
                .components
                .iter()
                .all(|component| { tables[component.quantization_table as usize].is_some() })
        );
    }

    #[test]
    fn reads_quantization_segments_across_frame_and_overrides_previous_values() {
        let mut data = vec![0xff, 0xd8, 0xff, 0xdb, 0, 67, 0];

        data.extend([1; 64]);

        // One-component baseline frame.
        data.extend_from_slice(&[0xff, 0xc0, 0, 11, 8, 0, 1, 0, 1, 1, 1, 0x11, 0]);
        data.extend_from_slice(&[0xff, 0xdb, 0, 67, 0]);
        data.extend([2; 64]);

        // The reader stops at SOS and does not parse its contents.
        data.extend_from_slice(&[0xff, 0xda]);

        assert_eq!(read_frame_header(&data).unwrap().width, 1);

        let tables = read_quantization_tables(&data).unwrap();

        assert_eq!(tables[0].as_ref().unwrap().values, [2; 64]);
    }

    #[test]
    fn quantization_error_offset_is_absolute() {
        let mut data = vec![0xff, 0xd8, 0xff, 0xdb, 0, 67, 0];

        data.extend([1; 64]);

        data[8] = 0;

        data.extend_from_slice(&[0xff, 0xda]);

        let error = read_quantization_tables(&data).unwrap_err();

        assert_eq!(error.kind, JpegErrorKind::ZeroQuantizationValue);
        assert_eq!(error.offset, 8);
    }
}
