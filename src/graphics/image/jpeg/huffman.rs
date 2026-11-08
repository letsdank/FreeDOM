//! JPEG Huffman table specifications before the first scan.
//!
//! ITU-T T.81, sections B.2.4.2 and C.2:
//! <https://www.w3.org/Graphics/JPEG/itu-t81.pdf>

use super::{EntropyError, EntropyReader, HeaderSegments, JpegError, JpegErrorKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HuffmanBuildError {
    Empty,
    TooManySymbols,
    CountMismatch,
    InvalidCodeLengths,
}

impl std::fmt::Display for HuffmanBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid JPEG Huffman table: {self:?}")
    }
}

impl std::error::Error for HuffmanBuildError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HuffmanDecodeError {
    Entropy(EntropyError),
    InvalidCode,
}

impl std::fmt::Display for HuffmanDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JPEG Huffman decode error: {self:?}")
    }
}

impl std::error::Error for HuffmanDecodeError {}

/// Canonical JPEG Huffman codes, indexed by code length.
pub struct HuffmanDecoder {
    counts: [u8; 16],
    first_codes: [u32; 16],
    first_symbols: [usize; 16],
    symbols: Vec<u8>,
}

impl HuffmanDecoder {
    pub fn new(table: &HuffmanTable) -> Result<Self, HuffmanBuildError> {
        let count: usize = table.counts.iter().map(|&value| usize::from(value)).sum();

        if count == 0 {
            return Err(HuffmanBuildError::Empty);
        }

        if count > 256 {
            return Err(HuffmanBuildError::TooManySymbols);
        }

        if count != table.symbols.len() {
            return Err(HuffmanBuildError::CountMismatch);
        }

        let mut first_codes = [0; 16];
        let mut first_symbols = [0; 16];

        let mut code = 0u32;
        let mut symbol = 0usize;

        for index in 0..16 {
            let amount = u32::from(table.counts[index]);

            if code + amount >= (1u32 << (index + 1)) {
                return Err(HuffmanBuildError::InvalidCodeLengths);
            }

            first_codes[index] = code;
            first_symbols[index] = symbol;

            code = (code + amount) << 1;
            symbol += amount as usize;
        }

        Ok(Self {
            counts: table.counts,
            first_codes,
            first_symbols,
            symbols: table.symbols.clone(),
        })
    }

    /// Bits consumed before an error remain consumed.
    pub fn decode(&self, reader: &mut EntropyReader<'_>) -> Result<u8, HuffmanDecodeError> {
        let mut code = 0u32;

        for index in 0..16 {
            let bit = reader.read_bit().map_err(HuffmanDecodeError::Entropy)?;

            code = (code << 1) | u32::from(bit);

            let first = self.first_codes[index];

            if code >= first && code - first < u32::from(self.counts[index]) {
                let symbol_index = self.first_symbols[index] + (code - first) as usize;

                return Ok(self.symbols[symbol_index]);
            }
        }

        Err(HuffmanDecodeError::InvalidCode)
    }
}

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

impl HuffmanTables {
    /// Applies one DHT payload; errors are relative to the payload.
    ///
    /// Existing tables not defined by this payload remain unchanged.
    pub fn update_from_dht(&mut self, data: &[u8]) -> Result<(), JpegError> {
        let parsed = parse_dht_segment(data)?;

        for (destination, table) in self.dc.iter_mut().zip(parsed.dc) {
            if table.is_some() {
                *destination = table;
            }
        }

        for (destination, table) in self.ac.iter_mut().zip(parsed.ac) {
            if table.is_some() {
                *destination = table;
            }
        }

        Ok(())
    }
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

        tables.update_from_dht(segment.data).map_err(|mut error| {
            error.offset += segment.data_offset;
            error
        })?;
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

    #[test]
    fn decodes_canonical_codes_from_wpt_table() {
        let bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/images/computer.jpg"
        ));

        let tables = read_huffman_tables(bytes).unwrap();

        let decoder = HuffmanDecoder::new(tables.dc[0].as_ref().unwrap()).unwrap();

        // Codes: 0, 10, 110, 1110, 11110 and 111110.
        let mut reader = EntropyReader::new(&[0x5b, 0xbd, 0xf0]);

        for expected in 0..6 {
            assert_eq!(decoder.decode(&mut reader).unwrap(), expected);
        }
    }

    #[test]
    fn builds_all_baseline_wpt_tables() {
        let bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/images/arrow-oriented-upright.jpg"
        ));

        let frame = super::super::read_frame_header(bytes).unwrap();

        assert_eq!(
            (frame.kind, frame.width, frame.height),
            (super::super::JpegFrameKind::Baseline, 240, 144)
        );

        let tables = read_huffman_tables(bytes).unwrap();

        assert!(tables.dc.iter().any(Option::is_none));
        assert!(tables.ac.iter().any(Option::is_none));

        for table in tables.dc.iter().chain(&tables.ac).flatten() {
            HuffmanDecoder::new(table).unwrap();
        }
    }

    #[test]
    fn validates_manually_constructed_tables_and_invalid_codes() {
        let mut table = HuffmanTable {
            counts: [0; 16],
            symbols: vec![],
        };

        assert!(matches!(
            HuffmanDecoder::new(&table),
            Err(HuffmanBuildError::Empty)
        ));

        table.counts[0] = 1;

        assert!(matches!(
            HuffmanDecoder::new(&table),
            Err(HuffmanBuildError::CountMismatch)
        ));

        table.symbols.push(3);

        let decoder = HuffmanDecoder::new(&table).unwrap();

        assert_eq!(
            decoder.decode(&mut EntropyReader::new(&[0x80, 0x00])),
            Err(HuffmanDecodeError::InvalidCode)
        );

        table.counts[0] = 2;
        table.symbols.push(4);

        assert!(matches!(
            HuffmanDecoder::new(&table),
            Err(HuffmanBuildError::InvalidCodeLengths)
        ));
    }
}
