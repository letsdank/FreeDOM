use super::sfnt::{FontError, SfntFont, TableTag};

#[derive(Clone, Debug)]
pub struct CmapTable {
    format4: CmapFormat4,
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

        let mut chosen = None;

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

            if format != 4 {
                continue;
            }

            if chosen
                .as_ref()
                .is_none_or(|(chosen_priority, _)| priority > *chosen_priority)
            {
                chosen = Some((priority, subtable_offset));
            }
        }

        let Some((_, subtable_offset)) = chosen else {
            return Err(error(
                record.offset,
                "font has not supported Unicode cmap format 4 subtable",
            ));
        };

        Self::parse_format4(table, subtable_offset, record.offset)
    }

    pub fn glyph_id(&self, character: char) -> u16 {
        let Ok(code) = u16::try_from(character as u32) else {
            return 0;
        };

        let Some(segment) = self
            .format4
            .segments
            .iter()
            .find(|segment| code <= segment.end)
        else {
            return 0;
        };

        if code < segment.start {
            return 0;
        }

        if segment.range_offset == 0 {
            return code.wrapping_add(segment.delta);
        }

        let glyph_offset = segment.range_offset_position
            + usize::from(segment.range_offset)
            + usize::from(code - segment.start) * 2;

        let glyph_id = u16::from_be_bytes([
            self.format4.data[glyph_offset],
            self.format4.data[glyph_offset + 1],
        ]);

        if glyph_id == 0 {
            0
        } else {
            glyph_id.wrapping_add(segment.delta)
        }
    }

    fn parse_format4(
        table: &[u8],
        subtable_offset: usize,
        table_offset: usize,
    ) -> Result<Self, FontError> {
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

        Ok(Self {
            format4: CmapFormat4 {
                data: format_data,
                segments,
            },
        })
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
}
