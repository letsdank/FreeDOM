//! Bounds-checked primitives for the WebAssembly binary format.
//!
//! <https://webassembly.github.io/spec/core/binary/values.html#integers>

use super::{BinaryError, BinaryErrorKind};

pub(super) struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
    origin: usize,
}

impl<'a> Reader<'a> {
    pub(super) fn new(bytes: &'a [u8], origin: usize) -> Self {
        Self {
            bytes,
            position: 0,
            origin,
        }
    }

    pub(super) fn offset(&self) -> usize {
        self.origin + self.position
    }

    pub(super) fn is_empty(&self) -> bool {
        self.position == self.bytes.len()
    }

    pub(super) fn remaining(&self) -> &'a [u8] {
        &self.bytes[self.position..]
    }

    pub(super) fn read_bytes(&mut self, length: usize) -> Result<&'a [u8], BinaryError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or_else(|| BinaryError::new(self.offset(), BinaryErrorKind::SizeOverflow))?;
        let result = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| BinaryError::new(self.offset(), BinaryErrorKind::UnexpectedEnd))?;
        self.position = end;
        Ok(result)
    }

    pub(super) fn read_byte(&mut self) -> Result<u8, BinaryError> {
        Ok(self.read_bytes(1)?[0])
    }

    /// Accepts redundant zero groups, but rejects encodings exceeding u32.
    pub(super) fn read_u32_leb(&mut self) -> Result<u32, BinaryError> {
        let start = self.offset();
        let mut value = 0_u32;
        for index in 0..5 {
            let byte = self.read_byte()?;
            if index == 4 && byte & 0xf0 != 0 {
                return Err(BinaryError::new(
                    start,
                    BinaryErrorKind::InvalidUnsignedLeb128,
                ));
            }
            value |= u32::from(byte & 0x7f) << (index * 7);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        unreachable!("a fifth byte with continuation is rejected")
    }

    /// Accepts redundant groups within the ten-byte u64 limit.
    pub(super) fn read_u64_leb(&mut self) -> Result<u64, BinaryError> {
        let start = self.offset();
        let mut value = 0_u64;
        for index in 0..10 {
            let byte = self.read_byte()?;
            if index == 9 && byte & 0xfe != 0 {
                return Err(BinaryError::new(
                    start,
                    BinaryErrorKind::InvalidUnsignedLeb64,
                ));
            }
            value |= u64::from(byte & 0x7f) << (index * 7);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        unreachable!("a tenth byte with continuation is rejected")
    }

    /// Accepts redundant sign-extension groups within the five-byte limit.
    pub(super) fn read_i32_leb(&mut self) -> Result<i32, BinaryError> {
        let start = self.offset();
        let mut value = 0_i64;
        for index in 0..5 {
            let byte = self.read_byte()?;
            if index == 4 && (byte & 0x80 != 0 || !matches!(byte & 0x78, 0 | 0x78)) {
                return Err(BinaryError::new(
                    start,
                    BinaryErrorKind::InvalidSignedLeb128,
                ));
            }
            let shift = 7 * index;
            value |= i64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                if byte & 0x40 != 0 {
                    value |= !0_i64 << (shift + 7);
                }
                return Ok(value as i32);
            }
        }
        unreachable!("a fifth byte with continuation is rejected")
    }
}

#[cfg(test)]
mod tests {
    use super::{BinaryErrorKind, Reader};

    #[test]
    fn reads_u32_leb_at_boundaries_and_rejects_invalid_encodings() {
        for (bytes, expected) in [
            (&[0x00][..], 0),
            (&[0x83, 0x00][..], 3),
            (&[0xff, 0xff, 0xff, 0xff, 0x0f][..], u32::MAX),
            (&[0x80, 0x80, 0x80, 0x80, 0x00][..], 0),
        ] {
            let mut reader = Reader::new(bytes, 7);
            assert_eq!(reader.read_u32_leb().unwrap(), expected);
            assert!(reader.is_empty());
        }

        for bytes in [
            &[0xff, 0xff, 0xff, 0xff, 0x10][..],
            &[0x80, 0x80, 0x80, 0x80, 0x80][..],
        ] {
            let error = Reader::new(bytes, 7).read_u32_leb().unwrap_err();
            assert_eq!(error.offset, 7);
            assert_eq!(error.kind, BinaryErrorKind::InvalidUnsignedLeb128);
        }

        let error = Reader::new(&[0x80], 7).read_u32_leb().unwrap_err();
        assert_eq!(error.offset, 8);
        assert_eq!(error.kind, BinaryErrorKind::UnexpectedEnd);
    }

    #[test]
    fn reads_u64_leb_boundaries_and_rejects_overflow() {
        for (bytes, expected) in [
            (&[0x80, 0x80, 0x80, 0x80, 0x80, 0x00][..], 0),
            (
                &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01][..],
                u64::MAX,
            ),
        ] {
            let mut reader = Reader::new(bytes, 7);
            assert_eq!(reader.read_u64_leb().unwrap(), expected);
            assert!(reader.is_empty());
        }
        for bytes in [
            &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x02][..],
            &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80][..],
        ] {
            let error = Reader::new(bytes, 7).read_u64_leb().unwrap_err();
            assert_eq!(
                (error.offset, error.kind),
                (7, BinaryErrorKind::InvalidUnsignedLeb64)
            );
        }
    }

    #[test]
    fn reads_signed_leb_boundaries_and_rejects_invalid_sign_extension() {
        for (bytes, expected) in [
            (&[0x00][..], 0),
            (&[0x3f][..], 63),
            (&[0x40][..], -64),
            (&[0x7f][..], -1),
            (&[0xff, 0x7f][..], -1),
            (&[0x80, 0x00][..], 0),
            (&[0xff, 0xff, 0xff, 0xff, 0x07][..], i32::MAX),
            (&[0x80, 0x80, 0x80, 0x80, 0x78][..], i32::MIN),
        ] {
            let mut reader = Reader::new(bytes, 7);
            assert_eq!(reader.read_i32_leb().unwrap(), expected);
            assert!(reader.is_empty());
        }
        for bytes in [
            &[0x80, 0x80, 0x80, 0x80, 0x08][..],
            &[0xff, 0xff, 0xff, 0xff, 0x77][..],
            &[0x80, 0x80, 0x80, 0x80, 0x80][..],
        ] {
            let error = Reader::new(bytes, 7).read_i32_leb().unwrap_err();
            assert_eq!(
                (error.offset, error.kind),
                (7, BinaryErrorKind::InvalidSignedLeb128)
            );
        }
        let error = Reader::new(&[0x80], 7).read_i32_leb().unwrap_err();
        assert_eq!(
            (error.offset, error.kind),
            (8, BinaryErrorKind::UnexpectedEnd)
        );
    }
}
