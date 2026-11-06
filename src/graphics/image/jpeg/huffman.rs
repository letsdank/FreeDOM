//! JPEG Huffman table specifications before the first scan.
//!
//! ITU-T T.81, sections B.2.4.2 and C.2:
//! <https://www.w3.org/Graphics/JPEG/itu-t81.pdf>

use super::{HeaderSegments, JpegError, JpegErrorKind};

/// JPEG Huffman table specification; codes are constructed separately.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HuffmanTable {
    /// Number of codes for lengths 1 through 16.
    pub counts: [u8; 16],

    /// Symbol values in increasing code-length order.
    pub symbols: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HuffmanTables {
    pub dc: [Option<HuffmanTable>; 4],
    pub ac: [Option<HuffmanTable>; 4],
}

/// Reads one DHT payload; error offsets are relative to the payload.
///
/// Definitions with the same class and identifier replace earlier ones.
pub fn parse_dht_segment(data: &[u8]) -> Result<HuffmanTables, JpegError> {
    let error = |offset, kind| JpegError { offset, kind };

    if data.is_empty() {
        return Err(error(0, JpegErrorKind::InvalidHuffmanTable));
    }

    let mut tables = HuffmanTables::default();
    let mut cursor = 0;

    while cursor < data.len() {
        let info_offset = cursor;
        let info = data[cursor];

        cursor += 1;

        let class = info >> 4;
        let id = info & 0x0f;

        if class > 1 {
            return Err(error(
                info_offset,
                JpegErrorKind::InvalidHuffmanClass(class),
            ));
        }

        if id > 3 {
            return Err(error(info_offset, JpegErrorKind::InvalidHuffmanTableId(id)));
        }

        let end = cursor
            .checked_add(16)
            .ok_or_else(|| error(cursor, JpegErrorKind::Truncated))?;

        let bytes = data
            .get(cursor..end)
            .ok_or_else(|| error(cursor, JpegErrorKind::Truncated))?;

        let mut counts = [0u8; 16];
        counts.copy_from_slice(bytes);

        let symbol_count: usize = counts.iter().map(|&count| usize::from(count)).sum();

        if symbol_count == 0 {
            return Err(error(cursor, JpegErrorKind::InvalidHuffmanTable));
        }

        if symbol_count > 256 {
            return Err(error(cursor, JpegErrorKind::TooManyHuffmanSymbols));
        }

        // T.81 Annex C reserves the all-ones code at every length.
        let mut available = 1i32;

        for (index, &count) in counts.iter().enumerate() {
            available = available * 2 - i32::from(count);

            if available <= 0 {
                return Err(error(
                    cursor + index,
                    JpegErrorKind::InvalidHuffmanCodeLengths,
                ));
            }
        }

        cursor = end;

        let end = cursor
            .checked_add(symbol_count)
            .ok_or_else(|| error(cursor, JpegErrorKind::Truncated))?;

        let symbols = data
            .get(cursor..end)
            .ok_or_else(|| error(cursor, JpegErrorKind::Truncated))?
            .to_vec();

        let target = if class == 0 {
            &mut tables.dc
        } else {
            &mut tables.ac
        };

        target[usize::from(id)] = Some(HuffmanTable { counts, symbols });

        cursor = end;
    }

    Ok(tables)
}

/// Reads DHT definitions before the first SOS or EOI.
///
/// Definitions between later scans are not read.
pub fn read_huffman_tables(input: &[u8]) -> Result<HuffmanTables, JpegError> {
    let mut segments = HeaderSegments::new(input)?;
    let mut tables = HuffmanTables::default();

    while let Some(segment) = segments.next_segment()? {
        if segment.marker != 0xc4 {
            continue;
        }

        let parsed = parse_dht_segment(segment.data).map_err(|mut error| {
            error.offset += segment.data_offset;
            error
        })?;

        for (destination, table) in tables.dc.iter_mut().zip(parsed.dc) {
            if table.is_some() {
                *destination = table;
            }
        }

        for (destination, table) in tables.ac.iter_mut().zip(parsed.ac) {
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

    fn dht(info: u8, symbol: u8) -> Vec<u8> {
        let mut data = vec![info, 1];

        data.extend([0; 15]);
        data.push(symbol);

        data
    }

    #[test]
    fn reads_multiple_dht_tables_and_redefinitions() {
        let mut data = dht(0x00, 4);

        data.extend(dht(0x12, 5));
        data.extend(dht(0x00, 6));

        let tables = parse_dht_segment(&data).unwrap();

        assert_eq!(tables.dc[0].as_ref().unwrap().symbols, [6]);
        assert_eq!(tables.ac[2].as_ref().unwrap().symbols, [5]);
        assert_eq!(tables.dc[0].as_ref().unwrap().counts[0], 1);

        assert!(tables.dc[1].is_none());
    }

    #[test]
    fn rejects_invalid_dht_data() {
        assert_eq!(
            parse_dht_segment(&[]).unwrap_err().kind,
            JpegErrorKind::InvalidHuffmanTable
        );

        assert_eq!(
            parse_dht_segment(&[0x20]).unwrap_err().kind,
            JpegErrorKind::InvalidHuffmanClass(2)
        );

        assert_eq!(
            parse_dht_segment(&[0x04]).unwrap_err().kind,
            JpegErrorKind::InvalidHuffmanTableId(4)
        );

        assert_eq!(
            parse_dht_segment(&[0x00]).unwrap_err().kind,
            JpegErrorKind::Truncated
        );

        let mut data = dht(0, 7);
        data.pop();

        assert_eq!(
            parse_dht_segment(&data).unwrap_err().kind,
            JpegErrorKind::Truncated
        );

        // Two one-bit codes would include the reserved all-one code.
        let mut data = dht(0, 7);
        data[1] = 2;
        data.push(8);

        assert_eq!(
            parse_dht_segment(&data).unwrap_err().kind,
            JpegErrorKind::InvalidHuffmanCodeLengths
        );

        let mut data = vec![0];
        data.extend([255, 2]);
        data.extend([0; 14]);

        assert_eq!(
            parse_dht_segment(&data).unwrap_err().kind,
            JpegErrorKind::TooManyHuffmanSymbols
        );
    }

    #[test]
    fn reads_first_scan_huffman_tables_from_wpt_jpeg() {
        // This checks a WPT asset, not the original browser test.
        let bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/images/computer.jpg"
        ));

        let tables = read_huffman_tables(bytes).unwrap();

        assert_eq!(tables.dc[0].as_ref().unwrap().symbols, [0, 1, 2, 3, 4, 5]);
        assert_eq!(tables.dc[1].as_ref().unwrap().symbols, [0, 1]);

        assert!(tables.ac.iter().all(Option::is_none));
    }

    #[test]
    fn reads_dht_before_scan_and_reports_absolute_errors() {
        let payload = dht(0, 7);

        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xc4, 0, 20];

        jpeg.extend(&payload);
        jpeg.extend([0xff, 0xda]);

        let tables = read_huffman_tables(&jpeg).unwrap();

        assert_eq!(tables.dc[0].as_ref().unwrap().symbols, [7]);

        jpeg[6] = 0x20;

        let error = read_huffman_tables(&jpeg).unwrap_err();

        assert_eq!(error.offset, 6);
        assert_eq!(error.kind, JpegErrorKind::InvalidHuffmanClass(2));
    }
}
