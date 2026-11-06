//! Bit access to Huffman-coded JPEG scan data.
//!
//! ITU-T T.81, sections C.3 and F.1.2.3:
//! <https://www.w3.org/Graphics/JPEG/itu-t81.pdf>

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntropyError {
    /// Byte offset relative to the slice passed to `EntropyReader::new`.
    pub offset: usize,
    pub kind: EntropyErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntropyErrorKind {
    Truncated,
    Marker(u8),
    InvalidByteStuffing,
    InvalidBitCount(u8),
    InvalidPadding,
    ExpectedMarker,
}

impl std::fmt::Display for EntropyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "JPEG entropy error at byte {}: {:?}",
            self.offset, self.kind
        )
    }
}

impl std::error::Error for EntropyError {}

/// Reads MSB first. Marker bytes remain unconsumed when an error is returned.
pub struct EntropyReader<'a> {
    input: &'a [u8],
    cursor: usize,
    byte: u8,
    bits_left: u8,
}

impl<'a> EntropyReader<'a> {
    pub fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            cursor: 0,
            byte: 0,
            bits_left: 0,
        }
    }

    /// Position of the next unread input byte, including stuffed bytes.
    pub fn byte_position(&self) -> usize {
        self.cursor
    }

    pub fn read_bit(&mut self) -> Result<u8, EntropyError> {
        if self.bits_left == 0 {
            let error = |kind| EntropyError {
                offset: self.cursor,
                kind,
            };

            let byte = *self
                .input
                .get(self.cursor)
                .ok_or_else(|| error(EntropyErrorKind::Truncated))?;

            let mut consumed = 1;

            if byte == 0xff {
                let mut index = self.cursor + 1;

                while self.input.get(index) == Some(&0xff) {
                    index += 1;
                }

                let next = *self
                    .input
                    .get(index)
                    .ok_or_else(|| error(EntropyErrorKind::Truncated))?;

                if next == 0 {
                    if index != self.cursor + 1 {
                        return Err(error(EntropyErrorKind::InvalidByteStuffing));
                    }

                    consumed = 2;
                } else {
                    return Err(error(EntropyErrorKind::Marker(next)));
                }
            }

            self.cursor += consumed;
            self.byte = byte;
            self.bits_left = 8;
        }

        self.bits_left -= 1;

        Ok((self.byte >> self.bits_left) & 1)
    }

    /// Reads up to 16 bits; earlier bits stay consumed if a later bit fails.
    pub fn read_bits(&mut self, count: u8) -> Result<u16, EntropyError> {
        if count > 16 {
            return Err(EntropyError {
                offset: self.cursor,
                kind: EntropyErrorKind::InvalidBitCount(count),
            });
        }

        let mut result = 0u16;

        for _ in 0..count {
            result = (result << 1) | u16::from(self.read_bit()?);
        }

        Ok(result)
    }

    /// Checks one-bit padding and peeks at the following marker without consuming it.
    pub fn peek_scan_end_marker(&self) -> Result<u8, EntropyError> {
        let error = |kind| EntropyError {
            offset: self.cursor,
            kind,
        };

        let mask = (1u16 << self.bits_left) - 1;

        if u16::from(self.byte) & mask != mask {
            return Err(error(EntropyErrorKind::InvalidPadding));
        }

        if self.input.get(self.cursor) != Some(&0xff) {
            return Err(error(if self.cursor == self.input.len() {
                EntropyErrorKind::Truncated
            } else {
                EntropyErrorKind::ExpectedMarker
            }));
        }

        let mut index = self.cursor + 1;

        while self.input.get(index) == Some(&0xff) {
            index += 1;
        }

        let marker = *self
            .input
            .get(index)
            .ok_or_else(|| error(EntropyErrorKind::Truncated))?;

        if marker == 0 {
            return Err(error(EntropyErrorKind::ExpectedMarker));
        }

        Ok(marker)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_msb_first_and_unstuffs_ff() {
        let mut reader = EntropyReader::new(&[0xa6, 0xff, 0x00, 0x80]);

        assert_eq!(reader.read_bits(4).unwrap(), 0xa);
        assert_eq!(reader.read_bits(4).unwrap(), 6);
        assert_eq!(reader.read_bits(8).unwrap(), 0xff);
        assert_eq!(reader.byte_position(), 3);
        assert_eq!(reader.read_bit().unwrap(), 1);
    }

    #[test]
    fn leaves_marker_and_fill_unconsumed() {
        let mut reader = EntropyReader::new(&[0x42, 0xff, 0xff, 0xd0]);

        assert_eq!(reader.read_bits(8).unwrap(), 0x42);

        let expected = EntropyError {
            offset: 1,
            kind: EntropyErrorKind::Marker(0xd0),
        };

        assert_eq!(reader.read_bit(), Err(expected.clone()));
        assert_eq!(reader.byte_position(), 1);
        assert_eq!(reader.read_bit(), Err(expected));
    }

    #[test]
    fn handles_truncation_and_invalid_stuffing() {
        let mut reader = EntropyReader::new(&[0xff]);

        assert_eq!(
            reader.read_bit().unwrap_err().kind,
            EntropyErrorKind::Truncated
        );

        assert_eq!(reader.byte_position(), 0);

        let mut reader = EntropyReader::new(&[0xff, 0xff, 0x00]);

        assert_eq!(
            reader.read_bit().unwrap_err().kind,
            EntropyErrorKind::InvalidByteStuffing
        );

        assert_eq!(reader.byte_position(), 0);

        assert_eq!(
            reader.read_bits(17).unwrap_err().kind,
            EntropyErrorKind::InvalidBitCount(17)
        );
    }

    #[test]
    fn peeks_scan_end_only_after_one_bit_padding() {
        let mut reader = EntropyReader::new(&[0x5f, 0xff, 0xff, 0xd9]);

        assert_eq!(reader.read_bits(3).unwrap(), 2);
        assert_eq!(reader.peek_scan_end_marker().unwrap(), 0xd9);
        assert_eq!(reader.byte_position(), 1);
        assert_eq!(reader.peek_scan_end_marker().unwrap(), 0xd9);

        let mut reader = EntropyReader::new(&[0x40, 0xff, 0xd9]);

        reader.read_bits(3).unwrap();

        assert_eq!(
            reader.peek_scan_end_marker().unwrap_err().kind,
            EntropyErrorKind::InvalidPadding
        );

        let reader = EntropyReader::new(&[0xff, 0x00]);

        assert_eq!(
            reader.peek_scan_end_marker().unwrap_err().kind,
            EntropyErrorKind::ExpectedMarker
        );
    }
}
