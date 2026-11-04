//! PNG zlib header parsing.
//!
//! RFC 1950, section 2.2:
//! <https://www.rfc-editor.org/rfc/rfc1950#section-2.2>
//!
//! PNG Third Edition, section 10.1:
//! <https://www.w3.org/TR/png-3/#10Compression>

use std::{
    error::Error,
    fmt,
    io::{self, Read},
};

mod decode;
mod encode;

pub use self::decode::{ZlibDecodeResult, decode_zlib};
pub use self::encode::{encode_zlib_fixed_literals, encode_zlib_fixed_lz77, encode_zlib_stored};

/// Metadata from a zlib header under PNG compression method 0.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZlibHeader {
    pub window_size: usize,
    pub compression_level_hint: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ZlibHeaderError {
    UnsupportedMethod(u8),
    InvalidWindowSize(u8),
    InvalidHeaderCheck,
    PresetDictionaryNotAllowed,
}

impl fmt::Display for ZlibHeaderError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "invalid PNG zlib header: {self:?}")
    }
}

impl Error for ZlibHeaderError {}

impl ZlibHeader {
    /// Parses the CMF and FLG bytes.
    ///
    /// This does not validate DEFLATE data or the Adler-32 checksum.
    pub fn parse(bytes: [u8; 2]) -> Result<Self, ZlibHeaderError> {
        let [cmf, flg] = bytes;

        let method = cmf & 0x0f;
        let window = cmf >> 4;

        if method != 8 {
            return Err(ZlibHeaderError::UnsupportedMethod(method));
        }

        if window > 7 {
            return Err(ZlibHeaderError::InvalidWindowSize(window));
        }

        if u16::from_be_bytes(bytes) % 31 != 0 {
            return Err(ZlibHeaderError::InvalidHeaderCheck);
        }

        if flg & 0x20 != 0 {
            return Err(ZlibHeaderError::PresetDictionaryNotAllowed);
        }

        Ok(Self {
            window_size: 1usize << (usize::from(window) + 8),
            compression_level_hint: flg >> 6,
        })
    }

    /// Reads and validates two bytes from a zlib stream.
    ///
    /// A truncated header returns `UnexpectedEof`; an invalid header returns
    /// `InvalidData` containing a `ZlibHeaderError`. If both bytes are read,
    /// they remain consumed even when validation fails.
    pub fn read_from(reader: &mut impl Read) -> io::Result<Self> {
        let mut bytes = [0; 2];
        reader.read_exact(&mut bytes)?;

        Self::parse(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::graphics::image::png_format::{IdatReader, SIGNATURE};

    #[test]
    fn parses_valid_headers_and_compression_hints() {
        assert_eq!(
            ZlibHeader::parse([0x78, 0x9c]),
            Ok(ZlibHeader {
                window_size: 32_768,
                compression_level_hint: 2,
            })
        );

        assert_eq!(
            ZlibHeader::parse([0x08, 0x1d]),
            Ok(ZlibHeader {
                window_size: 256,
                compression_level_hint: 0,
            })
        );
    }

    #[test]
    fn rejects_invalid_header_fields() {
        let cases = [
            ([0x77, 0x09], ZlibHeaderError::UnsupportedMethod(7)),
            ([0x88, 0x1c], ZlibHeaderError::InvalidWindowSize(8)),
            ([0x78, 0x9d], ZlibHeaderError::InvalidHeaderCheck),
            ([0x78, 0x20], ZlibHeaderError::PresetDictionaryNotAllowed),
        ];

        for (bytes, expected) in cases {
            assert_eq!(ZlibHeader::parse(bytes), Err(expected));
        }
    }

    #[test]
    fn distinguishes_truncated_and_invalid_headers() {
        let error = ZlibHeader::read_from(&mut &b"\x78"[..]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);

        let error = ZlibHeader::read_from(&mut &b"\x78\x9d"[..]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        assert_eq!(
            error.get_ref().unwrap().downcast_ref::<ZlibHeaderError>(),
            Some(&ZlibHeaderError::InvalidHeaderCheck)
        );
    }

    #[test]
    fn reads_header_split_between_idat_chunks() {
        let mut png = SIGNATURE.to_vec();

        // A valid 1x1 RGBA IHDR.
        png.extend_from_slice(&[
            0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15,
            0xc4, 0x89,
        ]);

        // Split the two-byte zlib header between two IDAT chunks.
        for data in [&[0x78][..], &[0x9c, 0x42][..]] {
            png.extend_from_slice(&(data.len() as u32).to_be_bytes());
            png.extend_from_slice(b"IDAT");
            png.extend_from_slice(data);

            // IDAT CRCs are not checked yet.
            png.extend_from_slice(&[0; 4]);
        }

        png.extend_from_slice(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xae, 0x42, 0x60, 0x82]);

        let mut reader = IdatReader::new(&png).unwrap();
        let header = ZlibHeader::read_from(&mut reader).unwrap();

        assert_eq!(header.window_size, 32_768);

        // The reader must remain positioned after the zlib header.
        let mut next = [0];
        reader.read_exact(&mut next).unwrap();

        assert_eq!(next, [0x42]);
    }
}
