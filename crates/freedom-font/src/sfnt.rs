use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableTag([u8; 4]);

impl TableTag {
    pub const HEAD: Self = Self(*b"head");
    pub const MAXP: Self = Self(*b"maxp");
    pub const CMAP: Self = Self(*b"cmap");
    pub const LOCA: Self = Self(*b"loca");
    pub const GLYF: Self = Self(*b"glyf");
    pub const HHEA: Self = Self(*b"hhea");
    pub const HMTX: Self = Self(*b"hmtx");
    pub const NAME: Self = Self(*b"name");
    pub const KERN: Self = Self(*b"kern");
    pub const GPOS: Self = Self(*b"GPOS");

    pub const fn from_bytes(bytes: [u8; 4]) -> Self {
        Self(bytes)
    }

    pub const fn bytes(self) -> [u8; 4] {
        self.0
    }
}

impl fmt::Display for TableTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tag = std::str::from_utf8(&self.0).unwrap_or("????");
        write!(formatter, "{tag}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SfntFlavor {
    TrueType,
    Cff,
    AppleTrueType,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableRecord {
    pub tag: TableTag,
    pub checksum: u32,
    pub offset: usize,
    pub length: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontError {
    pub offset: usize,
    pub message: String,
}

impl FontError {
    fn new(offset: usize, message: impl Into<String>) -> Self {
        Self {
            offset,
            message: message.into(),
        }
    }
}

impl fmt::Display for FontError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid SFNT font at byte {}: {}",
            self.offset, self.message
        )
    }
}

impl Error for FontError {}

#[derive(Clone, Debug)]
pub struct SfntFont<'a> {
    data: &'a [u8],
    flavor: SfntFlavor,
    tables: Vec<TableRecord>,
}

impl<'a> SfntFont<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, FontError> {
        let flavor = match read_u32(data, 0)? {
            0x0001_0000 => SfntFlavor::TrueType,
            0x4f54_544f => SfntFlavor::Cff,
            0x7472_7565 => SfntFlavor::AppleTrueType,
            _ => return Err(FontError::new(0, "unsupported SFNT flavor")),
        };

        let table_count = read_u16(data, 4)? as usize;
        let directory_length = table_count
            .checked_mul(16)
            .and_then(|length| length.checked_add(12))
            .ok_or_else(|| FontError::new(4, "table directory is too large"))?;

        bytes_at(data, 0, directory_length)?;

        let mut tables = Vec::with_capacity(table_count);

        for index in 0..table_count {
            let record_offset = 12 + index * 16;

            let tag = TableTag::from_bytes([
                data[record_offset],
                data[record_offset + 1],
                data[record_offset + 2],
                data[record_offset + 3],
            ]);

            if tables.iter().any(|record: &TableRecord| record.tag == tag) {
                return Err(FontError::new(record_offset, "duplicate SFNT table tag"));
            }

            let checksum = read_u32(data, record_offset + 4)?;
            let offset = usize::try_from(read_u32(data, record_offset + 8)?)
                .map_err(|_| FontError::new(record_offset + 8, "table offset is too large"))?;

            let length = usize::try_from(read_u32(data, record_offset + 12)?)
                .map_err(|_| FontError::new(record_offset + 12, "table length is too large"))?;

            bytes_at(data, offset, length).map_err(|_| {
                FontError::new(
                    record_offset,
                    format!("table {tag} lies outside the font file"),
                )
            })?;

            tables.push(TableRecord {
                tag,
                checksum,
                offset,
                length,
            });
        }

        Ok(Self {
            data,
            flavor,
            tables,
        })
    }

    pub fn flavor(&self) -> SfntFlavor {
        self.flavor
    }

    pub fn tables(&self) -> &[TableRecord] {
        &self.tables
    }

    pub fn table_record(&self, tag: TableTag) -> Option<&TableRecord> {
        self.tables.iter().find(|record| record.tag == tag)
    }

    pub fn table(&self, tag: TableTag) -> Option<&'a [u8]> {
        let record = self.table_record(tag)?;
        self.data.get(record.offset..record.offset + record.length)
    }

    pub fn required_table(&self, tag: TableTag) -> Result<&'a [u8], FontError> {
        self.table(tag)
            .ok_or_else(|| FontError::new(0, format!("missing required {tag} table")))
    }
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16, FontError> {
    let bytes = bytes_at(data, offset, 2)?;

    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, FontError> {
    let bytes = bytes_at(data, offset, 4)?;

    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn bytes_at(data: &[u8], offset: usize, length: usize) -> Result<&[u8], FontError> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| FontError::new(offset, "integer overflow while reading font"))?;

    data.get(offset..end)
        .ok_or_else(|| FontError::new(end, "unexpected end of font file"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_sfnt_table_directory() {
        let data = minimal_true_type_font();
        let font = SfntFont::parse(&data).unwrap();

        assert_eq!(font.flavor(), SfntFlavor::TrueType);
        assert_eq!(font.tables().len(), 2);

        let head = font.table_record(TableTag::HEAD).unwrap();
        assert_eq!(head.offset, 44);
        assert_eq!(head.length, 54);

        assert_eq!(font.table(TableTag::MAXP).unwrap().iter().len(), 6);
    }

    #[test]
    fn rejects_tables_outside_the_font_file() {
        let mut data = minimal_true_type_font();

        data[24..28].copy_from_slice(&1000_u32.to_be_bytes());

        let error = SfntFont::parse(&data).unwrap_err();

        assert!(error.message.contains("outside the font file"));
    }

    #[test]
    fn rejects_unknown_font_flavors() {
        let mut data = minimal_true_type_font();

        data[0..4].copy_from_slice(b"NOPE");

        let error = SfntFont::parse(&data).unwrap_err();

        assert_eq!(error.message, "unsupported SFNT flavor");
    }

    fn minimal_true_type_font() -> Vec<u8> {
        let mut data = vec![0_u8; 104];

        data[0..4].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
        data[4..6].copy_from_slice(&2_u16.to_be_bytes());

        data[12..16].copy_from_slice(b"head");
        data[20..24].copy_from_slice(&44_u32.to_be_bytes());
        data[24..28].copy_from_slice(&54_u32.to_be_bytes());

        data[28..32].copy_from_slice(b"maxp");
        data[36..40].copy_from_slice(&98_u32.to_be_bytes());
        data[40..44].copy_from_slice(&6_u32.to_be_bytes());

        data
    }
}
