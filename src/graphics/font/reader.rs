use super::sfnt::FontError;

#[derive(Clone, Copy, Debug)]
pub(crate) struct FontReader<'a> {
    data: &'a [u8],
    base_offset: usize,
}

impl<'a> FontReader<'a> {
    pub(crate) const fn new(data: &'a [u8], base_offset: usize) -> Self {
        Self { data, base_offset }
    }

    pub(crate) fn bytes(&self, offset: usize, length: usize) -> Result<&'a [u8], FontError> {
        let end = offset
            .checked_add(length)
            .ok_or_else(|| self.error(offset, "font read offset overflows"))?;

        self.data
            .get(offset..end)
            .ok_or_else(|| self.error(offset, "truncated font data"))
    }

    pub(crate) fn u8(&self, offset: usize) -> Result<u8, FontError> {
        self.data
            .get(offset)
            .copied()
            .ok_or_else(|| self.error(offset, "truncated font data"))
    }

    pub(crate) fn u16(&self, offset: usize) -> Result<u16, FontError> {
        let bytes = self.bytes(offset, 2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    pub(crate) fn i16(&self, offset: usize) -> Result<i16, FontError> {
        let bytes = self.bytes(offset, 2)?;
        Ok(i16::from_be_bytes([bytes[0], bytes[1]]))
    }

    pub(crate) fn u32(&self, offset: usize) -> Result<u32, FontError> {
        let bytes = self.bytes(offset, 4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn error(&self, offset: usize, message: impl Into<String>) -> FontError {
        FontError {
            offset: self.base_offset.saturating_add(offset),
            message: message.into(),
        }
    }
}

pub(crate) fn read_bytes(
    data: &[u8],
    offset: usize,
    length: usize,
    base_offset: usize,
) -> Result<&[u8], FontError> {
    FontReader::new(data, base_offset).bytes(offset, length)
}

pub(crate) fn read_u8(data: &[u8], offset: usize, base_offset: usize) -> Result<u8, FontError> {
    FontReader::new(data, base_offset).u8(offset)
}

pub(crate) fn read_u16(data: &[u8], offset: usize, base_offset: usize) -> Result<u16, FontError> {
    FontReader::new(data, base_offset).u16(offset)
}

pub(crate) fn read_i16(data: &[u8], offset: usize, base_offset: usize) -> Result<i16, FontError> {
    FontReader::new(data, base_offset).i16(offset)
}

pub(crate) fn read_u32(data: &[u8], offset: usize, base_offset: usize) -> Result<u32, FontError> {
    FontReader::new(data, base_offset).u32(offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_big_endian_font_scalars_with_absolute_offsets() {
        let reader = FontReader::new(&[0x12, 0x34, 0xff, 0xfe, 0xab, 0xcd, 0xef, 0x01], 40);

        assert_eq!(reader.u8(0).unwrap(), 0x12);
        assert_eq!(reader.u16(0).unwrap(), 0x1234);
        assert_eq!(reader.i16(2).unwrap(), -2);
        assert_eq!(reader.u32(4).unwrap(), 0xabcd_ef01);
    }

    #[test]
    fn reports_absolute_offsets_for_truncated_data() {
        let reader = FontReader::new(&[0, 1, 2], 100);

        let error = reader.u32(1).unwrap_err();

        assert_eq!(error.offset, 101);
        assert_eq!(error.message, String::from("truncated font data"));
    }
}
