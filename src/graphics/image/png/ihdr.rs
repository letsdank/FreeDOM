//! PNG IHDR parsing and validation.
//!
//! PNG Third Edition, section 11.2.1:
//! <https://www.w3.org/TR/png-3>
//!
//! This reads metadata only; image data, palette, and chunk order after IHDR
//! are not validated here.

use super::{Chunk, ChunkReader, PngError, PngErrorKind};

const IHDR_LENGTH: usize = 13;
const MAX_DIMENSION: u32 = 0x7fff_ffff;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorType {
    Grayscale,
    Truecolor,
    Indexed,
    GrayscaleAlpha,
    TruecolorAlpha,
}

impl ColorType {
    /// Grayscale, truecolor and indexed PNGs can still use transparency through tRNS.
    pub fn has_alpha_channel(self) -> bool {
        matches!(self, Self::GrayscaleAlpha | Self::TruecolorAlpha)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterlaceMethod {
    None,
    Adam7,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ihdr {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub color_type: ColorType,
    pub interlace: InterlaceMethod,
}

impl Ihdr {
    /// Parses an IHDR chunk, including its required CRC check.
    /// This does not check where the chunk occurs in the stream.
    pub fn from_chunk(chunk: &Chunk<'_>) -> Result<Self, PngError> {
        let error = |relative: usize, kind| PngError {
            offset: chunk.offset.saturating_add(relative),
            kind,
        };

        if chunk.kind != *b"IHDR" {
            return Err(error(4, PngErrorKind::ExpectedIhdr { found: chunk.kind }));
        }

        if chunk.data.len() != IHDR_LENGTH {
            return Err(error(
                0,
                PngErrorKind::InvalidIhdrLength {
                    actual: chunk.data.len(),
                },
            ));
        }

        let computed = chunk.computed_crc();

        if computed != chunk.stored_crc {
            return Err(error(
                8 + IHDR_LENGTH,
                PngErrorKind::InvalidIhdrCrc {
                    stored: chunk.stored_crc,
                    computed,
                },
            ));
        }

        let bytes = chunk.data;
        let width = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let height = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);

        if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
            let relative = if width == 0 || width > MAX_DIMENSION {
                8
            } else {
                12
            };

            return Err(error(
                relative,
                PngErrorKind::InvalidDimensions { width, height },
            ));
        }

        let bit_depth = bytes[8];
        let color_byte = bytes[9];

        let color_type = match color_byte {
            0 => ColorType::Grayscale,
            2 => ColorType::Truecolor,
            3 => ColorType::Indexed,
            4 => ColorType::GrayscaleAlpha,
            6 => ColorType::TruecolorAlpha,

            _ => {
                return Err(error(17, PngErrorKind::InvalidColorType(color_byte)));
            }
        };

        let allowed = match color_type {
            ColorType::Grayscale => {
                matches!(bit_depth, 1 | 2 | 4 | 8 | 16)
            }

            ColorType::Indexed => {
                matches!(bit_depth, 1 | 2 | 4 | 8)
            }

            ColorType::Truecolor | ColorType::GrayscaleAlpha | ColorType::TruecolorAlpha => {
                matches!(bit_depth, 8 | 16)
            }
        };

        if !allowed {
            return Err(error(
                16,
                PngErrorKind::InvalidBitDepth {
                    color_type: color_byte,
                    bit_depth,
                },
            ));
        }

        if bytes[10] != 0 {
            return Err(error(18, PngErrorKind::InvalidCompressionMethod(bytes[10])));
        }

        if bytes[11] != 0 {
            return Err(error(19, PngErrorKind::InvalidFilterMethod(bytes[11])));
        }

        let interlace = match bytes[12] {
            0 => InterlaceMethod::None,
            1 => InterlaceMethod::Adam7,

            other => {
                return Err(error(20, PngErrorKind::InvalidInterlaceMethod(other)));
            }
        };

        Ok({
            Self {
                width,
                height,
                bit_depth,
                color_type,
                interlace,
            }
        })
    }
}

/// Reads only the first PNG chunk and validates it as IHDR.
/// A successful result does not imply that the complete PNG is valid.
pub fn read_ihdr(input: &[u8]) -> Result<Ihdr, PngError> {
    let mut reader = ChunkReader::new(input)?;

    let chunk = reader.next_chunk()?.ok_or(PngError {
        offset: 8,
        kind: PngErrorKind::MissingIhdr,
    })?;

    Ihdr::from_chunk(&chunk)
}

#[cfg(test)]
mod tests {
    use super::{ColorType, Ihdr, InterlaceMethod, read_ihdr};

    use crate::graphics::{
        image::{
            encode_png,
            png_format::{Chunk, PngErrorKind},
        },
        raster::{Color, Surface},
    };

    const FIXED_IHDR: &[u8] = b"\x89PNG\r\n\x1a\n\
      \0\0\0\x0d\
      IHDR\
      \0\0\0\x01\
      \0\0\0\x01\
      \x08\x06\0\0\0\
      \x1f\x15\xc4\x89";

    fn chunk(data: &[u8]) -> Chunk<'_> {
        let mut chunk = Chunk {
            offset: 8,
            kind: *b"IHDR",
            data,
            stored_crc: 0,
        };

        chunk.stored_crc = chunk.computed_crc();

        chunk
    }

