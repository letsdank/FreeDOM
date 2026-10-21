use super::{
    cmap::CmapTable,
    glyf::{Glyph, parse_glyph},
    hmtx::{HorizontalMetric, HorizontalMetrics},
    loca::GlyphLocations,
    sfnt::{FontError, SfntFlavor, SfntFont, TableTag},
};

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
    cmap: CmapTable,
    locations: GlyphLocations,
    glyf: &'a [u8],
    glyf_offset: usize,
    metrics: HorizontalMetrics,
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
        let cmap = CmapTable::parse(&sfnt)?;
        let metrics = HorizontalMetrics::parse(&sfnt, maxp.num_glyphs)?;

        let locations = GlyphLocations::parse(&sfnt, head.index_to_loc_format, maxp.num_glyphs)?;

        let glyf_offset = sfnt
            .table_record(TableTag::GLYF)
            .ok_or_else(|| FontError {
                offset: 0,
                message: "missing required glyf table".to_owned(),
            })?
            .offset;

        let glyf = sfnt.table(TableTag::GLYF).ok_or_else(|| FontError {
            offset: glyf_offset,
            message: "cannot read glyf table".to_owned(),
        })?;

        if locations.final_offset() > glyf.len() {
            return Err(FontError {
                offset: glyf_offset + glyf.len(),
                message: "loca table points outside the glyf table".to_owned(),
            });
        }

        Ok(Self {
            sfnt,
            head,
            maxp,
            cmap,
            locations,
            glyf,
            glyf_offset,
            metrics,
        })
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

    pub fn cmap(&self) -> &CmapTable {
        &self.cmap
    }

    pub fn glyph_id(&self, character: char) -> u16 {
        self.cmap.glyph_id(character)
    }

    pub fn locations(&self) -> &GlyphLocations {
        &self.locations
    }

    pub fn glyph(&self, glyph_id: u16) -> Result<Glyph, FontError> {
        if glyph_id >= self.maxp.num_glyphs {
            return Err(FontError {
                offset: self.glyf_offset,
                message: "glyph ID lies outside the font".to_owned(),
            });
        }

        let range = self
            .locations
            .glyph_range(glyph_id)
            .ok_or_else(|| FontError {
                offset: self.glyf_offset,
                message: "missing glyph location".to_owned(),
            })?;

        let data = self.glyf.get(range.clone()).ok_or_else(|| FontError {
            offset: self.glyf_offset + range.start,
            message: "glyph data lies outside the glyf table".to_owned(),
        })?;

        parse_glyph(data, self.glyf_offset + range.start)
    }

    pub fn glyph_for(&self, character: char) -> Result<Glyph, FontError> {
        self.glyph(self.glyph_id(character))
    }

    pub fn metrics(&self) -> &HorizontalMetrics {
        &self.metrics
    }

    pub fn metric(&self, glyph_id: u16) -> Option<HorizontalMetric> {
        self.metrics.metric(glyph_id)
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
        assert_eq!(font.maxp().num_glyphs, 2);
        assert_eq!(font.metric(1).unwrap().advance_width, 600);
    }

    #[test]
    fn rejects_invalid_index_to_loc_format() {
        let mut data = minimal_true_type_font();

        data[174..176].copy_from_slice(&2_i16.to_be_bytes());

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

    #[test]
    fn maps_characters_to_glyph_ids() {
        let data = minimal_true_type_font();
        let font = TrueTypeFont::parse(&data).unwrap();

        assert_eq!(font.glyph_id('A'), 1);
        assert_eq!(font.glyph_id('B'), 0);
    }

    #[test]
    fn loads_a_simple_glyph_for_a_unicode_character() {
        let data = minimal_true_type_font();
        let font = TrueTypeFont::parse(&data).unwrap();

        let Glyph::Simple(glyph) = font.glyph_for('A').unwrap() else {
            panic!("expected a simple glyph");
        };

        assert_eq!(glyph.path.commands().len(), 4);
    }

    fn minimal_true_type_font() -> Vec<u8> {
        let mut data = vec![0_u8; 330];

        data[0..4].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
        data[4..6].copy_from_slice(&7_u16.to_be_bytes());

        data[12..16].copy_from_slice(b"head");
        data[20..24].copy_from_slice(&124_u32.to_be_bytes());
        data[24..28].copy_from_slice(&54_u32.to_be_bytes());

        data[28..32].copy_from_slice(b"maxp");
        data[36..40].copy_from_slice(&178_u32.to_be_bytes());
        data[40..44].copy_from_slice(&32_u32.to_be_bytes());

        data[44..48].copy_from_slice(b"cmap");
        data[52..56].copy_from_slice(&210_u32.to_be_bytes());
        data[56..60].copy_from_slice(&44_u32.to_be_bytes());

        data[60..64].copy_from_slice(b"loca");
        data[68..72].copy_from_slice(&254_u32.to_be_bytes());
        data[72..76].copy_from_slice(&12_u32.to_be_bytes());

        data[76..80].copy_from_slice(b"glyf");
        data[84..88].copy_from_slice(&266_u32.to_be_bytes());
        data[88..92].copy_from_slice(&20_u32.to_be_bytes());

        data[92..96].copy_from_slice(b"hhea");
        data[100..104].copy_from_slice(&286_u32.to_be_bytes());
        data[104..108].copy_from_slice(&36_u32.to_be_bytes());

        data[108..112].copy_from_slice(b"hmtx");
        data[116..120].copy_from_slice(&322_u32.to_be_bytes());
        data[120..124].copy_from_slice(&8_u32.to_be_bytes());

        data[136..140].copy_from_slice(&0x5f0f_3cf5_u32.to_be_bytes());
        data[142..144].copy_from_slice(&1000_u16.to_be_bytes());
        data[174..176].copy_from_slice(&1_i16.to_be_bytes());
        data[176..178].copy_from_slice(&0_i16.to_be_bytes());

        data[178..182].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
        data[182..184].copy_from_slice(&2_u16.to_be_bytes());

        data[210..212].copy_from_slice(&0_u16.to_be_bytes());
        data[212..214].copy_from_slice(&1_u16.to_be_bytes());
        data[214..216].copy_from_slice(&3_u16.to_be_bytes());
        data[216..218].copy_from_slice(&1_u16.to_be_bytes());
        data[218..222].copy_from_slice(&12_u32.to_be_bytes());

        data[222..224].copy_from_slice(&4_u16.to_be_bytes());
        data[224..226].copy_from_slice(&32_u16.to_be_bytes());
        data[228..230].copy_from_slice(&4_u16.to_be_bytes());
        data[230..232].copy_from_slice(&4_u16.to_be_bytes());
        data[232..234].copy_from_slice(&1_u16.to_be_bytes());

        data[236..238].copy_from_slice(&0x0041_u16.to_be_bytes());
        data[238..240].copy_from_slice(&u16::MAX.to_be_bytes());
        data[242..244].copy_from_slice(&0x0041_u16.to_be_bytes());
        data[244..246].copy_from_slice(&u16::MAX.to_be_bytes());
        data[246..248].copy_from_slice(&(-64_i16).to_be_bytes());
        data[248..250].copy_from_slice(&1_u16.to_be_bytes());

        data[254..258].copy_from_slice(&0_u32.to_be_bytes());
        data[258..262].copy_from_slice(&0_u32.to_be_bytes());
        data[262..266].copy_from_slice(&20_u32.to_be_bytes());

        data[266..268].copy_from_slice(&1_i16.to_be_bytes());
        data[276..278].copy_from_slice(&2_u16.to_be_bytes());

        data[280] = 0x31;
        data[281] = 0x33;
        data[282] = 0x27;
        data[283] = 50;
        data[284] = 25;
        data[285] = 80;

        data[320..322].copy_from_slice(&2_u16.to_be_bytes());

        data[322..324].copy_from_slice(&600_u16.to_be_bytes());
        data[324..326].copy_from_slice(&0_i16.to_be_bytes());
        data[326..328].copy_from_slice(&600_u16.to_be_bytes());
        data[328..330].copy_from_slice(&0_i16.to_be_bytes());

        data
    }
}
