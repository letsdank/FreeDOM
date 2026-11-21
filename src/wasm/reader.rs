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
}
