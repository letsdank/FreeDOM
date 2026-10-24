use std::ops::Range;

use super::{
    reader::{read_u16, read_u32},
    sfnt::{FontError, SfntFont, TableTag},
    truetype::IndexToLocFormat,
};

#[derive(Clone, Debug)]
pub struct GlyphLocations {
    offsets: Vec<usize>,
}

impl GlyphLocations {
    pub fn parse(
        font: &SfntFont<'_>,
        format: IndexToLocFormat,
        num_glyphs: u16,
    ) -> Result<Self, FontError> {
        let record = font.table_record(TableTag::LOCA).ok_or_else(|| FontError {
            offset: 0,
            message: "missing required loca table".to_owned(),
        })?;

        let table = font.table(TableTag::LOCA).ok_or_else(|| FontError {
            offset: record.offset,
            message: "cannot read loca table".to_owned(),
        })?;

        let entry_count = usize::from(num_glyphs)
            .checked_add(1)
            .ok_or_else(|| table_error(record.offset, "too many glyph locations"))?;

        let (entry_size, multiplier) = match format {
            IndexToLocFormat::Short => (2, 2),
            IndexToLocFormat::Long => (4, 1),
        };

        let required_length = entry_count
            .checked_mul(entry_size)
            .ok_or_else(|| table_error(record.offset, "loca table is too large"))?;

        if table.len() < required_length {
            return Err(table_error(
                record.offset + table.len(),
                "truncated loca table",
            ));
        }

        let mut offsets = Vec::with_capacity(entry_count);

        for index in 0..entry_count {
            let position = index * entry_size;
            let raw_offset = match format {
                IndexToLocFormat::Short => usize::from(read_u16(table, position, record.offset)?),
                IndexToLocFormat::Long => {
                    usize::try_from(read_u32(table, position, record.offset)?).map_err(|_| {
                        table_error(record.offset + position, "loca glyph offset is too large")
                    })?
                }
            };

            let offset = raw_offset.checked_mul(multiplier).ok_or_else(|| {
                table_error(record.offset + position, "loca glyph offset overflows")
            })?;

            if let Some(previous) = offsets.last() {
                if offset < *previous {
                    return Err(table_error(
                        record.offset + position,
                        "loca offsets are not monotonically increasing",
                    ));
                }
            }

            offsets.push(offset);
        }

        Ok(Self { offsets })
    }

    pub fn glyph_count(&self) -> u16 {
        self.offsets
            .len()
            .saturating_sub(1)
            .try_into()
            .unwrap_or(u16::MAX)
    }

    pub fn glyph_range(&self, glyph_id: u16) -> Option<Range<usize>> {
        let index = usize::from(glyph_id);

        Some(*self.offsets.get(index)?..*self.offsets.get(index + 1)?)
    }

    pub fn final_offset(&self) -> usize {
        self.offsets.last().copied().unwrap_or(0)
    }
}

fn table_error(offset: usize, message: impl Into<String>) -> FontError {
    FontError {
        offset,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_short_loca_offsets() {
        let data = font_with_loca(&[
            0, 0, // glyph 0 starts at byte 0
            0, 5, // glyph 1 starts at byte 10
            0, 7, // end of glyph 1 is byte 14
        ]);

        let font = SfntFont::parse(&data).unwrap();
        let locations = GlyphLocations::parse(&font, IndexToLocFormat::Short, 2).unwrap();

        assert_eq!(locations.glyph_count(), 2);
        assert_eq!(locations.glyph_range(0), Some(0..10));
        assert_eq!(locations.glyph_range(1), Some(10..14));
        assert_eq!(locations.glyph_range(2), None);
    }

    #[test]
    fn reads_long_loca_offsets() {
        let data = font_with_loca(&[
            0, 0, 0, 0, // glyph 0 starts at byte 0
            0, 0, 0, 9, // glyph 1 starts at byte 9
            0, 0, 1, 4, // end of glyph 1 is byte 260
        ]);

        let font = SfntFont::parse(&data).unwrap();
        let locations = GlyphLocations::parse(&font, IndexToLocFormat::Long, 2).unwrap();

        assert_eq!(locations.glyph_range(0), Some(0..9));
        assert_eq!(locations.glyph_range(1), Some(9..260));
    }

    #[test]
    fn rejects_descending_loca_offsets() {
        let data = font_with_loca(&[
            0, 0, // byte 0
            0, 4, // byte 8
            0, 3, // byte 6 - invalid
        ]);

        let font = SfntFont::parse(&data).unwrap();
        let error = GlyphLocations::parse(&font, IndexToLocFormat::Short, 2).unwrap_err();

        assert_eq!(
            error.message,
            "loca offsets are not monotonically increasing"
        );
    }

    fn font_with_loca(loca: &[u8]) -> Vec<u8> {
        let mut data = vec![0_u8; 28 + loca.len()];

        data[0..4].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
        data[4..6].copy_from_slice(&1_u16.to_be_bytes());

        data[12..16].copy_from_slice(b"loca");
        data[20..24].copy_from_slice(&28_u32.to_be_bytes());
        data[24..28].copy_from_slice(&(loca.len() as u32).to_be_bytes());

        data[28..].copy_from_slice(loca);

        data
    }
}
