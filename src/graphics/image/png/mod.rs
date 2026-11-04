//! Low-level PNG chunk reading.
//!
//! PNG Third Edition, sections 5.2-5.5:
//! <https://www.w3.org/TR/png-3/>
//!
//! Chunk framing and IHDR metadata parsing; this is not a complete PNG parser.
//! Later chunks, decompression and pixel data are not validated. Chunk CRC
//! results are exposed to the caller so higher layers can choose error recovery.

use std::{error::Error, fmt};

mod adler32;
mod decode;
mod deflate;
mod encode;
mod filter;
mod idat;
mod ihdr;
mod structure;
mod zlib;

pub use self::adler32::{Adler32, Adler32Writer, verify_zlib_adler32};
pub use self::decode::{
    PngDecodeError, PngDecodeLimits, Rgba8Image, Rgba16BeImage, decode_rgba8, decode_rgba16_be,
};
pub use self::deflate::{
    DeflateBitReader, DeflateBitWriter, DeflateBlockHeader, DeflateBlockType, DeflateOutput,
    HuffmanError, HuffmanTree, Lz77Finder, Lz77Token, copy_stored_block,
    decode_dynamic_huffman_block, decode_fixed_huffman_block, encode_fixed_literals,
    encode_fixed_lz77, read_block_header,
};
pub use self::encode::{PngEncodeError, encode_rgba8, encode_rgba16_be};
pub use self::filter::{PngFilterError, filter_row, unfilter_row};
pub use self::idat::IdatReader;
pub use self::ihdr::{ColorType, Ihdr, InterlaceMethod, read_ihdr};
pub use self::structure::{PngStructure, inspect_structure};
pub use self::zlib::{
    ZlibDecodeResult, ZlibHeader, ZlibHeaderError, decode_zlib, encode_zlib_fixed_literals,
    encode_zlib_fixed_lz77, encode_zlib_stored,
};

const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const MAX_CHUNK_LENGTH: u32 = 0x7fff_ffff;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PngError {
    pub offset: usize,
    pub kind: PngErrorKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PngErrorKind {
    TruncatedSignature,
    InvalidSignature,
    TruncatedChunk,
    ChunkTooLarge,
    InvalidChunkType,
    OffsetOverflow,
    MissingIhdr,
    ExpectedIhdr { found: [u8; 4] },
    InvalidIhdrLength { actual: usize },
    InvalidIhdrCrc { stored: u32, computed: u32 },
    InvalidDimensions { width: u32, height: u32 },
    InvalidColorType(u8),
    InvalidBitDepth { color_type: u8, bit_depth: u8 },
    InvalidCompressionMethod(u8),
    InvalidFilterMethod(u8),
    InvalidInterlaceMethod(u8),
    DuplicateIhdr,
    MissingIdat,
    NonConsecutiveIdat,
    MissingIend,
    InvalidIendLength { actual: usize },
}

impl fmt::Display for PngError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PNG error at byte {}: {:?}", self.offset, self.kind)
    }
}

impl Error for PngError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Chunk<'a> {
    pub offset: usize,
    pub kind: [u8; 4],
    pub data: &'a [u8],
    pub stored_crc: u32,
}

impl Chunk<'_> {
    /// Whether the first chunk-type byte marks this chunk as critical.
    pub fn is_critical(&self) -> bool {
        self.kind[0] & 0x20 == 0
    }

    /// Computes the PNG CRC over the four type bytes and the chunk data.
    pub fn computed_crc(&self) -> u32 {
        let crc = update_crc(0xffff_ffff, &self.kind);

        update_crc(crc, self.data) ^ 0xffff_ffff
    }

    pub fn crc_matches(&self) -> bool {
        self.computed_crc() == self.stored_crc
    }
}

/// Sequential reader over a complete in-memory byte slice.
///
/// End of input is not proof of a valid PNG: callers must separately require
/// IHDR, IDAT, IEND and all other structural constraints.
pub struct ChunkReader<'a> {
    input: &'a [u8],
    cursor: usize,
}

