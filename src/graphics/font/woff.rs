use std::{io::Read, ops::Range};

use flate2::read::ZlibDecoder;

use super::{
    sfnt::{FontError, TableTag},
    woff2::decode_woff2,
};

const WOFF_SIGNATURE: u32 = 0x774F_4646;
const WOFF_HEADER_LENGTH: usize = 44;
const WOFF_TABLE_DIRECTORY_ENTRY_LENGTH: usize = 20;
const SFNT_HEADER_LENGTH: usize = 12;
const SFNT_TABLE_DIRECTORY_ENTRY_LENGTH: usize = 16;
const MAX_SFNT_TABLE_COUNT: usize = 4095;

#[derive(Clone, Debug)]
struct WoffTable {
    tag: TableTag,
    checksum: u32,
    source_offset: usize,
    compressed_length: usize,
    original_length: usize,
}

#[derive(Clone, Debug)]
struct DecodedTable {
    tag: TableTag,
    checksum: u32,
    data: Vec<u8>,
}

/// Decodes a WOFF 1.0 container into an SFNT byte buffer.
pub fn decode_woff(data: &[u8]) -> Result<Vec<u8>, FontError> {
    if data.get(..4) == Some(b"wOF2") {
        return decode_woff2(data);
    }

    if read_u32(data, 0)? != WOFF_SIGNATURE {
        return Err(error(0, "invalid WOFF signature"));
    }

    let flavor = read_u32(data, 4)?;
    let declared_length = to_usize(read_u32(data, 8)?, 8)?;

    if declared_length != data.len() {
        return Err(error(8, "WOFF length does not match the input size"));
    }

    let table_count = read_u16(data, 12)? as usize;

    if table_count > MAX_SFNT_TABLE_COUNT {
        return Err(error(12, "WOFF contains too many tables for an SFNT font"));
    }

    if read_u16(data, 14)? != 0 {
        return Err(error(14, "WOFF reserved header field must be zero"));
    }

    let total_sfnt_size = to_usize(read_u32(data, 16)?, 16)?;

    if total_sfnt_size % 4 != 0 {
        return Err(error(16, "WOFF totalSfntSize must be a multiple of four"));
    }

    let directory_length = table_count
        .checked_mul(WOFF_TABLE_DIRECTORY_ENTRY_LENGTH)
        .and_then(|length| length.checked_add(WOFF_HEADER_LENGTH))
        .ok_or_else(|| error(12, "WOFF table directory is too large"))?;

    checked_range(data, 0, directory_length, "truncated WOFF table directory")?;

    let mut occupied_ranges = vec![(0..directory_length, "WOFF header or table directory")];
    let mut tables = Vec::with_capacity(table_count);

    for index in 0..table_count {
        let entry_offset = WOFF_HEADER_LENGTH + index * WOFF_TABLE_DIRECTORY_ENTRY_LENGTH;

        let tag = TableTag::from_bytes([
            data[entry_offset],
            data[entry_offset + 1],
            data[entry_offset + 2],
            data[entry_offset + 3],
        ]);

        let source_offset = to_usize(read_u32(data, entry_offset + 4)?, entry_offset + 4)?;
        let compressed_length = to_usize(read_u32(data, entry_offset + 8)?, entry_offset + 8)?;
        let original_length = to_usize(read_u32(data, entry_offset + 12)?, entry_offset + 12)?;
        let checksum = read_u32(data, entry_offset + 16)?;

        if compressed_length > original_length {
            return Err(error(
                entry_offset + 8,
                "WOFF compressed table is larger than its original table",
            ));
        }

        if source_offset % 4 != 0 {
            return Err(error(
                entry_offset + 4,
                "WOFF table offset must be aligned to four bytes",
            ));
        }

        if source_offset < directory_length {
            return Err(error(
                entry_offset + 4,
                "WOFF table overlaps the header or table directory",
            ));
        }

        let range = checked_range(
            data,
            source_offset,
            compressed_length,
            "WOFF table extends beyond the input",
        )?;

        add_occupied_range(&mut occupied_ranges, range, "WOFF table data")?;

        tables.push(WoffTable {
            tag,
            checksum,
            source_offset,
            compressed_length,
            original_length,
        });
    }

    validate_optional_blocks(data, &mut occupied_ranges)?;

    let mut decoded_tables = Vec::with_capacity(tables.len());

    for table in tables {
        let compressed = checked_slice(data, table.source_offset, table.compressed_length)?;

        let table_data = if table.compressed_length == table.original_length {
            compressed.to_vec()
        } else {
            decompress_table(compressed, table.original_length, table.source_offset)?
        };

        decoded_tables.push(DecodedTable {
            tag: table.tag,
            checksum: table.checksum,
            data: table_data,
        });
    }

    decoded_tables.sort_by_key(|table| table.tag.bytes());

    for pair in decoded_tables.windows(2) {
        if pair[0].tag == pair[1].tag {
            return Err(error(0, "WOFF contains duplicate table tags"));
        }
    }

    let expected_sfnt_size = decoded_tables.iter().try_fold(
        SFNT_HEADER_LENGTH + table_count * SFNT_TABLE_DIRECTORY_ENTRY_LENGTH,
        |size, table| {
            size.checked_add(padded_length(table.data.len())?)
                .ok_or_else(|| error(16, "decoded SFNT size overflows"))
        },
    )?;

    if expected_sfnt_size != total_sfnt_size {
        return Err(error(
            16,
            "WOFF totalSfntSize does not match decoded table sizes",
        ));
    }

    let mut sfnt = vec![0; total_sfnt_size];

    write_u32(&mut sfnt, 0, flavor);
    write_u16(&mut sfnt, 4, table_count as u16);

    let (search_range, entry_selector, range_shift) = sfnt_search_parameters(table_count);
    write_u16(&mut sfnt, 6, search_range);
    write_u16(&mut sfnt, 8, entry_selector);
    write_u16(&mut sfnt, 10, range_shift);

    let mut table_offset = SFNT_HEADER_LENGTH + table_count * SFNT_TABLE_DIRECTORY_ENTRY_LENGTH;

    for (index, table) in decoded_tables.iter().enumerate() {
        let record_offset = SFNT_HEADER_LENGTH + index * SFNT_TABLE_DIRECTORY_ENTRY_LENGTH;
        let tag = table.tag.bytes();

        sfnt[record_offset..record_offset + 4].copy_from_slice(&tag);
        write_u32(&mut sfnt, record_offset + 4, table.checksum);
        write_u32(
            &mut sfnt,
            record_offset + 8,
            u32::try_from(table_offset)
                .map_err(|_| error(record_offset + 8, "SFNT table offset is too large"))?,
        );
        write_u32(
            &mut sfnt,
            record_offset + 12,
            u32::try_from(table.data.len())
                .map_err(|_| error(record_offset + 12, "SFNT table length is too large"))?,
        );

        let table_end = table_offset + table.data.len();
        sfnt[table_offset..table_end].copy_from_slice(&table.data);
        table_offset = padded_length(table_end)?;
    }

    Ok(sfnt)
}

