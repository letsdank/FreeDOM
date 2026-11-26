//! Bit-level input for Brotli streams.
//!
//! Integer fields are read starting with their least significant bit.
//! Specification: RFC 7932, section 1.5.1.
//! <https://www.rfc-editor.org/rfc/rfc7931.html#section-1.5.1>

use super::{FontError, error};

#[derive(Clone, Debug)]
pub struct BitReader<'a> {
    data: &'a [u8],
    bit_position: usize,
}

impl<'a> BitReader<'a> {
    pub const fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            bit_position: 0,
        }
    }

    pub const fn bit_position(&self) -> usize {
        self.bit_position
    }

    pub fn bits_remaining(&self) -> usize {
        self.data
            .len()
            .saturating_mul(8)
            .saturating_sub(self.bit_position)
    }

    pub fn read_bit(&mut self) -> Result<bool, FontError> {
        Ok(self.read_bits(1)? != 0)
    }

    pub fn read_bits(&mut self, bit_count: usize) -> Result<u32, FontError> {
        if bit_count > 32 {
            return Err(error(
                self.bit_position,
                "Brotli bit reader cannot read more than 32 bits",
            ));
        }

        let end = self
            .bit_position
            .checked_add(bit_count)
            .ok_or_else(|| error(self.bit_position, "Brotli bit position overflows"))?;

        let total_bits = self
            .data
            .len()
            .checked_mul(8)
            .ok_or_else(|| error(self.bit_position, "Brotli input is too large"))?;

        if end > total_bits {
            return Err(error(self.bit_position, "truncated Brotli bit stream"));
        }

        let mut value = 0_u32;

        for bit_index in 0..bit_count {
            let source_bit = self.bit_position + bit_index;
            let byte = self.data[source_bit / 8];
            let bit = (byte >> (source_bit % 8)) & 1;

            value |= u32::from(bit) << bit_index;
        }

        self.bit_position = end;

        Ok(value)
    }

    pub fn align_to_byte(&mut self) -> Result<(), FontError> {
        let aligned = self
            .bit_position
            .checked_add(7)
            .ok_or_else(|| error(self.bit_position, "Brotli bit position overflows"))?
            & !7;

        let total_bits = self
            .data
            .len()
            .checked_mul(8)
            .ok_or_else(|| error(self.bit_position, "Brotli input is too large"))?;

        if aligned > total_bits {
            return Err(error(self.bit_position, "truncated Brotli bit stream"));
        }

        self.bit_position = aligned;

        Ok(())
    }

    pub fn read_aligned_bytes(&mut self, length: usize) -> Result<&'a [u8], FontError> {
        self.align_to_byte()?;

        let start = self.bit_position / 8;
        let end = start
            .checked_add(length)
            .ok_or_else(|| error(start, "Brotli byte range overflows"))?;

        let bytes = self
            .data
            .get(start..end)
            .ok_or_else(|| error(start, "truncated Brotli byte stream"))?;

        self.bit_position = end
            .checked_mul(8)
            .ok_or_else(|| error(end, "Brotli bit position overflows"))?;

        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_brotli_integer_fields_from_low_bits_first() {
        let mut reader = BitReader::new(&[0b1001_0110, 0b1100_0101]);

        assert_eq!(reader.read_bits(3).unwrap(), 6);
        assert_eq!(reader.read_bits(4).unwrap(), 2);
        assert_eq!(reader.bit_position(), 7);
        assert!(reader.read_bit().unwrap());
        assert_eq!(reader.read_bits(4).unwrap(), 5);
    }

    #[test]
    fn aligns_before_reading_byte_data() {
        let mut reader = BitReader::new(&[0b0000_0111, 0xaa, 0xbb]);

        assert_eq!(reader.read_bits(3).unwrap(), 7);
        assert_eq!(reader.read_aligned_bytes(2).unwrap(), &[0xaa, 0xbb]);
    }

    #[test]
    fn rejects_reads_past_the_end_of_the_input() {
        let mut reader = BitReader::new(&[0]);

        let error = reader.read_bits(9).unwrap_err();

        assert_eq!(error.message, "truncated Brotli bit stream");
    }
}