impl<'a> ChunkReader<'a> {
    pub fn new(input: &'a [u8]) -> Result<Self, PngError> {
        let signature = input.get(..SIGNATURE.len()).ok_or(PngError {
            offset: 0,
            kind: PngErrorKind::TruncatedSignature,
        })?;

        if signature != SIGNATURE {
            return Err(PngError {
                offset: 0,
                kind: PngErrorKind::InvalidSignature,
            });
        }

        Ok(Self {
            input,
            cursor: SIGNATURE.len(),
        })
    }

    /// Reads the next chunk without copying its payload.
    ///
    /// The cursor remains unchanged when an error is returned. IEND does not
    /// terminate this low-level reader; the caller decides how to handle it.
    pub fn next_chunk(&mut self) -> Result<Option<Chunk<'a>>, PngError> {
        let offset = self.cursor;
        let remaining = &self.input[offset..];

        if remaining.is_empty() {
            return Ok(None);
        }

        let header = remaining.get(..8).ok_or(PngError {
            offset,
            kind: PngErrorKind::TruncatedChunk,
        })?;

        let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);

        if length > MAX_CHUNK_LENGTH {
            return Err(PngError {
                offset,
                kind: PngErrorKind::ChunkTooLarge,
            });
        }

        let kind = [header[4], header[5], header[6], header[7]];

        // Unknown ancillary chunks can be ignored even when their type bytes
        // are not valid ASCII letters. Critical types remain strictly checked.
        // PNG Third Edition, section 13.3:
        // https://www.w3.org/TR/png-3/#13Decoders.Errors
        if kind[0] & 0x20 == 0 && !kind.iter().all(u8::is_ascii_alphabetic) {
            return Err(PngError {
                offset: offset + 4,
                kind: PngErrorKind::InvalidChunkType,
            });
        }

        let length = usize::try_from(length).map_err(|_| PngError {
            offset,
            kind: PngErrorKind::OffsetOverflow,
        })?;

        let data_start = offset.checked_add(8).ok_or(PngError {
            offset,
            kind: PngErrorKind::OffsetOverflow,
        })?;

        let data_end = data_start.checked_add(length).ok_or(PngError {
            offset,
            kind: PngErrorKind::OffsetOverflow,
        })?;

        let chunk_end = data_end.checked_add(4).ok_or(PngError {
            offset,
            kind: PngErrorKind::OffsetOverflow,
        })?;

        let data = self.input.get(data_start..data_end).ok_or(PngError {
            offset,
            kind: PngErrorKind::TruncatedChunk,
        })?;

        let crc = self.input.get(data_end..chunk_end).ok_or(PngError {
            offset,
            kind: PngErrorKind::TruncatedChunk,
        })?;

        let stored_crc = u32::from_be_bytes([crc[0], crc[1], crc[2], crc[3]]);

        self.cursor = chunk_end;

        Ok(Some(Chunk {
            offset,
            kind,
            data,
            stored_crc,
        }))
    }
}

// PNG uses the reflected CRC-32 polynomial 0xedb88320.
// See PNG Third Edition, section 5.5:
// <https://www.w3.org/TR/png-3/>
fn update_crc(mut crc: u32, bytes: &[u8]) -> u32 {
    for &byte in bytes {
        crc ^= u32::from(byte);

        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }

    crc
}

#[cfg(test)]
mod tests {
    use super::{ChunkReader, PngErrorKind, SIGNATURE, update_crc};

    use crate::graphics::{
        image::encode_png,
        raster::{Color, Surface},
    };

    #[test]
    fn crc32_matches_a_known_independent_check_value() {
        assert_eq!(
            update_crc(0xffff_ffff, b"123456789") ^ 0xffff_ffff,
            0xcbf4_3926
        );
    }

