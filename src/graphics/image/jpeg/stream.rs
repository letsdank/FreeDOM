//! Traversal of JPEG segments and entropy-coded scan boundaries.
//!
//! ITU-T T.81, sections B.1.1.5, B.2.3 and F.1.2.3:
//! <https://www.w3.org/Graphics/JPEG/itu-t81.pdf>
//!
//! This locates scans but does not decode or validate their entropy data.

use super::{HeaderSegments, JpegError, JpegErrorKind, ScanHeader, scan::parse_scan_header_at};

#[derive(Debug)]
pub enum JpegEvent<'a> {
    Segment {
        marker: u8,
        marker_offset: usize,
        data_offset: usize,
        data: &'a [u8],
    },

    Scan {
        header: ScanHeader,

        /// End of encoded scan data; restart markers remain inside the range.
        entropy_end: usize,
    },

    End {
        marker_offset: usize,
    },
}

pub struct JpegEventReader<'a> {
    input: &'a [u8],
    cursor: usize,
    finished: bool,
}

impl<'a> JpegEventReader<'a> {
    pub fn new(input: &'a [u8]) -> Result<Self, JpegError> {
        HeaderSegments::new(input)?;

        Ok(Self {
            input,
            cursor: 2,
            finished: false,
        })
    }

    /// Returns the next event in file order.
    ///
    /// After an error the reader stops.
    pub fn next_event(&mut self) -> Result<Option<JpegEvent<'a>>, JpegError> {
        if self.finished {
            return Ok(None);
        }

        let result = self.read_next();

        if result.is_err() {
            self.finished = true;
        }

        result
    }

    fn read_next(&mut self) -> Result<Option<JpegEvent<'a>>, JpegError> {
        let mut segments = HeaderSegments {
            input: self.input,
            cursor: self.cursor,
            end_marker_offset: None,
        };

        if let Some(segment) = segments.next_segment()? {
            self.cursor = segments.cursor;

            return Ok(Some(JpegEvent::Segment {
                marker: segment.marker,
                marker_offset: segment.marker_offset,
                data_offset: segment.data_offset,
                data: segment.data,
            }));
        }

        let marker_offset = segments.end_marker_offset.expect("SOS or EOI");
        let marker = self.input[segments.cursor - 1];

        if marker == 0xd9 {
            self.finished = true;

            return Ok(Some(JpegEvent::End { marker_offset }));
        }

        let header = parse_scan_header_at(self.input, marker_offset, segments.cursor)?;
        let entropy_end = scan_end(self.input, header.entropy_offset)?;
        self.cursor = entropy_end;

        Ok(Some(JpegEvent::Scan {
            header,
            entropy_end,
        }))
    }
}

