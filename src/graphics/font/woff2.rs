//! WOFF 2.0 directory parsing and SFNT reconstruction.
//!
//! The decoder supports Brotli-compressed table data and reconstruction
//! of transformed glyf and loca tables.
//!
//! Font collections and the hmtx table transform are not supported yet.
//!
//! Specification: <https://www.w3.org/TR/WOFF2/>

mod glyf;

use super::{
    brotli::decode_brotli,
    reader::{
        checked_add, checked_end, checked_range, padded_length,
        read_bytes_from_start as read_bytes, read_u8, read_u16, read_u32, to_usize, write_u16,
        write_u32,
    },
    sfnt::{FontError, TableTag},
};
use std::ops::Range;

const WOFF2_SIGNATURE: u32 = 0x774f_4632;
const WOFF2_HEADER_LENGTH: usize = 48;
const MAX_TABLE_COUNT: usize = 4095;
const TTC_FLAVOR: u32 = 0x7474_6366;

const KNOWN_TABLE_TAGS: [[u8; 4]; 63] = [
    *b"cmap", *b"head", *b"hhea", *b"hmtx", *b"maxp", *b"name", *b"OS/2", *b"post", *b"cvt ",
    *b"fpgm", *b"glyf", *b"loca", *b"prep", *b"CFF ", *b"VORG", *b"EBDT", *b"EBLC", *b"gasp",
    *b"hdmx", *b"kern", *b"LTSH", *b"PCLT", *b"VDMX", *b"vhea", *b"vmtx", *b"BASE", *b"GDEF",
    *b"GPOS", *b"GSUB", *b"EBSC", *b"JSTF", *b"MATH", *b"CBDT", *b"CBLC", *b"COLR", *b"CPAL",
    *b"SVG ", *b"sbix", *b"acnt", *b"avar", *b"bdat", *b"bloc", *b"bsln", *b"cvar", *b"fdsc",
    *b"feat", *b"fmtx", *b"fvar", *b"gvar", *b"hsty", *b"just", *b"lcar", *b"mort", *b"morx",
    *b"opbd", *b"prop", *b"trak", *b"Zapf", *b"Silf", *b"Glat", *b"Gloc", *b"Feat", *b"Sill",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Woff2Transform {
    Null,
    GlyfLoca,
    Hmtx,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Woff2Table {
    pub tag: TableTag,
    pub original_length: usize,
    pub transformed_length: Option<usize>,
    pub transform: Woff2Transform,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Woff2Directory {
    pub flavor: u32,
    pub total_sfnt_size: usize,
    pub tables: Vec<Woff2Table>,
    pub compressed_data: Range<usize>,
}

pub fn parse_woff2_directory(data: &[u8]) -> Result<Woff2Directory, FontError> {
    if read_u32(data, 0, 0)? != WOFF2_SIGNATURE {
        return Err(error(0, "invalid WOFF2 signature"));
    }

    if data.len() < WOFF2_HEADER_LENGTH {
        return Err(error(0, "WOFF2 header is shorter than 48 bytes"));
    }

    let flavor = read_u32(data, 4, 0)?;
    let declared_length = to_usize(read_u32(data, 8, 0)?, 8)?;

    if declared_length != data.len() {
        return Err(error(8, "WOFF2 length does not match the input size"));
    }

    if flavor == TTC_FLAVOR {
        return Err(error(4, "WOFF2 collections are not supported yet"));
    }

    let table_count = usize::from(read_u16(data, 12, 0)?);

    if table_count > MAX_TABLE_COUNT {
        return Err(error(12, "WOFF contains too many tables"));
    }

    let total_sfnt_size = to_usize(read_u32(data, 16, 0)?, 16)?;
    let compressed_length = to_usize(read_u32(data, 20, 0)?, 20)?;

    let metadata_offset = to_usize(read_u32(data, 28, 0)?, 28)?;
    let metadata_length = to_usize(read_u32(data, 32, 0)?, 32)?;
    let metadata_original_length = to_usize(read_u32(data, 36, 0)?, 36)?;

    let private_offset = to_usize(read_u32(data, 40, 0)?, 40)?;
    let private_length = to_usize(read_u32(data, 44, 0)?, 44)?;

    let mut cursor = WOFF2_HEADER_LENGTH;
    let mut tables = Vec::with_capacity(table_count);

    for _ in 0..table_count {
        let flags = read_u8(data, cursor, 0)?;
        cursor = checked_add(cursor, 1, "WOFF2 table directory offset overflows")?;

        let known_tag_index = usize::from(flags & 0x3f);
        let transform_version = flags >> 6;

        let tag = if known_tag_index == 63 {
            let bytes = read_bytes(data, cursor, 4)?;
            cursor = checked_add(cursor, 4, "WOFF2 table directory offset overflows")?;
            TableTag::from_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        } else {
            TableTag::from_bytes(KNOWN_TABLE_TAGS[known_tag_index])
        };

        if tables.iter().any(|table: &Woff2Table| table.tag == tag) {
            return Err(error(cursor, "duplicate WOFF2 table tag"));
        }

        let original_length = read_base_128(data, &mut cursor)?;
        let transform = table_transform(tag, transform_version, cursor)?;

        let transformed_length = match transform {
            Woff2Transform::Null => None,
            Woff2Transform::GlyfLoca | Woff2Transform::Hmtx => {
                Some(read_base_128(data, &mut cursor)?)
            }
        };

        if tag.bytes() == *b"loca"
            && transform == Woff2Transform::GlyfLoca
            && transformed_length != Some(0)
        {
            return Err(error(
                cursor,
                "transformed WOFF2 loca table must have zero length",
            ));
        }

        tables.push(Woff2Table {
            tag,
            original_length,
            transformed_length,
            transform,
        });
    }

    validate_transformed_pairs(&tables, cursor)?;

    let compressed_end = checked_end(
        cursor,
        compressed_length,
        data.len(),
        "WOFF2 compressed data extends beyond input",
    )?;

    let compressed_data = cursor..compressed_end;
    let metadata = optional_metadata_range(
        data,
        metadata_offset,
        metadata_length,
        metadata_original_length,
    )?;

    let private_data = optional_private_range(data, private_offset, private_length)?;

    let mut ranges = vec![0..cursor, compressed_data.clone()];

    if let Some(range) = metadata {
        ranges.push(range);
    }

    if let Some(range) = private_data {
        ranges.push(range);
    }

    ranges.sort_by_key(|range| range.start);

    for pair in ranges.windows(2) {
        if pair[0].end > pair[1].start {
            return Err(error(pair[1].start, "WOFF2 data blocks overlap"));
        }
    }

    Ok(Woff2Directory {
        flavor,
        total_sfnt_size,
        tables,
        compressed_data,
    })
}

/// Decompresses the concatenated WOFF2 table data.
///
/// The output contains transformed data for tables that use a WOFF2
/// transform. Reconstructing those tables is a separate step.
///
/// Specification: <https://www.w3.org/TR/WOFF2/#compressed_data_format>
fn decode_table_data(data: &[u8], directory: &Woff2Directory) -> Result<Vec<u8>, FontError> {
    let expected_length = directory.tables.iter().try_fold(0_usize, |length, table| {
        let table_length = table.transformed_length.unwrap_or(table.original_length);

        length
            .checked_add(table_length)
            .ok_or_else(|| error(directory.compressed_data.start, "WOFF2 data is too large"))
    })?;

    let compressed = data.get(directory.compressed_data.clone()).ok_or_else(|| {
        error(
            directory.compressed_data.start,
            "WOFF2 compressed data extends beyond input",
        )
    })?;

    let decoded = decode_brotli(compressed, expected_length)?;

    if decoded.len() != expected_length {
        return Err(error(
            directory.compressed_data.start,
            "WOFF2 Brotli data has an unexpected length",
        ));
    }

    Ok(decoded)
}

/// Splits decompressed WOFF2 data into table slices in directory order.
///
/// Transformed tables occupy `transformed_length` bytes; other tables
/// occupy `original_length` bytes.
///
/// Specification: <https://www.w3.org/TR/WOFF2/#compressed_data_format>
fn split_table_data<'a>(
    data: &'a [u8],
    directory: &Woff2Directory,
) -> Result<Vec<(TableTag, &'a [u8])>, FontError> {
    let mut cursor = 0_usize;
    let mut tables = Vec::with_capacity(directory.tables.len());

    for table in &directory.tables {
        let length = table.transformed_length.unwrap_or(table.original_length);

        let end = checked_end(cursor, length, data.len(), "WOFF2 table data is truncated")?;

        tables.push((table.tag, &data[cursor..end]));
        cursor = end;
    }

    if cursor != data.len() {
        return Err(error(cursor, "WOFF2 table data has trailing bytes"));
    }

    Ok(tables)
}

pub fn decode_woff2(data: &[u8]) -> Result<Vec<u8>, FontError> {
    let directory = parse_woff2_directory(data)?;

    if directory
        .tables
        .iter()
        .any(|table| table.transform == Woff2Transform::Hmtx)
    {
        return Err(error(
            directory.compressed_data.start,
            "WOFF2 hmtx transform is not supported yet",
        ));
    }

    let decoded_data = decode_table_data(data, &directory)?;

    let mut tables: Vec<_> = split_table_data(&decoded_data, &directory)?
        .into_iter()
        .map(|(tag, bytes)| (tag, bytes.to_vec()))
        .collect();

    reconstruct_transformed_glyf(&directory, &mut tables)?;
    tables.sort_by_key(|(tag, _)| tag.bytes());

    let table_count = tables.len();
    let directory_length = table_count
        .checked_mul(16)
        .and_then(|length| length.checked_add(12))
        .ok_or_else(|| error(12, "WOFF2 SFNT directory is too large"))?;

    let expected_sfnt_size = tables
        .iter()
        .try_fold(directory_length, |length, (_, table)| {
            length
                .checked_add(padded_length(table.len())?)
                .ok_or_else(|| error(16, "WOFF2 SFNT output is too large"))
        })?;

    // WOFF 2.0 treats totalSfntSize as a reference value.
    // Allocate using the reconstructed table sizes instead.
    // See: https://www.w3.org/TR/WOFF2/#WOFF2Header
    let mut sfnt = vec![0_u8; expected_sfnt_size];

    write_u32(&mut sfnt, 0, directory.flavor)?;
    write_u16(
        &mut sfnt,
        4,
        u16::try_from(table_count).map_err(|_| error(12, "WOFF2 contains too many SFNT tables"))?,
    )?;

    let (search_range, entry_selector, range_shift) = sfnt_search_parameters(table_count);
    write_u16(&mut sfnt, 6, search_range)?;
    write_u16(&mut sfnt, 8, entry_selector)?;
    write_u16(&mut sfnt, 10, range_shift)?;

    let mut table_offset = directory_length;
    let mut head_offset = None;

    for (index, (tag, table)) in tables.iter().enumerate() {
        let record_offset = 12 + index * 16;
        let tag_bytes = tag.bytes();

        sfnt[record_offset..record_offset + 4].copy_from_slice(&tag_bytes);
        write_u32(&mut sfnt, record_offset + 4, table_checksum(*tag, table))?;
        write_u32(
            &mut sfnt,
            record_offset + 8,
            u32::try_from(table_offset)
                .map_err(|_| error(record_offset + 8, "SFNT table offset is too large"))?,
        )?;
        write_u32(
            &mut sfnt,
            record_offset + 12,
            u32::try_from(table.len())
                .map_err(|_| error(record_offset + 12, "SFNT table length is too large"))?,
        )?;

        let table_end = table_offset
            .checked_add(table.len())
            .ok_or_else(|| error(table_offset, "SFNT table range overflows"))?;

        sfnt[table_offset..table_end].copy_from_slice(table);

        if *tag == TableTag::HEAD {
            head_offset = Some(table_offset);
        }

        table_offset = padded_length(table_end)?;
    }

    if let Some(offset) = head_offset {
        let adjustment_offset = offset
            .checked_add(8)
            .ok_or_else(|| error(offset, "head table offset overflows"))?;

        if adjustment_offset + 4 <= sfnt.len() {
            write_u32(&mut sfnt, adjustment_offset, 0)?;

            let adjustment = 0xb1b0_afba_u32.wrapping_sub(sfnt_checksum(&sfnt));
            write_u32(&mut sfnt, adjustment_offset, adjustment)?;
        }
    }

    Ok(sfnt)
}

/// Reconstructs transformed glyf and loca tables, if present.
///
/// The table vector must remain in WOFF2 directory order while this
/// function runs.
///
/// Specification: WOFF 2.0, sections 5.1 and 5.3.
/// <https://www.w3.org/TR/WOFF2/>
fn reconstruct_transformed_glyf(
    directory: &Woff2Directory,
    tables: &mut [(TableTag, Vec<u8>)],
) -> Result<(), FontError> {
    let Some(glyf_index) = directory.tables.iter().position(|table| {
        table.tag == TableTag::GLYF && table.transform == Woff2Transform::GlyfLoca
    }) else {
        return Ok(());
    };

    let find = |tag| directory.tables.iter().position(|table| table.tag == tag);

    let loca_index = find(TableTag::LOCA)
        .ok_or_else(|| error(0, "transformed WOFF2 glyf table is missing loca"))?;

    let head_index = find(TableTag::HEAD)
        .ok_or_else(|| error(0, "transformed WOFF2 glyf table is missing head"))?;

    let maxp_index = find(TableTag::MAXP)
        .ok_or_else(|| error(0, "transformed WOFF2 glyf table is missing maxp"))?;

    // maxp.numGlyphs and head.indexToLocFormat.
    let glyph_count = read_u16(&tables[maxp_index].1, 4, 0)?;
    let index_format = read_u16(&tables[head_index].1, 50, 0)?;

    let streams = glyf::parse(&tables[glyf_index].1)?;

    if streams.num_glyphs != glyph_count {
        return Err(error(0, "WOFF2 glyf glyph count does not match maxp"));
    }

    if streams.index_format != index_format {
        return Err(error(0, "WOFF2 glyf index format does not match head"));
    }

    let (glyf_data, loca_data) =
        streams.reconstruct_tables(directory.tables[loca_index].original_length)?;

    tables[glyf_index].1 = glyf_data;
    tables[loca_index].1 = loca_data;

    Ok(())
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

    let search_range = largest_power_of_two * 16;
    let range_shift = table_count * 16 - search_range;

    (search_range as u16, entry_selector, range_shift as u16)
}

fn table_checksum(tag: TableTag, data: &[u8]) -> u32 {
    let mut checksum = 0_u32;

    for offset in (0..data.len()).step_by(4) {
        let mut word = [0_u8; 4];

        for (index, byte) in word.iter_mut().enumerate() {
            let position = offset + index;

            if position < data.len() && !(tag == TableTag::HEAD && (8..12).contains(&position)) {
                *byte = data[position];
            }
        }

        checksum = checksum.wrapping_add(u32::from_be_bytes(word));
    }

    checksum
}

fn sfnt_checksum(data: &[u8]) -> u32 {
    let mut checksum = 0_u32;

    for offset in (0..data.len()).step_by(4) {
        let mut word = [0_u8; 4];

        for (index, byte) in word.iter_mut().enumerate() {
            if let Some(value) = data.get(offset + index) {
                *byte = *value;
            }
        }

        checksum = checksum.wrapping_add(u32::from_be_bytes(word));
    }

    checksum
}

fn table_transform(tag: TableTag, version: u8, offset: usize) -> Result<Woff2Transform, FontError> {
    let tag = tag.bytes();

    if tag == *b"glyf" || tag == *b"loca" {
        match version {
            0 => Ok(Woff2Transform::GlyfLoca),
            3 => Ok(Woff2Transform::Null),
            _ => Err(error(offset, "unsupported WOFF2 glyf/loca transform")),
        }
    } else if tag == *b"hmtx" {
        match version {
            0 => Ok(Woff2Transform::Null),
            1 => Ok(Woff2Transform::Hmtx),
            _ => Err(error(offset, "unsupported WOFF2 hmtx transform")),
        }
    } else if version == 0 {
        Ok(Woff2Transform::Null)
    } else {
        Err(error(offset, "unsupported WOFF2 table transform"))
    }
}

/// Validates glyf/loca pairing in a single-font WOFF2 directory.
///
/// Other tables may appear between glyf and loca, but loca must follow
/// glyf. Both tables must use the same transformation.
///
/// Specification: WOFF 2.0, sections 5.3 and 5.5.
/// <https://www.w3.org/TR/WOFF2/>
fn validate_transformed_pairs(tables: &[Woff2Table], offset: usize) -> Result<(), FontError> {
    let glyf = tables.iter().position(|table| table.tag == TableTag::GLYF);
    let loca = tables.iter().position(|table| table.tag == TableTag::LOCA);

    match (glyf, loca) {
        (Some(glyf_index), Some(loca_index)) => {
            if glyf_index >= loca_index {
                return Err(error(offset, "WOFF2 loca table must follow glyf"));
            }

            if tables[glyf_index].transform != tables[loca_index].transform {
                return Err(error(offset, "WOFF2 glyf and loca transforms must match"));
            }
        }

        (Some(index), None) if tables[index].transform == Woff2Transform::GlyfLoca => {
            return Err(error(
                offset,
                "transformed WOFF2 glyf table is missing loca",
            ));
        }

        (None, Some(index)) if tables[index].transform == Woff2Transform::GlyfLoca => {
            return Err(error(
                offset,
                "transformed WOFF2 loca table is missing glyf",
            ));
        }

        _ => {}
    }

    Ok(())
}

fn optional_metadata_range(
    data: &[u8],
    offset: usize,
    length: usize,
    original_length: usize,
) -> Result<Option<Range<usize>>, FontError> {
    match (offset, length, original_length) {
        (0, 0, 0) => Ok(None),

        (offset, length, original_length) if offset != 0 && length != 0 && original_length != 0 => {
            Ok(Some(checked_range(
                data,
                offset,
                length,
                "WOFF2 metadata extends beyond input",
            )?))
        }

        _ => Err(error(28, "invalid WOFF2 metadata block")),
    }
}

fn optional_private_range(
    data: &[u8],
    offset: usize,
    length: usize,
) -> Result<Option<Range<usize>>, FontError> {
    match (offset, length) {
        (0, 0) => Ok(None),

        (offset, length) if offset != 0 && length != 0 => Ok(Some(checked_range(
            data,
            offset,
            length,
            "WOFF2 private data extends beyond input",
        )?)),

        _ => Err(error(40, "invalid WOFF2 private data block")),
    }
}

fn read_base_128(data: &[u8], cursor: &mut usize) -> Result<usize, FontError> {
    let mut value = 0_u32;

    for index in 0..5 {
        let byte = read_u8(data, *cursor, 0)?;
        *cursor = checked_add(*cursor, 1, "WOFF2 directory offset overflow")?;

        if index == 0 && byte == 0x80 {
            return Err(error(*cursor - 1, "WOFF2 UIntBase128 has a leading zero"));
        }

        if value & 0xfe00_0000 != 0 {
            return Err(error(*cursor - 1, "WOFF2 UIntBase128 overflows"));
        }

        value = (value << 7) | u32::from(byte & 0x7f);

        if byte & 0x80 == 0 {
            return to_usize(value, *cursor - 1);
        }
    }

    Err(error(
        *cursor,
        "WOFF2 UIntBase128 is longer than five bytes",
    ))
}

fn error(offset: usize, message: impl Into<String>) -> FontError {
    FontError {
        offset,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_null_transformed_woff2_directory() {
        let mut data = minimal_woff2(1, &[5, 4], &[0]);

        data[16..20].copy_from_slice(&32_u32.to_be_bytes());

        let directory = parse_woff2_directory(&data).unwrap();

        assert_eq!(directory.flavor, 0x0001_0000);
        assert_eq!(directory.total_sfnt_size, 32);
        assert_eq!(directory.compressed_data, 50..51);
        assert_eq!(directory.tables.len(), 1);
        assert_eq!(directory.tables[0].tag, TableTag::NAME);
        assert_eq!(directory.tables[0].original_length, 4);
        assert_eq!(directory.tables[0].transformed_length, None);
        assert_eq!(directory.tables[0].transform, Woff2Transform::Null);
    }

    #[test]
    fn parses_transformed_glyf_and_loca_directory_entries() {
        let data = minimal_woff2(2, &[10, 8, 6, 11, 4, 0], &[0]);

        let directory = parse_woff2_directory(&data).unwrap();

        assert_eq!(directory.compressed_data, 54..55);
        assert_eq!(directory.tables[0].tag, TableTag::GLYF);
        assert_eq!(directory.tables[0].original_length, 8);
        assert_eq!(directory.tables[0].transformed_length, Some(6));
        assert_eq!(directory.tables[0].transform, Woff2Transform::GlyfLoca);
        assert_eq!(directory.tables[1].tag, TableTag::LOCA);
        assert_eq!(directory.tables[1].transformed_length, Some(0));
    }

    #[test]
    fn rejects_uint_base_128_with_a_leading_zero() {
        let data = minimal_woff2(1, &[5, 0x80, 4], &[0]);

        let error = parse_woff2_directory(&data).unwrap_err();

        assert_eq!(error.message, "WOFF2 UIntBase128 has a leading zero");
    }

    #[test]
    fn decodes_an_untransformed_woff2_font() {
        let compressed = uncompressed_brotli(b"abcd");
        let mut data = minimal_woff2(1, &[5, 4], &compressed);

        data[16..20].copy_from_slice(&32_u32.to_be_bytes());

        let sfnt = decode_woff2(&data).unwrap();

        assert_eq!(sfnt.len(), 32);
        assert_eq!(&sfnt[0..4], &0x0001_0000_u32.to_be_bytes());
        assert_eq!(&sfnt[4..6], &1_u16.to_be_bytes());
        assert_eq!(&sfnt[12..16], b"name");
        assert_eq!(&sfnt[16..20], &0x6162_6364_u32.to_be_bytes());
        assert_eq!(&sfnt[20..24], &28_u32.to_be_bytes());
        assert_eq!(&sfnt[24..28], &4_u32.to_be_bytes());
        assert_eq!(&sfnt[28..32], b"abcd");
    }

    #[test]
    fn rejects_unsupported_hmtx_transform_before_decompression() {
        let data = minimal_woff2(1, &[0x43, 4, 3], &[0x06]);
        let error = decode_woff2(&data).unwrap_err();

        assert_eq!(error.message, "WOFF2 hmtx transform is not supported yet");
    }

    #[test]
    fn parses_distinct_cvt_and_fdsc_known_tags() {
        // Known tag indexes 8 and 44.
        let data = minimal_woff2(2, &[8, 4, 44, 4], &[0]);
        let directory = parse_woff2_directory(&data).unwrap();

        assert_eq!(directory.tables[0].tag.bytes(), *b"cvt ");
        assert_eq!(directory.tables[1].tag.bytes(), *b"fdsc");
    }

    #[test]
    fn reconstructs_woff2_table_from_compressed_brotli() {
        use crate::graphics::font::sfnt::SfntFont;

        // Brotli stream producing eleven 'a' bytes using an overlapping copy.
        // The stream was independently verified with libbrolidec.
        let compressed = [0x42, 0x01, 0x00, 0x04, 0x44, 0x58, 0x20, 0x13, 0x10];

        // One untransformed `name` table containing eleven bytes.
        let mut data = minimal_woff2(1, &[5, 11], &compressed);
        data[16..20].copy_from_slice(&40_u32.to_be_bytes());

        let sfnt_data = decode_woff2(&data).unwrap();
        let font = SfntFont::parse(&sfnt_data).unwrap();

        assert_eq!(sfnt_data.len(), 40);
        assert_eq!(font.table(TableTag::NAME), Some(&b"aaaaaaaaaaa"[..]));
    }

    #[test]
    fn reconstructs_woff2_when_total_sfnt_size_differs() {
        use crate::graphics::font::sfnt::SfntFont;

        let compressed = uncompressed_brotli(b"abcd");

        // The reconstructed SFNT occupies 32 bytes. The header value is
        // deliberately varied to check that it is not used as an allocation size.
        for declared_size in [0_u32, 16, 32, 40, u32::MAX] {
            let mut data = minimal_woff2(1, &[5, 4], &compressed);
            data[16..20].copy_from_slice(&declared_size.to_be_bytes());

            let sfnt_data = decode_woff2(&data).unwrap();
            let font = SfntFont::parse(&sfnt_data).unwrap();

            assert_eq!(sfnt_data.len(), 32, "declared size: {declared_size}");
            assert_eq!(
                font.table(TableTag::NAME),
                Some(&b"abcd"[..]),
                "declared size: {declared_size}"
            );
        }
    }

    #[test]
    fn parses_real_wpt_woff2_directory() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/css/WOFF2/support/",
            "metadatadisplay-schema-metadata-004.woff2",
        );

        let data = std::fs::read(path).expect("WPT WOFF2 fixture must exist");

        assert!(data.starts_with(b"wOF2"), "fixture is not a WOFF2 file");

        let directory = parse_woff2_directory(&data).unwrap();

        assert!(!directory.tables.is_empty());
        assert!(directory.compressed_data.start < directory.compressed_data.end);
        assert!(directory.compressed_data.end <= data.len());

        println!(
            "WOFF2 fixture: {} byte, {} tables, {} compressed bytes",
            data.len(),
            directory.tables.len(),
            directory.compressed_data.len()
        );

        for table in &directory.tables {
            println!(
                "  {:?}: original={}, transformed={:?}, transform={:?}",
                table.tag, table.original_length, table.transformed_length, table.transform,
            );
        }
    }

    #[test]
    fn decodes_real_wpt_woff2_fixture() {
        use crate::graphics::font::sfnt::SfntFont;

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/css/WOFF2/support/",
            "metadatadisplay-schema-metadata-004.woff2",
        );

        let data = std::fs::read(path).expect("WPT WOFF2 fixture must exist");
        let directory = parse_woff2_directory(&data).unwrap();

        let decoded = decode_brotli(&data[directory.compressed_data.clone()], 1691).unwrap();

        assert_eq!(decoded.len(), 1691);

        // Check the decompressed bytes against an independent Brotli decoder.
        let mut crc = 0xffff_ffff_u32;

        for byte in decoded {
            crc ^= u32::from(byte);

            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }

        assert_eq!(!crc, 0x293b_f12b);

        let sfnt_data = decode_woff2(&data).unwrap();
        let font = SfntFont::parse(&sfnt_data).unwrap();

        assert_eq!(font.table(TableTag::NAME).unwrap().len(), 663);
    }

    #[test]
    fn decompresses_transformed_table_lengths() {
        let compressed = uncompressed_brotli(b"abcdef");

        // Transformed glyf: original length 8, transformed length 6.
        // Transformed loca: original length 4, transformed length 0.
        let data = minimal_woff2(2, &[10, 8, 6, 11, 4, 0], &compressed);

        let directory = parse_woff2_directory(&data).unwrap();
        let decoded = decode_table_data(&data, &directory).unwrap();

        assert_eq!(decoded, b"abcdef");

        // Six bytes are not a valid transformed glyf table.
        assert!(decode_woff2(&data).is_err());
    }

    #[test]
    fn rejects_shorter_transformed_table_data() {
        let compressed = uncompressed_brotli(b"abcde");

        // The directory declares six decompressed bytes, but the stream
        // contains only five.
        let data = minimal_woff2(2, &[10, 8, 6, 11, 4, 0], &compressed);

        let directory = parse_woff2_directory(&data).unwrap();
        let error = decode_table_data(&data, &directory).unwrap_err();

        assert_eq!(error.message, "WOFF2 Brotli data has an unexpected length");
    }

    #[test]
    fn splits_mixed_transformed_and_untransformed_table_data() {
        let compressed = uncompressed_brotli(b"xyzabcdef");

        // name: original length 3.
        // glyf: original length 8, transformed length 6.
        // loca: original length 4, transformed length 0.
        let data = minimal_woff2(3, &[5, 3, 10, 8, 6, 11, 4, 0], &compressed);

        let directory = parse_woff2_directory(&data).unwrap();
        let decoded = decode_table_data(&data, &directory).unwrap();
        let tables = split_table_data(&decoded, &directory).unwrap();

        assert_eq!(
            tables,
            vec![
                (TableTag::NAME, &b"xyz"[..]),
                (TableTag::GLYF, &b"abcdef"[..]),
                (TableTag::LOCA, &b""[..]),
            ]
        );
    }

    #[test]
    fn rejects_incomplete_or_extra_table_data() {
        let compressed = uncompressed_brotli(b"abcdef");
        let data = minimal_woff2(2, &[10, 8, 6, 11, 4, 0], &compressed);

        let directory = parse_woff2_directory(&data).unwrap();

        let truncated = split_table_data(b"abcde", &directory).unwrap_err();

        assert_eq!(truncated.message, "WOFF2 table data is truncated");

        let extra = split_table_data(b"abcdefg", &directory).unwrap_err();

        assert_eq!(extra.message, "WOFF2 table data has trailing bytes");
    }

    #[test]
    fn parses_glyf_from_decompressed_table_data() {
        // One empty glyph: two contour bytes and a four-byte bbox bitmap.
        let mut transformed = vec![0_u8; 42];

        write_u16(&mut transformed, 4, 1).unwrap();
        write_u32(&mut transformed, 8, 2).unwrap();
        write_u32(&mut transformed, 28, 4).unwrap();

        let compressed = uncompressed_brotli(&transformed);

        // glyf: original length 10, transformed length 42.
        // loca: original length 4, transformed length 0.
        let data = minimal_woff2(2, &[10, 10, 42, 11, 4, 0], &compressed);

        let directory = parse_woff2_directory(&data).unwrap();
        let decoded = decode_table_data(&data, &directory).unwrap();
        let tables = split_table_data(&decoded, &directory).unwrap();

        assert_eq!(tables[0].0, TableTag::GLYF);

        let streams = glyf::parse(tables[0].1).unwrap();

        assert_eq!(streams.num_glyphs, 1);
        assert_eq!(streams.contours, &[0, 0]);
        assert_eq!(streams.bbox_bitmap, &[0, 0, 0, 0]);
        assert_eq!(tables[1], (TableTag::LOCA, &b""[..]));
    }

    #[test]
    fn accepts_other_tables_between_glyf_and_loca() {
        let directories: &[&[u8]] = &[
            // Transformed glyf, hhea, transformed loca.
            &[10, 8, 6, 2, 2, 11, 4, 0],
            // Untransformed glyf, hhea, untransformed loca.
            &[0xca, 8, 2, 2, 0xcb, 4],
        ];

        for &directory in directories {
            let data = minimal_woff2(3, directory, &[0]);
            let parsed = parse_woff2_directory(&data).unwrap();

            assert_eq!(parsed.tables[0].tag, TableTag::GLYF);
            assert_eq!(parsed.tables[1].tag, TableTag::HHEA);
            assert_eq!(parsed.tables[2].tag, TableTag::LOCA);
        }
    }

    #[test]
    fn rejects_reversed_or_mismatched_glyf_loca_pairs() {
        let reversed: &[&[u8]] = &[
            // Transformed loca before transformed glyf.
            &[11, 4, 0, 10, 8, 6],
            // Untransformed loca before untransformed glyf.
            &[0xcb, 4, 0xca, 8],
        ];

        for &directory in reversed {
            let data = minimal_woff2(2, directory, &[0]);
            let error = parse_woff2_directory(&data).unwrap_err();

            assert_eq!(error.message, "WOFF2 loca table must follow glyf");
        }

        let mismatched: &[&[u8]] = &[
            // Transformed glyf, untransformed loca.
            &[10, 8, 6, 0xcb, 4],
            // Untransformed glyf, transformed loca.
            &[0xca, 8, 11, 4, 0],
        ];

        for &directory in mismatched {
            let data = minimal_woff2(2, directory, &[0]);
            let error = parse_woff2_directory(&data).unwrap_err();

            assert_eq!(error.message, "WOFF2 glyf and loca transforms must match");
        }
    }

    #[test]
    fn rejects_unpaired_transformed_glyf_and_loca() {
        let data = minimal_woff2(1, &[10, 8, 6], &[0]);
        let error = parse_woff2_directory(&data).unwrap_err();

        assert_eq!(
            error.message,
            "transformed WOFF2 glyf table is missing loca"
        );

        let data = minimal_woff2(1, &[11, 4, 0], &[0]);
        let error = parse_woff2_directory(&data).unwrap_err();

        assert_eq!(
            error.message,
            "transformed WOFF2 loca table is missing glyf"
        );
    }

    fn minimal_woff2(table_count: u16, directory: &[u8], compressed: &[u8]) -> Vec<u8> {
        let length = WOFF2_HEADER_LENGTH + directory.len() + compressed.len();
        let mut data = vec![0_u8; length];

        data[0..4].copy_from_slice(&WOFF2_SIGNATURE.to_be_bytes());
        data[4..8].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
        data[8..12].copy_from_slice(&(length as u32).to_be_bytes());
        data[12..14].copy_from_slice(&table_count.to_be_bytes());
        data[20..24].copy_from_slice(&(compressed.len() as u32).to_be_bytes());

        let directory_end = WOFF2_HEADER_LENGTH + directory.len();
        data[WOFF2_HEADER_LENGTH..directory_end].copy_from_slice(directory);
        data[directory_end..].copy_from_slice(compressed);

        data
    }

    fn uncompressed_brotli(payload: &[u8]) -> Vec<u8> {
        assert!(!payload.is_empty());
        assert!(payload.len() <= 65_536);

        let mut data = Vec::new();
        let mut bit_position = 0_usize;

        append_bits(&mut data, &mut bit_position, 0, 1);
        append_bits(&mut data, &mut bit_position, 0, 1);
        append_bits(&mut data, &mut bit_position, 0, 2);
        append_bits(
            &mut data,
            &mut bit_position,
            u32::try_from(payload.len() - 1).unwrap(),
            16,
        );
        append_bits(&mut data, &mut bit_position, 1, 1);

        while bit_position % 8 != 0 {
            append_bits(&mut data, &mut bit_position, 0, 1);
        }

        data.extend_from_slice(payload);
        bit_position += payload.len() * 8;

        append_bits(&mut data, &mut bit_position, 1, 1);
        append_bits(&mut data, &mut bit_position, 1, 1);

        while bit_position % 8 != 0 {
            append_bits(&mut data, &mut bit_position, 0, 1);
        }

        data
    }

    fn transformed_glyf_fixture(maxp_glyphs: u16, head_format: u16, loca_length: u8) -> Vec<u8> {
        let mut head = vec![0_u8; 54];
        write_u16(&mut head, 50, head_format).unwrap();

        let mut maxp = vec![0_u8; 6];
        write_u32(&mut maxp, 0, 0x0001_0000).unwrap();
        write_u16(&mut maxp, 4, maxp_glyphs).unwrap();

        let mut glyf = vec![0_u8; 36];
        write_u16(&mut glyf, 4, 2).unwrap();

        let sizes = [4_u32, 1, 3, 5, 0, 4, 2];

        for (index, size) in sizes.into_iter().enumerate() {
            write_u32(&mut glyf, 8 + index * 4, size).unwrap()
        }

        glyf.extend_from_slice(&[
            0, 0, 0, 1, // empty glyph, then one-contour glyph
            3, // three points
            1, 11, 86, // point flags
            0, 50, 24, 79, // coordinates
            2,  // instruction length
            0, 0, 0, 0, // bbox bitmap
            0xaa, 0xbb, // instructions
        ]);

        let mut raw = head;
        raw.extend_from_slice(&maxp);
        raw.extend_from_slice(&glyf);

        let compressed = uncompressed_brotli(&raw);

        minimal_woff2(
            4,
            &[
                1,
                54, // head
                4,
                6, // maxp
                10,
                20,
                55, // transformed glyf
                11,
                loca_length,
                0, // transformed loca
            ],
            &compressed,
        )
    }

    #[test]
    fn decodes_transformed_glyf_loca_in_a_woff2_container() {
        use crate::graphics::font::{
            glyf::{Glyph, parse_glyph},
            sfnt::SfntFont,
        };

        let data = transformed_glyf_fixture(2, 0, 6);
        let sfnt = decode_woff2(&data).unwrap();
        let font = SfntFont::parse(&sfnt).unwrap();

        let glyf = font.table(TableTag::GLYF).unwrap();

        assert_eq!(font.table(TableTag::LOCA), Some(&[0, 0, 0, 0, 0, 11][..]));

        assert_eq!(glyf.len(), 22);

        let Glyph::Simple(glyph) = parse_glyph(glyf, 0).unwrap() else {
            panic!("expected a simple glyph");
        };

        assert_eq!(glyph.bounds.x_max, 50);
        assert_eq!(glyph.bounds.y_max, 80);
    }

    #[test]
    fn rejects_transformed_glyf_with_inconsistent_metadata() {
        let cases = [
            (
                transformed_glyf_fixture(3, 0, 6),
                "WOFF2 glyf glyph count does not match maxp",
            ),
            (
                transformed_glyf_fixture(2, 1, 6),
                "WOFF2 glyf index format does not match head",
            ),
            (
                transformed_glyf_fixture(2, 0, 8),
                "WOFF2 loca original length does not match glyph count",
            ),
        ];

        for (data, expected_message) in cases {
            let error = decode_woff2(&data).unwrap_err();

            assert_eq!(error.message, expected_message);
        }
    }

    fn append_bits(data: &mut Vec<u8>, bit_position: &mut usize, value: u32, bit_count: usize) {
        for bit_index in 0..bit_count {
            if *bit_position / 8 == data.len() {
                data.push(0);
            }

            let bit = (value >> bit_index) & 1;
            data[*bit_position / 8] |= (bit as u8) << (*bit_position % 8);
            *bit_position += 1;
        }
    }
}
