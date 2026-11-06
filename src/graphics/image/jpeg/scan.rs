//! JPEG scan headers and baseline table selection.
//!
//! ITU-T T.81, section B.2.3:
//! <https://www.w3.org/Graphics/JPEG/itu-t81.pdf>
//!
//! This module does not decode scan data or parse scans after the first.

use super::{
    HeaderSegments, HuffmanTables, JpegError, JpegErrorKind, JpegFrameHeader, JpegFrameKind,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanComponent {
    pub id: u8,
    pub dc_table: u8,
    pub ac_table: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanHeader {
    pub components: Vec<ScanComponent>,
    pub spectral_start: u8,
    pub spectral_end: u8,
    pub approximation_high: u8,
    pub approximation_low: u8,

    /// Absolute byte offset of the entropy-coded data in the original input.
    pub entropy_offset: usize,
}

/// Reads the first SOS header, but does not validate or decode scan data.
pub fn read_first_scan_header(input: &[u8]) -> Result<ScanHeader, JpegError> {
    let error = |offset, kind| JpegError { offset, kind };

    let mut segments = HeaderSegments::new(input)?;

    while segments.next_segment()?.is_some() {}

    let marker_offset = segments
        .end_marker_offset
        .expect("SOS or EOI was encountered");

    if input.get(segments.cursor - 1) != Some(&0xda) {
        return Err(error(marker_offset, JpegErrorKind::MissingScanHeader));
    }

    let cursor = segments.cursor;

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

    let count = usize::from(
        *data
            .first()
            .ok_or_else(|| error(marker_offset, JpegErrorKind::InvalidScanHeader))?,
    );

    if !(1..=4).contains(&count) || data.len() != 1 + 2 * count + 3 {
        return Err(error(marker_offset, JpegErrorKind::InvalidScanHeader));
    }

    let mut components = Vec::with_capacity(count);

    for item in data[1..1 + 2 * count].chunks_exact(2) {
        let component = ScanComponent {
            id: item[0],
            dc_table: item[1] >> 4,
            ac_table: item[1] & 0x0f,
        };

        if component.dc_table > 3
            || component.ac_table > 3
            || components
                .iter()
                .any(|old: &ScanComponent| old.id == component.id)
        {
            return Err(error(marker_offset, JpegErrorKind::InvalidScanHeader));
        }

        components.push(component);
    }

    let offset = 1 + count * 2;

    let spectral_start = data[offset];
    let spectral_end = data[offset + 1];

    let approximation_high = data[offset + 2] >> 4;
    let approximation_low = data[offset + 2] & 0x0f;

    if spectral_start > spectral_end
        || spectral_end > 63
        || approximation_high > 13
        || approximation_low > 13
    {
        return Err(error(marker_offset, JpegErrorKind::InvalidScanHeader));
    }

    Ok(ScanHeader {
        components,
        spectral_start,
        spectral_end,
        approximation_high,
        approximation_low,
        entropy_offset: end,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BaselineScanError {
    UnsupportedFrame,
    InvalidParameters,
    InvalidComponentCount,
    UnknownComponent(u8),
    ComponentOrder,
    InvalidTableSelector(u8),
    MissingDcTable(u8),
    MissingAcTable(u8),
}

impl std::fmt::Display for BaselineScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid baseline JPEG scan: {self:?}")
    }
}

impl std::error::Error for BaselineScanError {}

/// Validates a baseline scan against its frame and current Huffman tables.
///
/// Quantization tables and entropy-coded data are not checked here.
pub fn validate_baseline_scan(
    frame: &JpegFrameHeader,
    scan: &ScanHeader,
    tables: &HuffmanTables,
) -> Result<(), BaselineScanError> {
    if frame.kind != JpegFrameKind::Baseline {
        return Err(BaselineScanError::UnsupportedFrame);
    }

    if (
        scan.spectral_start,
        scan.spectral_end,
        scan.approximation_high,
        scan.approximation_low,
    ) != (0, 63, 0, 0)
    {
        return Err(BaselineScanError::InvalidParameters);
    }

    if scan.components.is_empty()
        || scan.components.len() > 4
        || scan.components.len() > frame.components.len()
    {
        return Err(BaselineScanError::InvalidComponentCount);
    }

    let mut previous_index = None;

    for component in &scan.components {
        let index = frame
            .components
            .iter()
            .position(|item| item.id == component.id)
            .ok_or(BaselineScanError::UnknownComponent(component.id))?;

        if previous_index.is_some_and(|previous| index <= previous) {
            return Err(BaselineScanError::ComponentOrder);
        }

        previous_index = Some(index);

        // Baseline JPEG allows Huffman table identifiers 0 and 1.
        if component.dc_table > 1 {
            return Err(BaselineScanError::InvalidTableSelector(component.dc_table));
        }

        if component.ac_table > 1 {
            return Err(BaselineScanError::InvalidTableSelector(component.ac_table));
        }

        if tables.dc[component.dc_table as usize].is_none() {
            return Err(BaselineScanError::MissingDcTable(component.dc_table));
        }

        if tables.ac[component.ac_table as usize].is_none() {
            return Err(BaselineScanError::MissingAcTable(component.ac_table));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{EntropyReader, HuffmanDecoder, read_frame_header, read_huffman_tables};
    use super::*;

    const BASELINE: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/wpt/images/arrow-oriented-upright.jpg"
    ));

    const PROGRESSIVE: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/wpt/images/computer.jpg"
    ));

    #[test]
    fn connects_wpt_baseline_scan_to_frame_tables_and_entropy_reader() {
        // This checks a WPT asset, not the original browser test.
        let frame = read_frame_header(BASELINE).unwrap();
        let scan = read_first_scan_header(BASELINE).unwrap();
        let tables = read_huffman_tables(BASELINE).unwrap();

        assert_eq!(scan.components.len(), 3);

        assert_eq!(
            (
                scan.spectral_start,
                scan.spectral_end,
                scan.approximation_high,
                scan.approximation_low
            ),
            (0, 63, 0, 0)
        );

        validate_baseline_scan(&frame, &scan, &tables).unwrap();

        let decoder = HuffmanDecoder::new(
            tables.dc[scan.components[0].dc_table as usize]
                .as_ref()
                .unwrap(),
        )
        .unwrap();

        let mut reader = EntropyReader::new(&BASELINE[scan.entropy_offset..]);
        let category = decoder.decode(&mut reader).unwrap();

        assert_eq!(category, 11);
        assert_eq!(reader.read_bits(category).unwrap(), 1023);
    }

    #[test]
    fn reads_progressive_header_but_does_not_claim_baseline_support() {
        let frame = read_frame_header(PROGRESSIVE).unwrap();
        let scan = read_first_scan_header(PROGRESSIVE).unwrap();
        let tables = read_huffman_tables(PROGRESSIVE).unwrap();

        assert_eq!(scan.spectral_start, 0);
        assert_eq!(scan.spectral_end, 0);

        assert_eq!(
            validate_baseline_scan(&frame, &scan, &tables),
            Err(BaselineScanError::UnsupportedFrame)
        );
    }

    #[test]
    fn rejects_wrong_component_order_and_missing_tables() {
        let frame = read_frame_header(BASELINE).unwrap();
        let mut scan = read_first_scan_header(BASELINE).unwrap();
        let mut tables = read_huffman_tables(BASELINE).unwrap();

        scan.components.swap(0, 1);

        assert_eq!(
            validate_baseline_scan(&frame, &scan, &tables),
            Err(BaselineScanError::ComponentOrder)
        );

        scan.components.swap(0, 1);

        let id = scan.components[0].dc_table;
        tables.dc[id as usize] = None;

        assert_eq!(
            validate_baseline_scan(&frame, &scan, &tables),
            Err(BaselineScanError::MissingDcTable(id))
        );
    }

    #[test]
    fn rejects_truncated_and_malformed_headers() {
        assert_eq!(
            read_first_scan_header(&[0xff, 0xd8, 0xff, 0xd9])
                .unwrap_err()
                .kind,
            JpegErrorKind::MissingScanHeader
        );

        assert_eq!(
            read_first_scan_header(&[0xff, 0xd8, 0xff, 0xda, 0])
                .unwrap_err()
                .kind,
            JpegErrorKind::Truncated
        );

        let duplicate = [0xff, 0xd8, 0xff, 0xda, 0, 10, 2, 1, 0, 1, 0, 0, 63, 0];

        assert_eq!(
            read_first_scan_header(&duplicate).unwrap_err().kind,
            JpegErrorKind::InvalidScanHeader
        );

        for len in 0..duplicate.len() {
            assert!(read_first_scan_header(&duplicate[..len]).is_err());
        }
    }
}
