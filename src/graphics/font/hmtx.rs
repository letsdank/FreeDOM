use super::{
    reader::{read_i16, read_u16},
    sfnt::{FontError, SfntFont, TableTag},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HorizontalMetric {
    pub advance_width: u16,
    pub left_side_bearing: i16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HorizontalHeader {
    pub ascender: i16,
    pub descender: i16,
    pub line_gap: i16,
    pub number_of_h_metrics: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HorizontalMetrics {
    header: HorizontalHeader,
    metrics: Vec<HorizontalMetric>,
}

impl HorizontalMetrics {
    pub fn parse(font: &SfntFont<'_>, num_glyphs: u16) -> Result<Self, FontError> {
        let hhea_record = font.table_record(TableTag::HHEA).ok_or_else(|| FontError {
            offset: 0,
            message: "missing required hhea table".to_owned(),
        })?;

        let hhea = font.table(TableTag::HHEA).ok_or_else(|| FontError {
            offset: hhea_record.offset,
            message: "cannot read hhea table".to_owned(),
        })?;

        if hhea.len() < 36 {
            return Err(error(
                hhea_record.offset,
                "hhea table is shorter than 36 bytes",
            ));
        }

        if read_i16(hhea, 32, hhea_record.offset)? != 0 {
            return Err(error(
                hhea_record.offset + 32,
                "unsupported hhea metricDataFormat value",
            ));
        }

        let header = HorizontalHeader {
            ascender: read_i16(hhea, 4, hhea_record.offset)?,
            descender: read_i16(hhea, 6, hhea_record.offset)?,
            line_gap: read_i16(hhea, 8, hhea_record.offset)?,
            number_of_h_metrics: read_u16(hhea, 34, hhea_record.offset)?,
        };

        let long_metric_count = usize::from(header.number_of_h_metrics);
        let glyph_count = usize::from(num_glyphs);

        if long_metric_count == 0 || long_metric_count > glyph_count {
            return Err(error(
                hhea_record.offset + 34,
                "invalid numberOfHMetrics value",
            ));
        }

        let hmtx_record = font.table_record(TableTag::HMTX).ok_or_else(|| FontError {
            offset: 0,
            message: "missing required hmtx table".to_owned(),
        })?;

        let hmtx = font.table(TableTag::HMTX).ok_or_else(|| FontError {
            offset: hmtx_record.offset,
            message: "cannot read hmtx table".to_owned(),
        })?;

        let long_metrics_length = long_metric_count
            .checked_mul(4)
            .ok_or_else(|| error(hmtx_record.offset, "hmtx table is too large"))?;

        let additional_bearings_length = glyph_count
            .checked_sub(long_metric_count)
            .and_then(|count| count.checked_mul(2))
            .ok_or_else(|| error(hmtx_record.offset, "hmtx table is too large"))?;

        let required_length = long_metrics_length
            .checked_add(additional_bearings_length)
            .ok_or_else(|| error(hmtx_record.offset, "hmtx table is too large"))?;

        if hmtx.len() < required_length {
            return Err(error(
                hmtx_record.offset + hmtx.len(),
                "truncated hmtx table",
            ));
        }

        let mut metrics = Vec::with_capacity(glyph_count);

        for index in 0..long_metric_count {
            let offset = index * 4;

            metrics.push(HorizontalMetric {
                advance_width: read_u16(hmtx, offset, hmtx_record.offset)?,
                left_side_bearing: read_i16(hmtx, offset + 2, hmtx_record.offset)?,
            });
        }

        let inherited_advance_width = metrics
            .last()
            .expect("long_metric_count was checked to be non-zero")
            .advance_width;

        for index in long_metric_count..glyph_count {
            let offset = long_metrics_length + (index - long_metric_count) * 2;

            metrics.push(HorizontalMetric {
                advance_width: inherited_advance_width,
                left_side_bearing: read_i16(hmtx, offset, hmtx_record.offset)?,
            });
        }

        Ok(Self { header, metrics })
    }

    pub fn glyph_count(&self) -> u16 {
        self.metrics.len().try_into().unwrap_or(u16::MAX)
    }

    pub fn metric(&self, glyph_id: u16) -> Option<HorizontalMetric> {
        self.metrics.get(usize::from(glyph_id)).copied()
    }

    pub fn header(&self) -> HorizontalHeader {
        self.header
    }
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
    fn parses_long_and_inherited_horizontal_metrics() {
        let data = font_with_metrics(
            2,
            &[
                0x01, 0xf4, 0x00, 0x0a, // glyph 0: advance 500, lsb 10
                0x02, 0x58, 0x00, 0x14, // glyph 1: advance 600, lsb 20
                0x00, 0x1e, // glyph 2: inherited advance 600, lsb 30
            ],
        );

        let font = SfntFont::parse(&data).unwrap();
        let metrics = HorizontalMetrics::parse(&font, 3).unwrap();

        assert_eq!(metrics.glyph_count(), 3);
        assert_eq!(
            metrics.metric(0),
            Some(HorizontalMetric {
                advance_width: 500,
                left_side_bearing: 10,
            })
        );
        assert_eq!(
            metrics.metric(2),
            Some(HorizontalMetric {
                advance_width: 600,
                left_side_bearing: 30,
            })
        );
        assert_eq!(
            metrics.header(),
            HorizontalHeader {
                ascender: 800,
                descender: -200,
                line_gap: 0,
                number_of_h_metrics: 2,
            }
        );
    }

    #[test]
    fn rejects_more_long_metrics_than_glyphs() {
        let data = font_with_metrics(4, &[]);
        let font = SfntFont::parse(&data).unwrap();

        let error = HorizontalMetrics::parse(&font, 3).unwrap_err();

        assert_eq!(error.message, "invalid numberOfHMetrics value");
    }

    fn font_with_metrics(number_of_h_metrics: u16, hmtx: &[u8]) -> Vec<u8> {
        let mut data = vec![0_u8; 80 + hmtx.len()];

        data[0..4].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
        data[4..6].copy_from_slice(&2_u16.to_be_bytes());

        data[12..16].copy_from_slice(b"hhea");
        data[20..24].copy_from_slice(&44_u32.to_be_bytes());
        data[24..28].copy_from_slice(&36_u32.to_be_bytes());

        data[28..32].copy_from_slice(b"hmtx");
        data[36..40].copy_from_slice(&80_u32.to_be_bytes());
        data[40..44].copy_from_slice(&(hmtx.len() as u32).to_be_bytes());

        data[48..50].copy_from_slice(&800_i16.to_be_bytes());
        data[50..52].copy_from_slice(&(-200_i16).to_be_bytes());
        data[52..54].copy_from_slice(&0_i16.to_be_bytes());

        data[78..80].copy_from_slice(&number_of_h_metrics.to_be_bytes());
        data[80..].copy_from_slice(hmtx);

        data
    }
}
