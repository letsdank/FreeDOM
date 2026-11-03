//! Streaming access to the concatenated contents of PNG IDAT chunks.
//!
//! PNG Third Edition, sections 10.2 and 11.2.3:
//! <https://www.w3.org/TR/png-3/>

use std::io::{self, Read};

use super::{ChunkReader, PngError, PngStructure, inspect_structure};

/// Reads the compressed IDAT data as one continuous stream.
///
/// Construction checks basic chunk order with `inspect_structure`. It does
/// not check IDAT CRCs or decompress or validate the zlib stream. No copy of
/// the complete compressed stream is allocated.
pub struct IdatReader<'a> {
    reader: ChunkReader<'a>,
    structure: PngStructure,
    remaining: &'a [u8],
    started: bool,
    finished: bool,
}

impl<'a> IdatReader<'a> {
    pub fn new(input: &'a [u8]) -> Result<Self, PngError> {
        let structure = inspect_structure(input)?;
        let reader = ChunkReader::new(input)?;

        Ok(Self {
            reader,
            structure,
            remaining: &[],
            started: false,
            finished: false,
        })
    }

    pub fn structure(&self) -> &PngStructure {
        &self.structure
    }
}

impl Read for IdatReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() || self.finished {
            return Ok(0);
        }

        loop {
            if !self.remaining.is_empty() {
                let count = output.len().min(self.remaining.len());

                output[..count].copy_from_slice(&self.remaining[..count]);
                self.remaining = &self.remaining[count..];

                return Ok(count);
            }

            let chunk = self.reader.next_chunk().map_err(io::Error::other)?;

            match chunk {
                Some(chunk) if chunk.kind == *b"IDAT" => {
                    self.started = true;
                    self.remaining = chunk.data;
                }

                Some(_) if !self.started => {}

                _ => {
                    self.finished = true;
                    return Ok(0);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::graphics::image::png_format::{PngErrorKind, SIGNATURE};

    const IHDR: &[u8] = &[
        0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15,
        0xc4, 0x89,
    ];

    const IEND: &[u8] = &[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xae, 0x42, 0x60, 0x82];

    fn image(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut bytes = SIGNATURE.to_vec();
        bytes.extend_from_slice(IHDR);

        for (name, data) in chunks {
            bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
            bytes.extend_from_slice(*name);
            bytes.extend_from_slice(data);

            // IDAT and ancillary CRCs are not checked by IdatReader.
            bytes.extend_from_slice(&[0; 4]);
        }

        bytes.extend_from_slice(IEND);
        bytes
    }

    #[test]
    fn reads_across_chunk_boundaries_and_ignores_empty_chunks() {
        let bytes = image(&[
            (b"tEXt", b"before"),
            (b"IDAT", b"ab"),
            (b"IDAT", b""),
            (b"IDAT", b"cde"),
            (b"tEXt", b"after"),
        ]);

        let mut reader = IdatReader::new(&bytes).unwrap();

        assert_eq!(reader.structure().idat_chunks, 3);
        assert_eq!(reader.structure().idat_bytes, 5);

        let mut output = Vec::new();
        reader.read_to_end(&mut output).unwrap();

        assert_eq!(output, b"abcde");
        assert_eq!(reader.read(&mut [0; 1]).unwrap(), 0);
    }

    #[test]
    fn reads_in_small_pieces_without_advancing_on_empty_buffer() {
        let bytes = image(&[(b"IDAT", b"abc")]);
        let mut reader = IdatReader::new(&bytes).unwrap();

        assert_eq!(reader.read(&mut []).unwrap(), 0);

        let mut byte = [0];

        for expected in b"abc" {
            assert_eq!(reader.read(&mut byte).unwrap(), 1);
            assert_eq!(byte[0], *expected);
        }

        assert_eq!(reader.read(&mut byte).unwrap(), 0);
    }

    #[test]
    fn rejects_separated_idat_before_returning_reader() {
        let bytes = image(&[(b"IDAT", b"a"), (b"tEXt", b""), (b"IDAT", b"b")]);

        assert!(matches!(
            IdatReader::new(&bytes),
            Err(PngError {
                kind: PngErrorKind::NonConsecutiveIdat,
                ..
            })
        ));
    }

    #[test]
    fn empty_idat_stream_reaches_eof() {
        let bytes = image(&[(b"IDAT", b""), (b"IDAT", b"")]);

        let mut reader = IdatReader::new(&bytes).unwrap();

        assert_eq!(reader.read(&mut [0; 8]).unwrap(), 0);
        assert_eq!(reader.read(&mut [0; 8]).unwrap(), 0);
    }

    #[test]
    fn passes_a_split_zlib_stream_to_an_independent_decoder() {
        // A zlib stream containing "hello", split inside its header and trailer.
        let bytes = image(&[
            (b"IDAT", &[0x78]),
            (
                b"IDAT",
                &[0x9c, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00, 0x06],
            ),
            (b"IDAT", &[0x2c, 0x02, 0x15]),
        ]);

        let reader = IdatReader::new(&bytes).unwrap();
        let mut decoder = flate2::read::ZlibDecoder::new(reader);
        let mut decoded = Vec::new();

        decoder.read_to_end(&mut decoded).unwrap();

        assert_eq!(decoded, b"hello");
    }
}