/// Converts a supported web-font container to SFNT data.
///
/// SFNT input is returned unchanged. WOFF 1.0 input is decoded into SFNT.
pub fn normalize_font_container(data: Vec<u8>) -> Result<Vec<u8>, FontError> {
    match data.get(..4) {
        Some(signature) if signature == b"wOFF" || signature == b"wOF2" => decode_woff(&data),
        _ => Ok(data),
    }
}

fn decompress_table(
    compressed: &[u8],
    original_length: usize,
    source_offset: usize,
) -> Result<Vec<u8>, FontError> {
    let maximum_output_length = u64::try_from(
        original_length
            .checked_add(1)
            .ok_or_else(|| error(source_offset, "WOFF table length overflows"))?,
    )
    .map_err(|_| error(source_offset, "WOFF table length is too large"))?;

    let mut decoder = ZlibDecoder::new(compressed).take(maximum_output_length);
    let mut decoded = Vec::with_capacity(original_length);

    decoder
        .read_to_end(&mut decoded)
        .map_err(|_| error(source_offset, "cannot decompress WOFF table"))?;

    if decoded.len() != original_length {
        return Err(error(
            source_offset,
            "decompressed WOFF table has an unexpected length",
        ));
    }

    Ok(decoded)
}

fn validate_optional_blocks(
    data: &[u8],
    occupied_ranges: &mut Vec<(Range<usize>, &'static str)>,
) -> Result<(), FontError> {
    let metadata_offset = to_usize(read_u32(data, 24)?, 24)?;
    let metadata_length = to_usize(read_u32(data, 28)?, 28)?;
    let metadata_original_length = to_usize(read_u32(data, 32)?, 32)?;

    match (metadata_offset, metadata_length, metadata_original_length) {
        (0, 0, 0) => {}
        (offset, length, original_length) if offset > 0 && length > 0 && original_length > 0 => {
            let range = checked_range(data, offset, length, "WOFF metadata extends beyond input")?;
            add_occupied_range(occupied_ranges, range, "WOFF metadata")?;
        }
        _ => return Err(error(24, "invalid WOFF metadata block")),
    }

    let private_offset = to_usize(read_u32(data, 36)?, 36)?;
    let private_length = to_usize(read_u32(data, 40)?, 40)?;

    match (private_offset, private_length) {
        (0, 0) => {}
        (offset, length) if offset > 0 && length > 0 => {
            let range = checked_range(
                data,
                offset,
                length,
                "WOFF private data extends beyond input",
            )?;
            add_occupied_range(occupied_ranges, range, "WOFF private data")?;
        }
        _ => return Err(error(36, "invalid WOFF private data block")),
    }

    Ok(())
}

fn add_occupied_range(
    ranges: &mut Vec<(Range<usize>, &'static str)>,
    range: Range<usize>,
    name: &'static str,
) -> Result<(), FontError> {
    if let Some((_, other_name)) = ranges
        .iter()
        .find(|(other, _)| ranges_overlap(other, &range))
    {
        return Err(error(range.start, format!("{name} overlaps {other_name}")));
    }

    ranges.push((range, name));
    Ok(())
}

fn ranges_overlap(left: &Range<usize>, right: &Range<usize>) -> bool {
    left.start < right.end && right.start < left.end
}

fn sfnt_search_parameters(table_count: usize) -> (u16, u16, u16) {
    if table_count == 0 {
        return (0, 0, 0);
    }

    let mut largest_power_of_two = 1_usize;
    let mut entry_selector = 0_u16;

    while largest_power_of_two * 2 <= table_count {
        largest_power_of_two *= 2;
        entry_selector += 1;
    }

    let search_range = largest_power_of_two * SFNT_TABLE_DIRECTORY_ENTRY_LENGTH;
    let range_shift = table_count * SFNT_TABLE_DIRECTORY_ENTRY_LENGTH - search_range;

    (search_range as u16, entry_selector, range_shift as u16)
}

fn padded_length(length: usize) -> Result<usize, FontError> {
    length
        .checked_add(3)
        .map(|length| length & !3)
        .ok_or_else(|| error(length, "font length overflows"))
}

fn checked_slice(data: &[u8], offset: usize, length: usize) -> Result<&[u8], FontError> {
    let range = checked_range(data, offset, length, "unexpected end of font file")?;
    Ok(&data[range])
}

fn checked_range(
    data: &[u8],
    offset: usize,
    length: usize,
    message: &'static str,
) -> Result<Range<usize>, FontError> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| error(offset, "font offset overflows"))?;

    if end > data.len() {
        return Err(error(offset, message));
    }

    Ok(offset..end)
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16, FontError> {
    let bytes = checked_slice(data, offset, 2)?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, FontError> {
    let bytes = checked_slice(data, offset, 4)?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn write_u16(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

fn write_u32(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn to_usize(value: u32, offset: usize) -> Result<usize, FontError> {
    usize::try_from(value).map_err(|_| error(offset, "font value is too large"))
}

fn error(offset: usize, message: impl Into<String>) -> FontError {
    FontError {
        offset,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::{Compression, write::ZlibEncoder};

    use super::*;
    use crate::graphics::font::sfnt::SfntFont;

    #[test]
    fn decodes_uncompressed_woff_tables() {
        let original_table = vec![0_u8; 32];
        let woff = woff_with_table(&original_table, false);

        let sfnt_data = decode_woff(&woff).unwrap();
        let font = SfntFont::parse(&sfnt_data).unwrap();

        assert_eq!(font.table(TableTag::NAME), Some(original_table.as_slice()));
        assert_eq!(sfnt_data.len(), 60);
    }

    #[test]
    fn decodes_zlib_compressed_woff_tables() {
        let original_table = vec![0_u8; 32];
        let woff = woff_with_table(&original_table, true);

        let sfnt_data = decode_woff(&woff).unwrap();
        let font = SfntFont::parse(&sfnt_data).unwrap();

        assert_eq!(font.table(TableTag::NAME), Some(original_table.as_slice()));
    }

    #[test]
    fn rejects_woff_tables_that_overlap_the_directory() {
        let original_table = vec![0_u8; 32];
        let mut woff = woff_with_table(&original_table, false);

        woff[48..52].copy_from_slice(&44_u32.to_be_bytes());

        let error = decode_woff(&woff).unwrap_err();

        assert_eq!(
            error.message,
            "WOFF table overlaps the header or table directory"
        );
    }

    #[test]
    fn preserves_sfnt_container_bytes() {
        let sfnt = vec![0x00, 0x01, 0x00, 0x00];

        assert_eq!(normalize_font_container(sfnt.clone()).unwrap(), sfnt);
    }

    #[test]
    fn normalizes_woff_containers() {
        let original_table = vec![0_u8; 32];
        let woff = woff_with_table(&original_table, true);

        let sfnt_data = normalize_font_container(woff).unwrap();
        let font = SfntFont::parse(&sfnt_data).unwrap();

        assert_eq!(font.table(TableTag::NAME), Some(original_table.as_slice()));
    }

    fn woff_with_table(table: &[u8], compress: bool) -> Vec<u8> {
        let encoded = if compress {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(table).unwrap();
            encoder.finish().unwrap()
        } else {
            table.to_vec()
        };

        assert!(!compress || encoded.len() < table.len());

        let table_offset = WOFF_HEADER_LENGTH + WOFF_TABLE_DIRECTORY_ENTRY_LENGTH;
        let file_length = padded_length(table_offset + encoded.len()).unwrap();
        let total_sfnt_size = SFNT_HEADER_LENGTH
            + SFNT_TABLE_DIRECTORY_ENTRY_LENGTH
            + padded_length(table.len()).unwrap();

        let mut data = vec![0_u8; file_length];

        write_u32(&mut data, 0, WOFF_SIGNATURE);
        write_u32(&mut data, 4, 0x0001_0000);
        write_u32(&mut data, 8, file_length as u32);
        write_u16(&mut data, 12, 1);
        write_u32(&mut data, 16, total_sfnt_size as u32);
        write_u16(&mut data, 20, 1);

        data[44..48].copy_from_slice(b"name");
        write_u32(&mut data, 48, table_offset as u32);
        write_u32(&mut data, 52, encoded.len() as u32);
        write_u32(&mut data, 56, table.len() as u32);
        write_u32(&mut data, 60, 0);

        data[table_offset..table_offset + encoded.len()].copy_from_slice(&encoded);
        data
    }
}
