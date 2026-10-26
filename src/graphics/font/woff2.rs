use super::{
    brotli::decode_uncompressed_meta_blocks,
    reader::{read_u8, read_u16, read_u32},
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
    *b"SVG ", *b"sbix", *b"acnt", *b"avar", *b"bdat", *b"bloc", *b"bsln", *b"cvar", *b"cvt ",
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

pub fn decode_woff2(data: &[u8]) -> Result<Vec<u8>, FontError> {
    let directory = parse_woff2_directory(data)?;

    if directory
        .tables
        .iter()
        .any(|table| table.transform != Woff2Transform::Null)
    {
        return Err(error(
            directory.compressed_data.start,
            "WOFF2 table transforms are not supported yet",
        ));
    }

    let decoded_length = directory.tables.iter().try_fold(0_usize, |length, table| {
        length
            .checked_add(table.original_length)
            .ok_or_else(|| error(directory.compressed_data.start, "WOFF2 data is too large"))
    })?;

    let decoded_data =
        decode_uncompressed_meta_blocks(&data[directory.compressed_data.clone()], decoded_length)?;

    if decoded_data.len() != decoded_length {
        return Err(error(
            directory.compressed_data.start,
            "WOFF2 Brotli data has an unexpected length",
        ));
    }

    let mut cursor = 0_usize;
    let mut tables = Vec::with_capacity(directory.tables.len());

    for table in &directory.tables {
        let end = checked_end(
            cursor,
            table.original_length,
            decoded_data.len(),
            "WOFF2 table data is truncated",
        )?;

        tables.push((table.tag, decoded_data[cursor..end].to_vec()));
        cursor = end;
    }

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

    if expected_sfnt_size != directory.total_sfnt_size {
        return Err(error(
            16,
            "WOFF2 totalSfntSize does not match reconstructed tables",
        ));
    }

    let mut sfnt = vec![0_u8; expected_sfnt_size];

    write_u32(&mut sfnt, 0, directory.flavor);
    write_u16(
        &mut sfnt,
        4,
        u16::try_from(table_count).map_err(|_| error(12, "WOFF2 contains too many SFNT tables"))?,
    );

    let (search_range, entry_selector, range_shift) = sfnt_search_parameters(table_count);
    write_u16(&mut sfnt, 6, search_range);
    write_u16(&mut sfnt, 8, entry_selector);
    write_u16(&mut sfnt, 10, range_shift);

    let mut table_offset = directory_length;
    let mut head_offset = None;

    for (index, (tag, table)) in tables.iter().enumerate() {
        let record_offset = 12 + index * 16;
        let tag_bytes = tag.bytes();

        sfnt[record_offset..record_offset + 4].copy_from_slice(&tag_bytes);
        write_u32(&mut sfnt, record_offset + 4, table_checksum(*tag, table));
        write_u32(
            &mut sfnt,
            record_offset + 8,
            u32::try_from(table_offset)
                .map_err(|_| error(record_offset + 8, "SFNT table offset is too large"))?,
        );
        write_u32(
            &mut sfnt,
            record_offset + 12,
            u32::try_from(table.len())
                .map_err(|_| error(record_offset + 12, "SFNT table length is too large"))?,
        );

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
            write_u32(&mut sfnt, adjustment_offset, 0);

            let adjustment = 0xb1b0_afba_u32.wrapping_sub(sfnt_checksum(&sfnt));
            write_u32(&mut sfnt, adjustment_offset, adjustment);
        }
    }

    Ok(sfnt)
}

fn padded_length(length: usize) -> Result<usize, FontError> {
    length
        .checked_add(3)
        .map(|length| length & !3)
        .ok_or_else(|| error(length, "WOFF2 length overflows"))
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

fn write_u16(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

fn write_u32(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
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

fn validate_transformed_pairs(tables: &[Woff2Table], offset: usize) -> Result<(), FontError> {
    for (index, table) in tables.iter().enumerate() {
        if table.transform != Woff2Transform::GlyfLoca {
            continue;
        }

        let tag = table.tag.bytes();

        if tag == *b"glyf" {
            let Some(next) = tables.get(index + 1) else {
                return Err(error(
                    offset,
                    "transformed WOFF2 glyf table is missing loca",
                ));
            };

            if next.tag.bytes() != *b"loca" || next.transform != Woff2Transform::GlyfLoca {
                return Err(error(
                    offset,
                    "transformed WOFF2 glyf table is not followed by loca",
                ));
            }
        } else if tag == *b"loca" {
            let Some(previous) = index.checked_sub(1).and_then(|index| tables.get(index)) else {
                return Err(error(
                    offset,
                    "transformed WOFF2 loca table has not preceding glyf table",
                ));
            };

            if previous.tag.bytes() != *b"glyf" || previous.transform != Woff2Transform::GlyfLoca {
                return Err(error(
                    offset,
                    "transformed WOFF2 loca table does not follow glyf",
                ));
            }
        } else {
            return Err(error(offset, "invalid WOFF2 glyf/loca transform target"));
        }
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

fn read_bytes(data: &[u8], offset: usize, length: usize) -> Result<&[u8], FontError> {
    let range = checked_range(data, offset, length, "unexpected end of WOFF2 file")?;
    Ok(&data[range])
}

fn checked_range(
    data: &[u8],
    offset: usize,
    length: usize,
    message: &str,
) -> Result<Range<usize>, FontError> {
    let end = checked_end(offset, length, data.len(), message)?;
    Ok(offset..end)
}

fn checked_end(
    offset: usize,
    length: usize,
    limit: usize,
    message: &str,
) -> Result<usize, FontError> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| error(offset, "WOFF2 offset overflows"))?;

    if end > limit {
        return Err(error(offset, message));
    }

    Ok(end)
}

fn checked_add(offset: usize, amount: usize, message: &str) -> Result<usize, FontError> {
    offset
        .checked_add(amount)
        .ok_or_else(|| error(offset, message))
}

fn to_usize(value: u32, offset: usize) -> Result<usize, FontError> {
    usize::try_from(value).map_err(|_| error(offset, "WOFF2 value is too large"))
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
    fn rejects_woff2_table_transforms_before_reconstruction() {
        let data = minimal_woff2(2, &[10, 8, 6, 11, 4, 0], &[0x06]);

        let error = decode_woff2(&data).unwrap_err();

        assert_eq!(
            error.message,
            "WOFF2 table transforms are not supported yet"
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
