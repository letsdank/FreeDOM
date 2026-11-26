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

    /// Reads a WOFF2 255UInt16 value and returns its encoded length.
    ///
    /// Specification: WOFF 2.0, section 3.1.
    /// <https://www.w3.org/TR/WOFF2/>
    pub(crate) fn u255_u16(&self, offset: usize) -> Result<(u16, usize), FontError> {
        let code = self.u8(offset)?;

        match code {
            0..=252 => Ok((u16::from(code), 1)),

            253 => {
                let bytes = self.bytes(offset, 3)?;
                let value = u16::from_be_bytes([bytes[1], bytes[2]]);

                Ok((value, 3))
            }

            254 | 255 => {
                let bytes = self.bytes(offset, 2)?;

                let base = if code == 254 { 506_u16 } else { 253_u16 };

                Ok((base + u16::from(bytes[1]), 2))
            }
        }
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

pub(crate) fn read_bytes_from_start(
    data: &[u8],
    offset: usize,
    length: usize,
) -> Result<&[u8], FontError> {
    read_bytes(data, offset, length, 0)
}

pub(crate) fn read_u16_from_start(data: &[u8], offset: usize) -> Result<u16, FontError> {
    read_u16(data, offset, 0)
}

pub(crate) fn read_u32_from_start(data: &[u8], offset: usize) -> Result<u32, FontError> {
    read_u32(data, offset, 0)
}

pub(crate) fn checked_add(offset: usize, amount: usize, message: &str) -> Result<usize, FontError> {
    offset
        .checked_add(amount)
        .ok_or_else(|| utility_error(offset, message))
}

pub(crate) fn checked_end(
    offset: usize,
    length: usize,
    limit: usize,
    message: &str,
) -> Result<usize, FontError> {
    let end = checked_add(offset, length, message)?;

    if end > limit {
        return Err(utility_error(offset, message));
    }

    Ok(end)
}

pub(crate) fn checked_range(
    data: &[u8],
    offset: usize,
    length: usize,
    message: &str,
) -> Result<std::ops::Range<usize>, FontError> {
    Ok(offset..checked_end(offset, length, data.len(), message)?)
}

pub(crate) fn padded_length(length: usize) -> Result<usize, FontError> {
    length
        .checked_add(3)
        .map(|length| length & !3)
        .ok_or_else(|| utility_error(length, "font length overflows"))
}

pub(crate) fn to_usize(value: u32, offset: usize) -> Result<usize, FontError> {
    usize::try_from(value).map_err(|_| utility_error(offset, "font value is too large"))
}

pub(crate) fn write_u16(data: &mut [u8], offset: usize, value: u16) -> Result<(), FontError> {
    let range = checked_range(data, offset, 2, "truncated font output")?;
    data[range].copy_from_slice(&value.to_be_bytes());
    Ok(())
}

pub(crate) fn write_u32(data: &mut [u8], offset: usize, value: u32) -> Result<(), FontError> {
    let range = checked_range(data, offset, 4, "truncated font output")?;
    data[range].copy_from_slice(&value.to_be_bytes());
    Ok(())
}

fn utility_error(offset: usize, message: impl Into<String>) -> FontError {
    FontError {
        offset,
        message: message.into(),
    }
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

    #[test]
    fn writes_big_endian_font_scalars() {
        let mut data = [0_u8; 6];

        write_u16(&mut data, 0, 0x1234).unwrap();
        write_u32(&mut data, 2, 0xabcd_ef01).unwrap();

        assert_eq!(data, [0x12, 0x34, 0xab, 0xcd, 0xef, 0x01]);
    }

    #[test]
    fn reads_woff2_255_u16_values() {
        let cases: &[(&[u8], u16, usize)] = &[
            (&[0], 0, 1),
            (&[252], 252, 1),
            (&[255, 0], 253, 2),
            (&[255, 253], 506, 2),
            (&[254, 0], 506, 2),
            (&[253, 1, 250], 506, 3),
            (&[254, 255], 761, 2),
            (&[253, 255, 255], 65535, 3),
        ];

        for &(bytes, expected, length) in cases {
            let reader = FontReader::new(bytes, 0);

            assert_eq!(
                reader.u255_u16(0).unwrap(),
                (expected, length),
                "input: {bytes:?}"
            );
        }
    }

    #[test]
    fn rejects_truncated_woff2_255_u16() {
        let cases: &[&[u8]] = &[&[], &[253], &[253, 1], &[254], &[255]];

        for &bytes in cases {
            let reader = FontReader::new(bytes, 100);
            let error = reader.u255_u16(0).unwrap_err();

            assert_eq!(error.offset, 100);
            assert_eq!(error.message, "truncated font data");
        }
    }
}
