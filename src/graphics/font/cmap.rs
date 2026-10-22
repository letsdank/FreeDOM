use super::sfnt::{FontError, SfntFont, TableTag};

#[derive(Clone, Debug)]
pub struct CmapTable {
    format4: Option<CmapFormat4>,
    format12: Option<CmapFormat12>,
}

#[derive(Clone, Debug)]
struct CmapFormat4 {
    data: Vec<u8>,
    segments: Vec<CmapSegment>,
}

#[derive(Clone, Copy, Debug)]
struct CmapSegment {
    start: u16,
    end: u16,
    delta: u16,
    range_offset: u16,
    range_offset_position: usize,
}

#[derive(Clone, Debug)]
struct CmapFormat12 {
    groups: Vec<CmapGroup>,
}

#[derive(Clone, Copy, Debug)]
struct CmapGroup {
    start_character: u32,
    end_character: u32,
    start_glyph_id: u32,
}

impl CmapTable {
    pub fn parse(font: &SfntFont<'_>) -> Result<Self, FontError> {
        let record = font.table_record(TableTag::CMAP).ok_or_else(|| FontError {
            offset: 0,
            message: "missing required cmap table".to_owned(),
        })?;

        let table = font.table(TableTag::CMAP).ok_or_else(|| FontError {
            offset: record.offset,
            message: "cannot read cmap table".to_owned(),
        })?;

        if read_u16(table, 0, record.offset)? != 0 {
            return Err(error(record.offset, "unsupported cmap version"));
        }

        let subtable_count = usize::from(read_u16(table, 2, record.offset)?);
        let directory_length = 4_usize
            .checked_add(
                subtable_count
                    .checked_mul(8)
                    .ok_or_else(|| error(record.offset + 2, "cmap directory is too large"))?,
            )
            .ok_or_else(|| error(record.offset + 2, "cmap directory is too large"))?;

        if table.len() < directory_length {
            return Err(error(record.offset, "truncated cmap encoding records"));
        }

        let mut selected_format4 = None;
        let mut selected_format12 = None;

        for index in 0..subtable_count {
            let entry_offset = 4 + index * 8;
            let platform_id = read_u16(table, entry_offset, record.offset)?;
            let encoding_id = read_u16(table, entry_offset + 2, record.offset)?;
            let subtable_offset =
                usize::try_from(read_u32(table, entry_offset + 4, record.offset)?).map_err(
                    |_| error(record.offset + entry_offset + 4, "cmap offset is too large"),
                )?;

            let format = read_u16(table, subtable_offset, record.offset)?;
            let Some(priority) = unicode_priority(platform_id, encoding_id) else {
                continue;
            };

            match format {
                4 if selected_format4
                    .as_ref()
                    .is_none_or(|(selected_priority, _)| priority > *selected_priority) =>
                    {
                        selected_format4 = Some((priority, subtable_offset));
                    }

                12 if selected_format12
                    .as_ref()
                    .is_none_or(|(selected_priority, _)| priority > *selected_priority) =>
                    {
                        selected_format12 = Some((priority, subtable_offset));
                    }

                _ => {}
            }
        }

        let format4 = selected_format4
            .map(|(_, offset)| Self::parse_format4(table, offset, record.offset))
            .transpose()?;

        let format12 = selected_format12
            .map(|(_, offset)| Self::parse_format12(table, offset, record.offset))
            .transpose()?;

        if format4.is_none() && format12.is_none() {
            return Err(error(
                record.offset,
                "font has no supported Unicode cmap subtable",
            ));
        }

        Ok(Self { format4, format12 })
    }

    pub fn glyph_id(&self, character: char) -> u16 {
        let code = character as u32;

        self.format12
            .as_ref()
            .and_then(|format| format.glyph_id(code))
            .or_else(|| {
                u16::try_from(code)
                    .ok()
                    .and_then(|code| self.format4.as_ref()?.glyph_id(code))
            })
            .unwrap_or(0)
    }

