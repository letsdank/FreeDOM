use super::sfnt::{FontError, SfntFlavor, SfntFont, TableTag};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexToLocFormat {
    Short,
    Long,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeadTable {
    pub units_per_em: u16,
    pub index_to_loc_format: IndexToLocFormat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaxpTable {
    pub num_glyphs: u16,
}

#[derive(Clone, Debug)]
pub struct TrueTypeFont<'a> {
    sfnt: SfntFont<'a>,
    head: HeadTable,
    maxp: MaxpTable,
}

impl<'a> TrueTypeFont<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, FontError> {
        let sfnt = SfntFont::parse(data)?;

        if !matches!(
            sfnt.flavor(),
            SfntFlavor::TrueType | SfntFlavor::AppleTrueType
        ) {
            return Err(FontError {
                offset: 0,
                message: "font does not contain TrueType outlines".to_owned(),
            });
        }

        let head = HeadTable::parse(&sfnt)?;
        let maxp = MaxpTable::parse(&sfnt)?;

        Ok(Self { sfnt, head, maxp })
    }

    pub fn sfnt(&self) -> &SfntFont<'a> {
        &self.sfnt
    }

    pub fn head(&self) -> HeadTable {
        self.head
    }

    pub fn maxp(&self) -> MaxpTable {
        self.maxp
    }
}

impl HeadTable {
    pub fn parse(font: &SfntFont<'_>) -> Result<Self, FontError> {
        let (table, offset) = required_table(font, TableTag::HEAD)?;

        if table.len() < 54 {
            return Err(table_error(offset, "head table is shorter than 54 bytes"));
        }

        let magic_number = read_u32(table, 12, offset)?;

        if magic_number != 0x5f0f_3cf5 {
            return Err(table_error(offset + 12, "invalid head magic number"));
        }

        let units_per_em = read_u16(table, 18, offset)?;

        if !(16..16_384).contains(&units_per_em) {
            return Err(table_error(offset + 18, "invalid unitsPerEm value"));
        }

        let index_to_loc_format = match read_i16(table, 50, offset)? {
            0 => IndexToLocFormat::Short,
            1 => IndexToLocFormat::Long,
            _ => return Err(table_error(offset + 50, "invalid indexToLocFormat value")),
        };

        if read_i16(table, 52, offset)? != 0 {
            return Err(table_error(
                offset + 52,
                "unsupported glyphDataFormat value",
            ));
        }

        Ok(Self {
            units_per_em,
            index_to_loc_format,
        })
    }
}

impl MaxpTable {
    pub fn parse(font: &SfntFont<'_>) -> Result<Self, FontError> {
        let (table, offset) = required_table(font, TableTag::MAXP)?;

        if table.len() < 32 {
            return Err(table_error(
                offset,
                "TrueType maxp table is shorter than 32 bytes",
            ));
        }

        if read_u32(table, 0, offset)? != 0x0001_0000 {
            return Err(table_error(
                offset,
                "unsupported maxp version for TrueType outlines",
            ));
        }

        Ok(Self {
            num_glyphs: read_u16(table, 4, offset)?,
        })
    }
}

fn required_table<'a>(
    font: &'a SfntFont<'a>,
    tag: TableTag,
) -> Result<(&'a [u8], usize), FontError> {
    let record = font.table_record(tag).ok_or_else(|| FontError {
        offset: 0,
        message: format!("missing required {tag} table"),
    })?;

    let table = font.table(tag).ok_or_else(|| FontError {
        offset: record.offset,
        message: format!("cannot read {tag} table"),
    })?;

    Ok((table, record.offset))
}

fn read_u16(table: &[u8], relative_offset: usize, table_offset: usize) -> Result<u16, FontError> {
    let bytes = read_bytes(table, relative_offset, 2, table_offset)?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_i16(table: &[u8], relative_offset: usize, table_offset: usize) -> Result<i16, FontError> {
    let bytes = read_bytes(table, relative_offset, 2, table_offset)?;
    Ok(i16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_u32(table: &[u8], relative_offset: usize, table_offset: usize) -> Result<u32, FontError> {
    let bytes = read_bytes(table, relative_offset, 4, table_offset)?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_bytes<'a>(
    table: &'a [u8],
    relative_offset: usize,
    length: usize,
    table_offset: usize,
) -> Result<&'a [u8], FontError> {
    let end = relative_offset
        .checked_add(length)
        .ok_or_else(|| table_error(table_offset, "table offset overflow"))?;

    table
        .get(relative_offset..end)
        .ok_or_else(|| table_error(table_offset + relative_offset, "truncated table"))
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
    fn reads_true_type_head_and_maxp_tables() {
        let data = minimal_true_type_font();
        let font = TrueTypeFont::parse(&data).unwrap();

        assert_eq!(font.head().units_per_em, 1000);
        assert_eq!(font.head().index_to_loc_format, IndexToLocFormat::Long);
        assert_eq!(font.maxp().num_glyphs, 42);
    }

    #[test]
    fn rejects_invalid_index_to_loc_format() {
        let mut data = minimal_true_type_font();

        data[94..96].copy_from_slice(&2_i16.to_be_bytes());

        let error = TrueTypeFont::parse(&data).unwrap_err();

        assert_eq!(error.message, "invalid indexToLocFormat value");
    }

    #[test]
    fn rejects_cff_when_true_type_outlines_are_required() {
        let mut data = minimal_true_type_font();

        data[0..4].copy_from_slice(b"OTTO");

        let error = TrueTypeFont::parse(&data).unwrap_err();

        assert_eq!(error.message, "font does not contain TrueType outlines");
    }

    fn minimal_true_type_font() -> Vec<u8> {
        let mut data = vec![0_u8; 130];

        data[0..4].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
        data[4..6].copy_from_slice(&2_u16.to_be_bytes());

        data[12..16].copy_from_slice(b"head");
        data[20..24].copy_from_slice(&44_u32.to_be_bytes());
        data[24..28].copy_from_slice(&54_u32.to_be_bytes());

        data[28..32].copy_from_slice(b"maxp");
        data[36..40].copy_from_slice(&98_u32.to_be_bytes());
        data[40..44].copy_from_slice(&32_u32.to_be_bytes());

        data[56..60].copy_from_slice(&0x5f0f_3cf5_u32.to_be_bytes());
        data[62..64].copy_from_slice(&1000_u16.to_be_bytes());
        data[94..96].copy_from_slice(&1_i16.to_be_bytes());
        data[96..98].copy_from_slice(&0_i16.to_be_bytes());

        data[98..102].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
        data[102..104].copy_from_slice(&42_u16.to_be_bytes());

        data
    }
}
