use std::collections::BTreeMap;

use super::sfnt::{FontError, SfntFont, TableTag};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KerningTable {
    adjustments: BTreeMap<(u16, u16), i16>,
}

impl KerningTable {
    pub fn parse_optional(font: &SfntFont<'_>) -> Result<Self, FontError> {
        let Some(record) = font.table_record(TableTag::KERN) else {
            return Ok(Self::default());
        };

        let table = font.table(TableTag::KERN).ok_or_else(|| FontError {
            offset: record.offset,
            message: "cannot read kern table".to_owned(),
        })?;

        if table.len() < 4 {
            return Err(error(record.offset, "kern table is shorter than 4 bytes"));
        }

        if read_u16(table, 0, record.offset)? != 0 {
            return Err(error(record.offset, "unsupported kern table version"));
        }

        let subtable_count = usize::from(read_u16(table, 2, record.offset)?);
        let mut cursor = 4;
        let mut adjustments = BTreeMap::new();

        for _ in 0..subtable_count {
            let subtable_length = usize::from(read_u16(table, cursor + 2, record.offset)?);

            if subtable_length < 6 {
                return Err(error(
                    record.offset + cursor,
                    "kern subtable is shorter than 6 bytes",
                ));
            }

            let subtable_end = cursor
                .checked_add(subtable_length)
                .ok_or_else(|| error(record.offset + cursor, "kern subtable length overflows"))?;

            if subtable_end > table.len() {
                return Err(error(record.offset + cursor, "truncated kern subtable"));
            }

            let coverage = read_u16(table, cursor + 4, record.offset)?;
            let format = coverage >> 8;
            let horizontal = coverage & 0x0001 != 0;
            let minimum = coverage & 0x0002 != 0;
            let cross_stream = coverage & 0x0004 != 0;
            let override_values = coverage & 0x0008 != 0;

            if format == 0 && horizontal && !minimum && !cross_stream {
                Self::parse_format_0(
                    &table[cursor + 6..subtable_end],
                    record.offset + cursor + 6,
                    override_values,
                    &mut adjustments,
                )?;
            }

            cursor = subtable_end;
        }

        Ok(Self { adjustments })
    }

    pub fn adjustment(&self, left_glyph: u16, right_glyph: u16) -> i16 {
        self.adjustments
            .get(&(left_glyph, right_glyph))
            .copied()
            .unwrap_or(0)
    }

    fn parse_format_0(
        data: &[u8],
        file_offset: usize,
        override_values: bool,
        adjustments: &mut BTreeMap<(u16, u16), i16>,
    ) -> Result<(), FontError> {
        if data.len() < 8 {
            return Err(error(file_offset, "kern format 0 subtable is too short"));
        }

        let pair_count = usize::from(read_u16(data, 0, file_offset)?);
        let required_length = 8_usize
            .checked_add(
                pair_count
                    .checked_mul(6)
                    .ok_or_else(|| error(file_offset, "too many kern pairs"))?,
            )
            .ok_or_else(|| error(file_offset, "too many kern pairs"))?;

        if data.len() < required_length {
            return Err(error(file_offset, "truncated kern format 0 pairs"));
        }

        let mut previous_pair = None;

        for index in 0..pair_count {
            let offset = 8 + index * 6;
            let pair = (
                read_u16(data, offset, file_offset)?,
                read_u16(data, offset + 2, file_offset)?,
            );
            let adjustment = read_i16(data, offset + 4, file_offset)?;

            if previous_pair.is_some_and(|previous| pair <= previous) {
                return Err(error(
                    file_offset + offset,
                    "kern pairs are not strictly ordered",
                ));
            }

            previous_pair = Some(pair);

            if override_values {
                adjustments.insert(pair, adjustment);
            } else {
                adjustments
                    .entry(pair)
                    .and_modify(|value| *value = value.saturating_add(adjustment))
                    .or_insert(adjustment);
            }
        }

        Ok(())
    }
}

fn read_u16(data: &[u8], offset: usize, file_offset: usize) -> Result<u16, FontError> {
    let bytes = read_bytes(data, offset, 2, file_offset)?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_i16(data: &[u8], offset: usize, file_offset: usize) -> Result<i16, FontError> {
    let bytes = read_bytes(data, offset, 2, file_offset)?;
    Ok(i16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_bytes<'a>(
    data: &'a [u8],
    offset: usize,
    length: usize,
    file_offset: usize,
) -> Result<&'a [u8], FontError> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| error(file_offset + offset, "kern offset overflows"))?;

    data.get(offset..end)
        .ok_or_else(|| error(file_offset + offset, "truncated kern table"))
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
    fn reads_format_0_horizontal_kerning_pairs() {
        let data = font_with_kern_table();
        let font = SfntFont::parse(&data).unwrap();
        let kern = KerningTable::parse_optional(&font).unwrap();

        assert_eq!(kern.adjustment(1, 2), -40);
        assert_eq!(kern.adjustment(2, 1), 0);
    }

    #[test]
    fn accepts_fonts_without_a_kern_table() {
        let data = vec![
            0, 1, 0, 0, // TrueType flavor
            0, 0, // table count
            0, 0, 0, 0, 0, 0,
        ];

        let font = SfntFont::parse(&data).unwrap();
        let kern = KerningTable::parse_optional(&font).unwrap();

        assert_eq!(kern.adjustment(1, 2), 0);
    }

    fn font_with_kern_table() -> Vec<u8> {
        let mut data = vec![0_u8; 52];

        data[0..4].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
        data[4..6].copy_from_slice(&1_u16.to_be_bytes());

        data[12..16].copy_from_slice(b"kern");
        data[20..24].copy_from_slice(&28_u32.to_be_bytes());
        data[24..28].copy_from_slice(&24_u32.to_be_bytes());

        data[28..30].copy_from_slice(&0_u16.to_be_bytes());
        data[30..32].copy_from_slice(&1_u16.to_be_bytes());

        data[34..36].copy_from_slice(&20_u16.to_be_bytes());
        data[36..38].copy_from_slice(&0x0001_u16.to_be_bytes());

        data[38..40].copy_from_slice(&1_u16.to_be_bytes());
        data[40..42].copy_from_slice(&6_u16.to_be_bytes());

        data[46..48].copy_from_slice(&1_u16.to_be_bytes());
        data[48..50].copy_from_slice(&2_u16.to_be_bytes());
        data[50..52].copy_from_slice(&(-40_i16).to_be_bytes());

        data
    }
}