    fn parse_format4(
        table: &[u8],
        subtable_offset: usize,
        table_offset: usize,
    ) -> Result<CmapFormat4, FontError> {
        let format_length = usize::from(read_u16(table, subtable_offset + 2, table_offset)?);

        if format_length < 16 {
            return Err(error(
                table_offset + subtable_offset,
                "cmap format 4 table is too short",
            ));
        }

        let end = subtable_offset
            .checked_add(format_length)
            .ok_or_else(|| error(table_offset + subtable_offset, "cmap length overflow"))?;

        let format_data = table
            .get(subtable_offset..end)
            .ok_or_else(|| {
                error(
                    table_offset + subtable_offset,
                    "truncated cmap format 4 table",
                )
            })?
            .to_vec();

        let seg_count_x2 = read_u16(&format_data, 6, table_offset + subtable_offset)?;

        if seg_count_x2 == 0 || seg_count_x2 % 2 != 0 {
            return Err(error(
                table_offset + subtable_offset + 6,
                "invalid cmap format 4 segment count",
            ));
        }

        let segment_count = usize::from(seg_count_x2 / 2);
        let minimum_length =
            16_usize
                .checked_add(segment_count.checked_mul(8).ok_or_else(|| {
                    error(table_offset + subtable_offset, "too many cmap segments")
                })?)
                .ok_or_else(|| error(table_offset + subtable_offset, "too many cmap segments"))?;

        if format_data.len() < minimum_length {
            return Err(error(
                table_offset + subtable_offset,
                "truncated cmap format 4 segment arrays",
            ));
        }

        let end_codes_offset = 14;
        let start_codes_offset = end_codes_offset + segment_count * 2 + 2;
        let deltas_offset = start_codes_offset + segment_count * 2;
        let range_offsets_offset = deltas_offset + segment_count * 2;

        let mut segments = Vec::with_capacity(segment_count);

        for index in 0..segment_count {
            let start = read_u16(
                &format_data,
                start_codes_offset + index * 2,
                table_offset + subtable_offset,
            )?;
            let end = read_u16(
                &format_data,
                end_codes_offset + index * 2,
                table_offset + subtable_offset,
            )?;
            let delta = read_u16(
                &format_data,
                deltas_offset + index * 2,
                table_offset + subtable_offset,
            )?;
            let range_offset_position = range_offsets_offset + index * 2;
            let range_offset = read_u16(
                &format_data,
                range_offset_position,
                table_offset + subtable_offset,
            )?;

            if start > end {
                return Err(error(
                    table_offset + subtable_offset + start_codes_offset + index * 2,
                    "cmap segment start exceeds its end",
                ));
            }

            if range_offset != 0 {
                let glyph_count = usize::from(end - start);
                let last_glyph_end = range_offset_position
                    .checked_add(usize::from(range_offset))
                    .and_then(|offset| offset.checked_add(glyph_count * 2))
                    .and_then(|offset| offset.checked_add(2))
                    .ok_or_else(|| {
                        error(
                            table_offset + subtable_offset + range_offset_position,
                            "cmap glyph index offset overflows",
                        )
                    })?;

                if last_glyph_end > format_data.len() {
                    return Err(error(
                        table_offset + subtable_offset + range_offset_position,
                        "cmap glyph index lies outside its subtable",
                    ));
                }
            }

            segments.push(CmapSegment {
                start,
                end,
                delta,
                range_offset,
                range_offset_position,
            });
        }

        if segments
            .last()
            .is_none_or(|segment| segment.end != u16::MAX)
        {
            return Err(error(
                table_offset + subtable_offset,
                "cmap format 4 is missing its sentinel segment",
            ));
        }

        Ok(CmapFormat4 {
            data: format_data,
            segments,
        })
    }