    #[test]
    fn reads_fixed_iend_chunk_and_reports_crc_corruption() {
        let bytes: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\0IEND\xaeB`\x82";

        let mut reader = ChunkReader::new(bytes).unwrap();
        let chunk = reader.next_chunk().unwrap().unwrap();

        assert_eq!(chunk.offset, 8);
        assert_eq!(chunk.kind, *b"IEND");
        assert!(chunk.data.is_empty());
        assert!(chunk.is_critical());
        assert!(chunk.crc_matches());

        assert!(reader.next_chunk().unwrap().is_none());

        let mut damaged = bytes.to_vec();
        *damaged.last_mut().unwrap() ^= 1;

        let mut reader = ChunkReader::new(&damaged).unwrap();
        let chunk = reader.next_chunk().unwrap().unwrap();

        assert!(!chunk.crc_matches());
    }

    #[test]
    fn reads_chunks_encoded_by_existing_png_implementation() {
        let surface = Surface::new(2, 1, Color::rgba(12, 34, 56, 78));

        let bytes = encode_png(&surface).unwrap();
        let mut reader = ChunkReader::new(&bytes).unwrap();

        let mut kinds = Vec::new();

        while let Some(chunk) = reader.next_chunk().unwrap() {
            assert!(chunk.crc_matches(), "bad CRC for {:?}", chunk.kind);

            kinds.push(chunk.kind);
        }

        assert_eq!(kinds.first(), Some(b"IHDR"));
        assert!(kinds.contains(b"IDAT"));
        assert_eq!(kinds.last(), Some(b"IEND"));
    }

    #[test]
    fn rejects_invalid_or_truncated_signature() {
        assert!(matches!(
            ChunkReader::new(b"PNG"),
            Err(error)
                if error.kind == PngErrorKind::TruncatedSignature
        ));

        assert!(matches!(
            ChunkReader::new(b"notapng!"),
            Err(error)
                if error.kind == PngErrorKind::InvalidSignature
        ));
    }

    #[test]
    fn rejects_invalid_chunk_framing_without_advancing() {
        let mut bytes = SIGNATURE.to_vec();

        bytes.extend_from_slice(b"\0\0\0\x02IDAT\x01");

        let mut reader = ChunkReader::new(&bytes).unwrap();

        for _ in 0..2 {
            let error = reader.next_chunk().unwrap_err();

            assert_eq!(error.offset, 8);
            assert_eq!(error.kind, PngErrorKind::TruncatedChunk);
        }

        let mut invalid_type = SIGNATURE.to_vec();

        invalid_type.extend_from_slice(b"\0\0\0\0I1ND\0\0\0\0");

        let error = ChunkReader::new(&invalid_type)
            .unwrap()
            .next_chunk()
            .unwrap_err();

        assert_eq!(error.offset, 12);
        assert_eq!(error.kind, PngErrorKind::InvalidChunkType);

        let mut huge = SIGNATURE.to_vec();

        huge.extend_from_slice(b"\x80\0\0\0IDAT");

        let error = ChunkReader::new(&huge).unwrap().next_chunk().unwrap_err();

        assert_eq!(error.kind, PngErrorKind::ChunkTooLarge);
    }

    #[test]
    fn accepts_unknown_ancillary_chunk_types_even_if_not_alphabetic() {
        for kind in [[b's', 1, b'I', b'T'], [0xa1, b'R', 14, b'S']] {
            let mut bytes = SIGNATURE.to_vec();

            bytes.extend_from_slice(&0u32.to_be_bytes());
            bytes.extend_from_slice(&kind);
            bytes.extend_from_slice(&0u32.to_be_bytes());

            let mut reader = ChunkReader::new(&bytes).unwrap();
            let chunk = reader.next_chunk().unwrap().unwrap();

            assert_eq!(chunk.kind, kind);
            assert!(!chunk.is_critical());
            assert!(reader.next_chunk().unwrap().is_none());
        }
    }

    #[test]
    fn malformed_ancillary_type_does_not_bypass_framing_checks() {
        let mut bytes = SIGNATURE.to_vec();

        bytes.extend_from_slice(&3u32.to_be_bytes());
        bytes.extend_from_slice(&[b's', 1, b'I', b'T']);
        bytes.extend_from_slice(&[1, 2]);

        let mut reader = ChunkReader::new(&bytes).unwrap();

        for _ in 0..2 {
            let error = reader.next_chunk().unwrap_err();

            assert_eq!(error.offset, 8);
            assert_eq!(error.kind, PngErrorKind::TruncatedChunk);
        }
    }
}
