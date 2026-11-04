//! Bit-level output for DEFLATE streams.
//!
//! RFC 1951, section 3.1.1:
//! <https://www.rfc-editor.org/rfc/rfc1951#section-3.1.1>
//!
//! Integer fields are written least-significant bit first. Huffman codes
//! require their own bit-order handling.

use std::io::{self, Write};

pub struct DeflateBitWriter<W> {
    inner: W,
    bits: u64,
    pending: u8,
}

impl<W: Write> DeflateBitWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            bits: 0,
            pending: 0,
        }
    }

    /// Writes the low `count` bits of `value`, least-significant bit first.
    ///
    /// Rejects counts above 32 before changing state. After an I/O error,
    /// the output may be partial; the writer must not be reused.
    pub fn write_bits(&mut self, value: u32, count: u8) -> io::Result<()> {
        if count > 32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot write more than 32 bits",
            ));
        }

        let mask = (1u64 << count) - 1;

        self.bits |= (u64::from(value) & mask) << self.pending;
        self.pending += count;

        while self.pending >= 8 {
            self.inner.write_all(&[self.bits as u8])?;

            self.bits >>= 8;
            self.pending -= 8;
        }

        Ok(())
    }

    /// Pads with zero bits to the next byte boundary, if necessary.
    ///
    /// Does not flush the underlying writer.
    pub fn align_to_byte(&mut self) -> io::Result<()> {
        if self.pending != 0 {
            self.inner.write_all(&[self.bits as u8])?;

            self.bits = 0;
            self.pending = 0;
        }

        Ok(())
    }

    /// Pads the last byte and returns the underlying writer.
    ///
    /// Does not flush the underlying writer.
    pub fn finish(mut self) -> io::Result<W> {
        self.align_to_byte()?;

        Ok(self.inner)
    }
}

#[cfg(test)]
#[path = "writer_tests.rs"]
mod tests;