    fn parse_format12(
        table: &[u8],
        subtable_offset: usize,
        table_offset: usize,
    ) -> Result<CmapFormat12, FontError> {
        let format_length = usize::try_from(read_u32(table, subtable_offset + 4, table_offset)?)
            .map_err(|_| {
                error(
                    table_offset + subtable_offset + 4,
                    "cmap length is too large",
                )
            })?;

        if format_length < 16 {
            return Err(error(
                table_offset + subtable_offset,
                "cmap format 12 table is too short",
            ));
        }

        let subtable_end = subtable_offset
            .checked_add(format_length)
            .ok_or_else(|| error(table_offset + subtable_offset, "cmap length overflows"))?;

        if subtable_end > table.len() {
            return Err(error(
                table_offset + subtable_offset,
                "truncated cmap format 12 table",
            ));
        }

        let group_count = usize::try_from(read_u32(table, subtable_offset + 12, table_offset)?)
            .map_err(|_| error(table_offset + subtable_offset + 12, "too many cmap groups"))?;

        let required_length = 16_usize
            .checked_add(
                group_count
                    .checked_mul(12)
                    .ok_or_else(|| error(table_offset + subtable_offset, "too many cmap groups"))?,
            )
            .ok_or_else(|| error(table_offset + subtable_offset, "too many cmap groups"))?;

        if required_length > format_length {
            return Err(error(
                table_offset + subtable_offset,
                "truncated cmap format 12 groups",
            ));
        }

        let mut groups = Vec::with_capacity(group_count);
        let mut previous_end = None;

        for index in 0..group_count {
            let group_offset = subtable_offset + 16 + index * 12;
            let start_character = read_u32(table, group_offset, table_offset)?;
            let end_character = read_u32(table, group_offset + 4, table_offset)?;
            let start_glyph_id = read_u32(table, group_offset + 8, table_offset)?;

            if start_character > end_character {
                return Err(error(
                    table_offset + group_offset,
                    "cmap format 12 group start exceeds its end",
                ));
            }

            if previous_end.is_some_and(|previous_end| start_character <= previous_end) {
                return Err(error(
                    table_offset + group_offset,
                    "cmap format 12 groups overlap or are unsorted",
                ));
            }

            previous_end = Some(end_character);

            groups.push(CmapGroup {
                start_character,
                end_character,
                start_glyph_id,
            });
        }

        Ok(CmapFormat12 { groups })
    }
}

impl CmapFormat4 {
    fn glyph_id(&self, code: u16) -> Option<u16> {
        let segment = self.segments.iter().find(|segment| code <= segment.end)?;

        if code < segment.start {
            return None;
        }

        if segment.range_offset == 0 {
            return Some(code.wrapping_add(segment.delta));
        }

        let glyph_offset = segment.range_offset_position
            + usize::from(segment.range_offset)
            + usize::from(code - segment.start) * 2;

        let glyph_id = u16::from_be_bytes([self.data[glyph_offset], self.data[glyph_offset + 1]]);

        Some(if glyph_id == 0 {
            0
        } else {
            glyph_id.wrapping_add(segment.delta)
        })
    }
}

impl CmapFormat12 {
    fn glyph_id(&self, code: u32) -> Option<u16> {
        let group = self
            .groups
            .iter()
            .find(|group| code >= group.start_character && code <= group.end_character)?;

        let glyph_id = group
            .start_glyph_id
            .checked_add(code - group.start_character)?;

        Some(u16::try_from(glyph_id).unwrap_or(0))
    }
}

fn unicode_priority(platform_id: u16, encoding_id: u16) -> Option<u8> {
    match (platform_id, encoding_id) {
        (0, _) => Some(3),
        (3, 1 | 10) => Some(2),
        _ => None,
    }
}