    #[test]
    fn parses_fixed_independent_ihdr_and_existing_encoder_output() {
        let header = read_ihdr(FIXED_IHDR).unwrap();

        assert_eq!(
            header,
            Ihdr {
                width: 1,
                height: 1,
                bit_depth: 8,
                color_type: ColorType::TruecolorAlpha,
                interlace: InterlaceMethod::None,
            }
        );

        assert!(header.color_type.has_alpha_channel());

        let bytes = encode_png(&Surface::new(2, 3, Color::TRANSPARENT)).unwrap();
        let header = read_ihdr(&bytes).unwrap();

        assert_eq!(
            (
                header.width,
                header.height,
                header.bit_depth,
                header.color_type,
            ),
            (2, 3, 8, ColorType::TruecolorAlpha)
        );
    }

    #[test]
    fn validates_all_bit_depth_and_color_type_combinations() {
        for color in [0_u8, 2, 3, 4, 6] {
            for depth in [0_u8, 1, 2, 4, 8, 16, 32] {
                let mut data = [0_u8; 13];

                data[3] = 1;
                data[7] = 1;
                data[8] = depth;
                data[9] = color;

                let allowed = match color {
                    0 => matches!(depth, 1 | 2 | 4 | 8 | 16),
                    3 => matches!(depth, 1 | 2 | 4 | 8),
                    _ => matches!(depth, 8 | 16),
                };

                let result = Ihdr::from_chunk(&chunk(&data));

                if allowed {
                    assert!(result.is_ok(), "color={color} depth={depth}");
                } else {
                    assert!(
                        matches!(
                            result,
                            Err(error)
                                if error.kind == PngErrorKind::InvalidBitDepth {
                                    color_type: color,
                                    bit_depth: depth,
                                }
                        ),
                        "color={color} depth={depth}"
                    );
                }
            }
        }
    }

    #[test]
    fn rejects_missing_wrong_type_length_and_crc() {
        assert_eq!(
            read_ihdr(b"\x89PNG\r\n\x1a\n").unwrap_err().kind,
            PngErrorKind::MissingIhdr
        );

        let data = [0_u8; 13];
        let mut wrong = chunk(&data);

        wrong.kind = *b"IDAT";

        assert_eq!(
            Ihdr::from_chunk(&wrong).unwrap_err().kind,
            PngErrorKind::ExpectedIhdr { found: *b"IDAT" }
        );

        assert_eq!(
            Ihdr::from_chunk(&chunk(&data[..12])).unwrap_err().kind,
            PngErrorKind::InvalidIhdrLength { actual: 12 }
        );

        let mut broken = FIXED_IHDR.to_vec();
        *broken.last_mut().unwrap() ^= 1;

        assert!(matches!(
            read_ihdr(&broken).unwrap_err().kind,
            PngErrorKind::InvalidIhdrCrc { .. }
        ));
    }

    #[test]
    fn validates_dimensions_and_accepts_format_limit_without_allocating() {
        let mut data = [0_u8; 13];

        data[3] = 1;
        data[7] = 1;
        data[8] = 8;
        data[9] = 6;

        data[..4].copy_from_slice(&0_u32.to_be_bytes());

        assert_eq!(
            Ihdr::from_chunk(&chunk(&data)).unwrap_err().kind,
            PngErrorKind::InvalidDimensions {
                width: 0,
                height: 1,
            }
        );

        data[..4].copy_from_slice(&1_u32.to_be_bytes());
        data[4..8].copy_from_slice(&0x8000_0000_u32.to_be_bytes());

        assert_eq!(
            Ihdr::from_chunk(&chunk(&data)).unwrap_err().kind,
            PngErrorKind::InvalidDimensions {
                width: 1,
                height: 0x8000_0000
            }
        );

        data[..4].copy_from_slice(&0x7fff_ffff_u32.to_be_bytes());
        data[4..8].copy_from_slice(&0x7fff_ffff_u32.to_be_bytes());

        let header = Ihdr::from_chunk(&chunk(&data)).unwrap();

        assert_eq!((header.width, header.height), (0x7fff_ffff, 0x7fff_ffff));
    }

    #[test]
    fn validates_other_header_fields() {
        let mut data = [0_u8; 13];

        data[3] = 1;
        data[7] = 1;
        data[8] = 8;
        data[9] = 6;

        data[9] = 5;

        assert_eq!(
            Ihdr::from_chunk(&chunk(&data)).unwrap_err().kind,
            PngErrorKind::InvalidColorType(5)
        );

        data[9] = 6;
        data[10] = 1;

        assert_eq!(
            Ihdr::from_chunk(&chunk(&data)).unwrap_err().kind,
            PngErrorKind::InvalidCompressionMethod(1)
        );

        data[10] = 0;
        data[11] = 1;

        assert_eq!(
            Ihdr::from_chunk(&chunk(&data)).unwrap_err().kind,
            PngErrorKind::InvalidFilterMethod(1)
        );

        data[11] = 0;
        data[12] = 2;

        assert_eq!(
            Ihdr::from_chunk(&chunk(&data)).unwrap_err().kind,
            PngErrorKind::InvalidInterlaceMethod(2)
        );

        data[12] = 1;

        assert_eq!(
            Ihdr::from_chunk(&chunk(&data)).unwrap().interlace,
            InterlaceMethod::Adam7
        );

        assert!(!ColorType::Indexed.has_alpha_channel());
    }
}