fn scan_end(input: &[u8], mut cursor: usize) -> Result<usize, JpegError> {
    while cursor < input.len() {
        if input[cursor] != 0xff {
            cursor += 1;
            continue;
        }

        let marker_offset = cursor;

        cursor += 1;

        while input.get(cursor) == Some(&0xff) {
            cursor += 1;
        }

        let marker = *input.get(cursor).ok_or(JpegError {
            offset: marker_offset,
            kind: JpegErrorKind::Truncated,
        })?;

        match marker {
            // Stuffed 0xff byte inside entropy-coded data.
            0x00 if cursor == marker_offset + 1 => {
                cursor += 1;
            }

            0x00 => {
                return Err(JpegError {
                    offset: marker_offset,
                    kind: JpegErrorKind::InvalidMarker,
                });
            }

            // Restart markers belong to the current scan.
            0xd0..=0xd7 => {
                cursor += 1;
            }

            // Any other marker terminates the current scan.
            _ => return Ok(marker_offset),
        }
    }

    Err(JpegError {
        offset: cursor,
        kind: JpegErrorKind::Truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::super::read_first_scan_header;
    use super::*;

    fn segment(out: &mut Vec<u8>, marker: u8, data: &[u8]) {
        out.extend_from_slice(&[0xff, marker]);
        out.extend_from_slice(&((data.len() + 2) as u16).to_be_bytes());
        out.extend_from_slice(data);
    }

    #[test]
    fn scans_stuffed_bytes_restart_markers_and_interleaved_segments() {
        let mut input = vec![0xff, 0xd8];

        segment(&mut input, 0xc4, &[7]);
        segment(&mut input, 0xda, &[1, 1, 0, 0, 0, 0]);

        let first_start = input.len();

        input.extend_from_slice(&[0x12, 0xff, 0x00, 0x34, 0xff, 0xff, 0xd0, 0x56]);

        let first_end = input.len();

        segment(&mut input, 0xc4, &[9]);
        segment(&mut input, 0xda, &[1, 1, 0, 1, 63, 0]);

        input.extend_from_slice(&[0x11, 0xff, 0xd9]);

        let first_header = read_first_scan_header(&input).unwrap();
        let mut reader = JpegEventReader::new(&input).unwrap();

        assert!(matches!(
            reader.next_event().unwrap(),
            Some(JpegEvent::Segment {
                marker: 0xc4,
                data: &[7],
                ..
            })
        ));

        match reader.next_event().unwrap().unwrap() {
            JpegEvent::Scan {
                header,
                entropy_end,
            } => {
                assert_eq!(header, first_header);

                assert_eq!(
                    (header.entropy_offset, entropy_end),
                    (first_start, first_end)
                );
            }

            _ => panic!("expected first scan"),
        }

        assert!(matches!(
            reader.next_event().unwrap(),
            Some(JpegEvent::Segment {
                marker: 0xc4,
                data: &[9],
                ..
            })
        ));

        assert!(matches!(
            reader.next_event().unwrap(),
            Some(JpegEvent::Scan { .. })
        ));

        assert!(matches!(
            reader.next_event().unwrap(),
            Some(JpegEvent::End { .. })
        ));

        assert!(reader.next_event().unwrap().is_none());
    }

    #[test]
    fn rejects_truncated_or_invalid_scan_boundaries() {
        let mut input = vec![0xff, 0xd8];

        segment(&mut input, 0xda, &[1, 1, 0, 0, 0, 0]);
        input.extend_from_slice(&[0x23, 0xff]);

        let mut reader = JpegEventReader::new(&input).unwrap();

        assert_eq!(
            reader.next_event().unwrap_err().kind,
            JpegErrorKind::Truncated
        );

        // The reader does not continue after an error.
        assert!(reader.next_event().unwrap().is_none());

        input.extend_from_slice(&[0xff, 0x00]);

        let mut reader = JpegEventReader::new(&input).unwrap();

        assert_eq!(
            reader.next_event().unwrap_err().kind,
            JpegErrorKind::InvalidMarker
        );

        assert!(JpegEventReader::new(b"no jpeg").is_err());
    }

    #[test]
    fn lists_scans_from_wpt_progressive_jpeg() {
        // The file comes from WPT; this is not a WPT browser test.
        let input = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/images/computer.jpg"
        ));

        let mut reader = JpegEventReader::new(input).unwrap();
        let mut scans = Vec::new();
        let mut dht_count = 0;

        while let Some(event) = reader.next_event().unwrap() {
            match event {
                JpegEvent::Scan {
                    header,
                    entropy_end,
                } => {
                    assert!(header.entropy_offset < entropy_end);

                    scans.push((
                        header.components.len(),
                        header.spectral_start,
                        header.spectral_end,
                        header.approximation_high,
                        header.approximation_low,
                    ));
                }

                JpegEvent::Segment { marker: 0xc4, .. } => {
                    dht_count += 1;
                }

                _ => {}
            }
        }

        assert_eq!(
            scans,
            [
                (3, 0, 0, 0, 0),
                (1, 1, 63, 0, 0),
                (1, 1, 63, 0, 0),
                (1, 1, 63, 0, 0)
            ]
        );

        assert_eq!(dht_count, 4);
    }
}