fn read_u16(data: &[u8], offset: usize, base_offset: usize) -> Result<u16, FontError> {
    let bytes = read_bytes(data, offset, 2, base_offset)?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize, base_offset: usize) -> Result<u32, FontError> {
    let bytes = read_bytes(data, offset, 4, base_offset)?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_bytes<'a>(
    data: &'a [u8],
    offset: usize,
    length: usize,
    base_offset: usize,
) -> Result<&'a [u8], FontError> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| error(base_offset + offset, "cmap offset overflows"))?;

    data.get(offset..end)
        .ok_or_else(|| error(base_offset + offset, "truncated cmap table"))
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
    fn maps_unicode_characters_through_a_format_4_table() {
        let font_data = cmap_only_font();
        let font = SfntFont::parse(&font_data).unwrap();
        let cmap = CmapTable::parse(&font).unwrap();

        assert_eq!(cmap.glyph_id('A'), 66);
        assert_eq!(cmap.glyph_id('B'), 0);
        assert_eq!(cmap.glyph_id('🦀'), 0);
    }

    #[test]
    fn maps_astral_unicode_with_a_format_12_table() {
        let data = cmap_format_12_font();
        let font = SfntFont::parse(&data).unwrap();
        let cmap = CmapTable::parse(&font).unwrap();

        assert_eq!(cmap.glyph_id('🦀'), 77);
        assert_eq!(cmap.glyph_id('A'), 0);
    }

    fn cmap_only_font() -> Vec<u8> {
        let mut data = vec![0_u8; 72];

        data[0..4].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
        data[4..6].copy_from_slice(&1_u16.to_be_bytes());

        data[12..16].copy_from_slice(b"cmap");
        data[20..24].copy_from_slice(&28_u32.to_be_bytes());
        data[24..28].copy_from_slice(&44_u32.to_be_bytes());

        data[28..30].copy_from_slice(&0_u16.to_be_bytes());
        data[30..32].copy_from_slice(&1_u16.to_be_bytes());

        data[32..34].copy_from_slice(&3_u16.to_be_bytes());
        data[34..36].copy_from_slice(&1_u16.to_be_bytes());
        data[36..40].copy_from_slice(&12_u32.to_be_bytes());

        data[40..42].copy_from_slice(&4_u16.to_be_bytes());
        data[42..44].copy_from_slice(&32_u16.to_be_bytes());
        data[46..48].copy_from_slice(&4_u16.to_be_bytes());
        data[48..50].copy_from_slice(&4_u16.to_be_bytes());
        data[50..52].copy_from_slice(&1_u16.to_be_bytes());

        data[54..56].copy_from_slice(&0x0041_u16.to_be_bytes());
        data[56..58].copy_from_slice(&u16::MAX.to_be_bytes());

        data[60..62].copy_from_slice(&0x0041_u16.to_be_bytes());
        data[62..64].copy_from_slice(&u16::MAX.to_be_bytes());

        data[64..66].copy_from_slice(&1_u16.to_be_bytes());
        data[66..68].copy_from_slice(&1_u16.to_be_bytes());

        data
    }

    fn cmap_format_12_font() -> Vec<u8> {
        let mut data = vec![0_u8; 68];

        data[0..4].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
        data[4..6].copy_from_slice(&1_u16.to_be_bytes());

        data[12..16].copy_from_slice(b"cmap");
        data[20..24].copy_from_slice(&28_u32.to_be_bytes());
        data[24..28].copy_from_slice(&40_u32.to_be_bytes());

        data[28..30].copy_from_slice(&0_u16.to_be_bytes());
        data[30..32].copy_from_slice(&1_u16.to_be_bytes());

        data[32..34].copy_from_slice(&0_u16.to_be_bytes());
        data[34..36].copy_from_slice(&4_u16.to_be_bytes());
        data[36..40].copy_from_slice(&12_u32.to_be_bytes());

        data[40..42].copy_from_slice(&12_u16.to_be_bytes());
        data[44..48].copy_from_slice(&28_u32.to_be_bytes());
        data[52..56].copy_from_slice(&1_u32.to_be_bytes());

        let crab = '🦀' as u32;
        data[56..60].copy_from_slice(&crab.to_be_bytes());
        data[60..64].copy_from_slice(&crab.to_be_bytes());
        data[64..68].copy_from_slice(&77_u32.to_be_bytes());

        data
    }
}
