//! Bounded DEFLATE output and LZ77 history.
//!
//! RFC 1951, sections 3.2.3 and 3.2.5:
//! <https://www.rfc-editor.org/rfc/rfc1951#section-3.2.3>

use super::super::Adler32Writer;
use std::io::{self, Write};

const MAX_WINDOW: usize = 32_768;

/// Maintains output history and checksum across all blocks in a zlib stream.
pub struct DeflateOutput<W> {
    writer: Adler32Writer<W>,
    history: Vec<u8>,
    next: usize,
    history_len: usize,
    written: usize,
    limit: usize,
}

impl<W: Write> DeflateOutput<W> {
    /// `window_size` should come from the validated zlib header.
    pub fn new(writer: W, window_size: usize, limit: usize) -> io::Result<Self> {
        if !(1..=MAX_WINDOW).contains(&window_size) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid DEFLATE window size",
            ));
        }

        Ok(Self {
            writer: Adler32Writer::new(writer),
            history: vec![0; window_size],
            next: 0,
            history_len: 0,
            written: 0,
            limit,
        })
    }

    pub fn bytes_written(&self) -> usize {
        self.written
    }

    pub fn checksum(&self) -> u32 {
        self.writer.checksum()
    }

    pub fn into_inner(self) -> W {
        self.writer.into_inner()
    }

    /// Copies a match, including overlapping matches.
    ///
    /// Errors from the underlying writer can leave partial output. Invalid
    /// matches and output-limit errors are rejected before writing.
    pub fn copy_match(&mut self, distance: usize, length: usize) -> io::Result<()> {
        if distance == 0 || distance > self.history_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid DEFLATE backward distance",
            ));
        }

        if !(3..=258).contains(&length) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid DEFLATE match length",
            ));
        }

        if length > self.limit - self.written {
            return Err(io::Error::other("DEFLATE output limit exceeded"));
        }

        for _ in 0..length {
            let index = (self.next + self.history.len() - distance) % self.history.len();

            let byte = self.history[index];
            self.write_all(&[byte])?;
        }

        Ok(())
    }
}

impl<W: Write> Write for DeflateOutput<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit - self.written {
            return Err(io::Error::other("DEFLATE output limit exceeded"));
        }

        let written = self.writer.write(bytes)?;

        for &byte in &bytes[..written] {
            self.history[self.next] = byte;
            self.next = (self.next + 1) % self.history.len();
            self.history_len = (self.history_len + 1).min(self.history.len());
        }

        self.written += written;

        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::graphics::image::png_format::{
        DeflateBitReader, copy_stored_block, read_block_header,
    };

    #[test]
    fn copies_overlapping_matches_and_updates_checksum() {
        let mut output = DeflateOutput::new(Vec::new(), 256, 9).unwrap();

        output.write_all(b"abc").unwrap();
        output.copy_match(3, 6).unwrap();

        assert_eq!(output.bytes_written(), 9);
        assert_eq!(output.checksum(), 0x113d_0373);
        assert_eq!(output.into_inner(), b"abcabcabc");
    }

    #[test]
    fn preserves_stored_block_history_for_a_later_match() {
        // Non-final stored block containing "abc".
        let bytes = [0, 3, 0, 0xfc, 0xff, b'a', b'b', b'c'];
        let mut bits = DeflateBitReader::new(&bytes[..]);

        read_block_header(&mut bits).unwrap();

        let mut output = DeflateOutput::new(Vec::new(), 256, 9).unwrap();

        copy_stored_block(&mut bits, &mut output, 6).unwrap();

        // A later compressed block can refer to the stored block's data.
        output.copy_match(3, 3).unwrap();

        assert_eq!(output.into_inner(), b"abcabc");
    }

    #[test]
    fn wraps_history_and_respects_window_size() {
        // A small synthetic window makes wraparound easy to test.
        let mut output = DeflateOutput::new(Vec::new(), 4, 10).unwrap();

        output.write_all(b"abcdef").unwrap();

        assert_eq!(
            output.copy_match(5, 3).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );

        output.copy_match(4, 4).unwrap();

        assert_eq!(output.into_inner(), b"abcdefcdef");
    }

    #[test]
    fn rejects_invalid_distances_lengths_and_limits_before_writing() {
        let mut output = DeflateOutput::new(Vec::new(), 256, 5).unwrap();
        output.write_all(b"ab").unwrap();

        assert_eq!(
            output.copy_match(0, 3).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );

        assert_eq!(
            output.copy_match(3, 3).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );

        assert_eq!(
            output.copy_match(1, 2).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );

        assert_eq!(
            output.copy_match(1, 4).unwrap_err().kind(),
            io::ErrorKind::Other
        );

        assert_eq!(
            output.write_all(b"abcd").unwrap_err().kind(),
            io::ErrorKind::Other
        );

        assert_eq!(output.bytes_written(), 2);
        assert_eq!(output.into_inner(), b"ab");
    }

    struct PartialWriter(Vec<u8>);

    impl Write for PartialWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let count = bytes.len().min(2);
            self.0.extend_from_slice(&bytes[..count]);

            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn records_only_bytes_written_by_partial_writer() {
        let mut output = DeflateOutput::new(PartialWriter(Vec::new()), 256, 7).unwrap();

        assert_eq!(output.write(b"abcd").unwrap(), 2);
        assert_eq!(output.bytes_written(), 2);

        output.copy_match(2, 5).unwrap();

        assert_eq!(output.into_inner().0, b"abababa");
    }

    #[test]
    fn rejects_invalid_window_sizes() {
        assert!(matches!(
            DeflateOutput::new(Vec::new(), 0, 10),
            Err(error) if error.kind() == io::ErrorKind::InvalidInput));

        assert!(matches!(
            DeflateOutput::new(Vec::new(), 32_769, 10),
            Err(error) if error.kind() == io::ErrorKind::InvalidInput
        ));
    }
}
