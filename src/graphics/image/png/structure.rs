//! Basic PNG chunk-order inspection, not complete image validation.
//!
//! PNG Third Edition, sections 5.6 and 11.2:
//! <https://www.w3.org/TR/png-3/>

use super::{ChunkReader, Ihdr, PngError, PngErrorKind, SIGNATURE};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PngStructure {
    pub header: Ihdr,
    pub idat_chunks: usize,
    pub idat_bytes: usize,
    pub trailing_bytes: usize,
    pub iend_crc_matches: bool,
}

/// Checks IHDR, IDAT ordering and IEND framing without decoding pixels.
///
/// Only IHDR's CRC is required here. The IEND CRC is reported; IDAT and
/// ancillary CRCs, palette rules, unknown critical chunks, compressed data,
/// and trailing bytes are not validated. Callers must apply their own policy.
pub fn inspect_structure(input: &[u8]) -> Result<PngStructure, PngError> {
    let mut reader = ChunkReader::new(input)?;

    let first = reader.next_chunk()?.ok_or(PngError {
        offset: SIGNATURE.len(),
        kind: PngErrorKind::MissingIhdr,
    })?;

    let header = Ihdr::from_chunk(&first)?;

    let mut idat_chunks = 0usize;
    let mut idat_bytes = 0usize;
    let mut idat_finished = false;

    while let Some(chunk) = reader.next_chunk()? {
        match &chunk.kind {
            b"IHDR" => {
                return Err(PngError {
                    offset: chunk.offset,
                    kind: PngErrorKind::DuplicateIhdr,
                });
            }

            b"IDAT" => {
                if idat_finished {
                    return Err(PngError {
                        offset: chunk.offset,
                        kind: PngErrorKind::NonConsecutiveIdat,
                    });
                }

                idat_chunks = idat_chunks.checked_add(1).ok_or(PngError {
                    offset: chunk.offset,
                    kind: PngErrorKind::OffsetOverflow,
                })?;

                idat_bytes = idat_bytes.checked_add(chunk.data.len()).ok_or(PngError {
                    offset: chunk.offset,
                    kind: PngErrorKind::OffsetOverflow,
                })?;
            }

            b"IEND" => {
                if !chunk.data.is_empty() {
                    return Err(PngError {
                        offset: chunk.offset,
                        kind: PngErrorKind::InvalidIendLength {
                            actual: chunk.data.len(),
                        },
                    });
                }

                if idat_chunks == 0 {
                    return Err(PngError {
                        offset: chunk.offset,
                        kind: PngErrorKind::MissingIdat,
                    });
                }

                return Ok(PngStructure {
                    header,
                    idat_chunks,
                    idat_bytes,
                    trailing_bytes: input.len() - reader.cursor,
                    iend_crc_matches: chunk.crc_matches(),
                });
            }

            _ => {
                if idat_chunks != 0 {
                    idat_finished = true;
                }
            }
        }
    }

    Err(PngError {
        offset: input.len(),
        kind: PngErrorKind::MissingIend,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::graphics::{
        image::encode_png,
        raster::{Color, Surface},
    };

    const IHDR: &[u8] = &[
        0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15,
        0xc4, 0x89,
    ];

    const IEND: &[u8] = &[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xae, 0x42, 0x60, 0x82];

    fn image(chunks: &[(&[u8; 4], &[u8])], end: Option<&[u8]>) -> Vec<u8> {
        let mut bytes = SIGNATURE.to_vec();
        bytes.extend_from_slice(IHDR);

        for (name, data) in chunks {
            bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
            bytes.extend_from_slice(*name);
            bytes.extend_from_slice(data);

            // CRCs are deliberately not checked by the structure inspector.
            bytes.extend_from_slice(&[0; 4]);
        }

        if let Some(end) = end {
            bytes.extend_from_slice(end);
        }

        bytes
    }

    #[test]
    fn inspects_our_encoder_output() {
        let surface = Surface::new(1, 1, Color::rgba(255, 0, 0, 255));
        let bytes = encode_png(&surface).unwrap();

        let result = inspect_structure(&bytes).unwrap();

        assert_eq!((result.header.width, result.header.height), (1, 1));
        assert!(result.idat_chunks > 0);
        assert!(result.idat_bytes > 0);
        assert_eq!(result.trailing_bytes, 0);
        assert!(result.iend_crc_matches);
    }

    #[test]
    fn handles_consecutive_and_empty_idat_and_trailing_data() {
        let mut bytes = image(
            &[
                (b"tEXt", b"note"),
                (b"IDAT", b"abc"),
                (b"IDAT", b""),
                (b"IDAT", b"de"),
                (b"tEXt", b"end"),
            ],
            Some(IEND),
        );

        bytes.extend_from_slice(b"tail");

        let result = inspect_structure(&bytes).unwrap();

        assert_eq!(
            (result.idat_chunks, result.idat_bytes, result.trailing_bytes),
            (3, 5, 4)
        );

        assert!(result.iend_crc_matches);
    }

    #[test]
    fn detects_missing_and_out_of_order_chunks() {
        let cases = [
            (image(&[], Some(IEND)), PngErrorKind::MissingIdat),
            (image(&[(b"IDAT", b"x")], None), PngErrorKind::MissingIend),
            (
                image(
                    &[(b"IDAT", b"x"), (b"tEXt", b""), (b"IDAT", b"y")],
                    Some(IEND),
                ),
                PngErrorKind::NonConsecutiveIdat,
            ),
            (
                image(&[(b"IHDR", b"")], Some(IEND)),
                PngErrorKind::DuplicateIhdr,
            ),
            (
                image(&[(b"IDAT", b"x"), (b"IEND", b"x")], None),
                PngErrorKind::InvalidIendLength { actual: 1 },
            ),
        ];

        for (bytes, expected) in cases {
            assert_eq!(inspect_structure(&bytes).unwrap_err().kind, expected);
        }
    }

    #[test]
    fn reports_bad_iend_crc_without_deciding_recovery() {
        let mut bytes = image(&[(b"IDAT", b"")], Some(IEND));
        *bytes.last_mut().unwrap() ^= 1;

        assert!(!inspect_structure(&bytes).unwrap().iend_crc_matches);
    }
}
