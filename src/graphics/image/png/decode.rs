//! PNG decoding into RGBA8 samples.
//!
//! PNG Third Edition, sections 9-11 and 13:
//! <https://www.w3.org/TR/png-3>

use std::{error::Error, fmt, io};

use super::{
    ChunkReader, ColorType, IdatReader, InterlaceMethod, PngError, PngFilterError, decode_zlib,
    unfilter_row,
};

mod adam7;
mod palette;

use self::palette::Palette;

#[derive(Clone, Copy, Debug)]
pub struct PngDecodeLimits {
    pub max_width: u32,
    pub max_height: u32,
    pub max_pixels: usize,
    pub max_filtered_bytes: usize,
}

impl Default for PngDecodeLimits {
    fn default() -> Self {
        Self {
            max_width: 4096,
            max_height: 4096,
            max_pixels: 16 * 1024 * 1024,
            max_filtered_bytes: 80 * 1024 * 1024,
        }
    }
}

/// Unassociated RGBA samples in file encoding; no color management is applied.
#[derive(Debug, Eq, PartialEq)]
pub struct Rgba8Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

#[derive(Debug)]
pub enum PngDecodeError {
    Container(PngError),
    Inflate(io::Error),
    Filter {
        row: usize,
        source: PngFilterError,
    },
    Adam7Filter {
        pass: usize,
        row: usize,
        source: PngFilterError,
    },
    Adam7OutputLength {
        expected: usize,
        actual: usize,
    },
    UnknownCriticalChunk([u8; 4]),
    InvalidTransparencyChunk,
    InvalidPalette,
    CriticalCrcMismatch {
        kind: [u8; 4],
        offset: usize,
    },
    SizeOverflow,
    LimitExceeded,
    AllocationFailed,
    ScanlineLength {
        expected: usize,
        actual: usize,
    },
    TransparencyCrcMismatch {
        offset: usize,
    },
    MissingPalette,
}

impl fmt::Display for PngDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PNG decode error: {self:?}")
    }
}

impl Error for PngDecodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Container(error) => Some(error),
            Self::Inflate(error) => Some(error),
            Self::Filter { source, .. } => Some(source),
            Self::Adam7Filter { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<PngError> for PngDecodeError {
    fn from(error: PngError) -> Self {
        Self::Container(error)
    }
}

/// Decodes the default image, checking critical chunk CRCs.
///
/// Supports both non-interlaced and Adam7 images for all standard PNG
/// color types and bit depths. Supported tRNS chunks use strict CRC
/// checks. Other ancillary chunks and trailing data are ignored.
///
/// Color profiles, gamma, HDR and animation are not interpreted. Limits
/// bound output sizes, not input size or CPU time.
pub fn decode_rgba8(input: &[u8], limits: PngDecodeLimits) -> Result<Rgba8Image, PngDecodeError> {
    let idat = IdatReader::new(input)?;
    let header = idat.structure().header;

    if header.width > limits.max_width || header.height > limits.max_height {
        return Err(PngDecodeError::LimitExceeded);
    }

    let width = usize::try_from(header.width).map_err(|_| PngDecodeError::SizeOverflow)?;
    let height = usize::try_from(header.height).map_err(|_| PngDecodeError::SizeOverflow)?;

    let pixels = width
        .checked_mul(height)
        .ok_or(PngDecodeError::SizeOverflow)?;

    let channels = match header.color_type {
        ColorType::Grayscale => 1,
        ColorType::GrayscaleAlpha => 2,
        ColorType::Truecolor => 3,
        ColorType::TruecolorAlpha => 4,
        ColorType::Indexed => 1,
    };

    let bits_per_pixel = channels * usize::from(header.bit_depth);

    let row_bytes = width
        .checked_mul(bits_per_pixel)
        .and_then(|bits| bits.checked_add(7))
        .ok_or(PngDecodeError::SizeOverflow)?
        / 8;

    let filter_bytes_per_pixel = bits_per_pixel.div_ceil(8);

    let rgba_bytes = pixels.checked_mul(4).ok_or(PngDecodeError::SizeOverflow)?;

    let stride = row_bytes
        .checked_add(1)
        .ok_or(PngDecodeError::SizeOverflow)?;

    let filtered_len = match header.interlace {
        InterlaceMethod::None => stride
            .checked_mul(height)
            .ok_or(PngDecodeError::SizeOverflow)?,

        InterlaceMethod::Adam7 => adam7::plan(width, height, bits_per_pixel)?.filtered_len,
    };

    if pixels > limits.max_pixels || filtered_len > limits.max_filtered_bytes {
        return Err(PngDecodeError::LimitExceeded);
    }

    let mut reader = ChunkReader::new(input)?;

    #[derive(Clone, Copy, Eq, PartialEq)]
    enum TransparentKey {
        Gray(u16),
        Rgb([u16; 3]),
    }

    let mut seen_idat = false;
    let mut seen_palette = false;
    let mut transparent = None;
    let mut palette: Option<Palette<'_>> = None;

    while let Some(chunk) = reader.next_chunk()? {
        if chunk.is_critical() && !chunk.crc_matches() {
            return Err(PngDecodeError::CriticalCrcMismatch {
                kind: chunk.kind,
                offset: chunk.offset,
            });
        }

        match &chunk.kind {
            b"IHDR" => {}

            b"IDAT" => {
                if header.color_type == ColorType::Indexed && palette.is_none() {
                    return Err(PngDecodeError::MissingPalette);
                }

                seen_idat = true;
            }

            b"IEND" => break,

            b"PLTE" => {
                if seen_palette
                    || seen_idat
                    || transparent.is_some()
                    || matches!(
                        header.color_type,
                        ColorType::Grayscale | ColorType::GrayscaleAlpha
                    )
                {
                    return Err(PngDecodeError::InvalidPalette);
                }

                if header.color_type == ColorType::Indexed
                    && chunk.data.len() > (1usize << header.bit_depth) * 3
                {
                    return Err(PngDecodeError::InvalidPalette);
                }

                palette = Some(Palette::new(chunk.data)?);
                seen_palette = true;
            }

            b"tRNS" => {
                if header.color_type == ColorType::Indexed {
                    if seen_idat {
                        return Err(PngDecodeError::InvalidTransparencyChunk);
                    }

                    let indexed_palette = palette
                        .as_mut()
                        .ok_or(PngDecodeError::InvalidTransparencyChunk)?;

                    if !chunk.crc_matches() {
                        return Err(PngDecodeError::TransparencyCrcMismatch {
                            offset: chunk.offset,
                        });
                    }

                    indexed_palette.set_alpha(chunk.data)?;
                    continue;
                }

                let expected_len = match header.color_type {
                    ColorType::Grayscale => 2,
                    ColorType::Truecolor => 6,

                    // Images with an alpha channel must not contain tRNS.
                    _ => return Err(PngDecodeError::InvalidTransparencyChunk),
                };

                if seen_idat || transparent.is_some() || chunk.data.len() != expected_len {
                    return Err(PngDecodeError::InvalidTransparencyChunk);
                }

                if !chunk.crc_matches() {
                    return Err(PngDecodeError::TransparencyCrcMismatch {
                        offset: chunk.offset,
                    });
                }

                // PNG uses the least significant bits when sample depth is below 16.
                let mask = if header.bit_depth == 16 {
                    u16::MAX
                } else {
                    (1u16 << header.bit_depth) - 1
                };

                transparent = Some(match header.color_type {
                    ColorType::Grayscale => {
                        let value = u16::from_be_bytes([chunk.data[0], chunk.data[1]]);

                        TransparentKey::Gray(value & mask)
                    }

                    ColorType::Truecolor => TransparentKey::Rgb([
                        u16::from_be_bytes([chunk.data[0], chunk.data[1]]) & mask,
                        u16::from_be_bytes([chunk.data[2], chunk.data[3]]) & mask,
                        u16::from_be_bytes([chunk.data[4], chunk.data[5]]) & mask,
                    ]),

                    _ => unreachable!(),
                });
            }

            _ if chunk.is_critical() => {
                return Err(PngDecodeError::UnknownCriticalChunk(chunk.kind));
            }

            _ => {}
        }
    }

    let mut buffer = Vec::new();

    buffer
        .try_reserve_exact(filtered_len.max(rgba_bytes))
        .map_err(|_| PngDecodeError::AllocationFailed)?;

    let mut filtered = decode_zlib(idat, buffer, filtered_len)
        .map_err(PngDecodeError::Inflate)?
        .output;

    if filtered.len() != filtered_len {
        return Err(PngDecodeError::ScanlineLength {
            expected: filtered_len,
            actual: filtered.len(),
        });
    }

    if header.interlace == InterlaceMethod::Adam7 {
        let raw_len = row_bytes
            .checked_mul(height)
            .ok_or(PngDecodeError::SizeOverflow)?;

        let mut output = Vec::new();

        output
            .try_reserve_exact(raw_len.max(rgba_bytes))
            .map_err(|_| PngDecodeError::AllocationFailed)?;

        output.resize(raw_len, 0);

        adam7::reconstruct::reconstruct(width, height, bits_per_pixel, &mut filtered, &mut output)?;

        filtered = output;
    } else {
        // Compact each reconstructed row over its filter byte in the same buffer.
        for y in 0..height {
            let source = y * stride;
            let destination = y * row_bytes;
            let filter = filtered[source];

            filtered.copy_within(source + 1..source + stride, destination);

            let (before, after) = filtered.split_at_mut(destination);

            let previous = if y == 0 {
                None
            } else {
                Some(&before[destination - row_bytes..])
            };

            unfilter_row(
                filter,
                &mut after[..row_bytes],
                previous,
                filter_bytes_per_pixel,
            )
            .map_err(|source| PngDecodeError::Filter { row: y, source })?;
        }

        filtered.truncate(row_bytes * height);
    }

    if header.bit_depth == 16 {
        let bytes_per_pixel = channels * 2;

        if rgba_bytes > filtered.len() {
            filtered.resize(rgba_bytes, 0);
        }

        for step in 0..pixels {
            // Expand grayscale backwards; process equal-sized or shrinking
            // representations forwards to avoid overwriting unread pixels.
            let i = if bytes_per_pixel < 4 {
                pixels - 1 - step
            } else {
                step
            };

            let source = i * bytes_per_pixel;
            let mut samples = [0u16; 4];

            for c in 0..channels {
                samples[c] =
                    u16::from_be_bytes([filtered[source + c * 2], filtered[source + c * 2 + 1]]);
            }

            let (rgb, alpha) = match header.color_type {
                ColorType::Grayscale => {
                    let alpha = if transparent == Some(TransparentKey::Gray(samples[0])) {
                        0
                    } else {
                        u16::MAX
                    };

                    ([samples[0]; 3], alpha)
                }

                ColorType::GrayscaleAlpha => ([samples[0]; 3], samples[1]),

                ColorType::Truecolor => {
                    let rgb = [samples[0], samples[1], samples[2]];

                    let alpha = if transparent == Some(TransparentKey::Rgb(rgb)) {
                        0
                    } else {
                        u16::MAX
                    };

                    (rgb, alpha)
                }

                ColorType::TruecolorAlpha => ([samples[0], samples[1], samples[2]], samples[3]),

                ColorType::Indexed => unreachable!(),
            };

            let destination = i * 4;

            filtered[destination..destination + 4].copy_from_slice(&[
                (rgb[0] >> 8) as u8,
                (rgb[1] >> 8) as u8,
                (rgb[2] >> 8) as u8,
                (alpha >> 8) as u8,
            ]);
        }

        filtered.truncate(rgba_bytes);
    } else if channels != 4 {
        filtered.resize(rgba_bytes, 0);

        // Expand backwards so that source samples are not overwritten.
        for i in (0..pixels).rev() {
            let source = i * channels;

            let sample = if header.bit_depth < 8 {
                let x = i % width;
                let y = i / width;

                let bit = x * usize::from(header.bit_depth);
                let byte = filtered[y * row_bytes + bit / 8];
                let shift = 8 - usize::from(header.bit_depth) - bit % 8;

                (byte >> shift) & ((1u8 << header.bit_depth) - 1)
            } else {
                filtered[source]
            };

            if header.color_type == ColorType::Indexed {
                let index = sample;

                let color = palette
                    .as_ref()
                    .ok_or(PngDecodeError::MissingPalette)?
                    .rgba(index);

                let destination = i * 4;

                filtered[destination..destination + 4].clone_from_slice(&color);

                continue;
            }

            let (rgb, alpha) = match channels {
                1 => {
                    let gray = sample;

                    let alpha = if transparent == Some(TransparentKey::Gray(u16::from(gray))) {
                        0
                    } else {
                        255
                    };

                    let max_sample = (1u16 << header.bit_depth) - 1;
                    let gray8 = (u16::from(gray) * 255 / max_sample) as u8;

                    ([gray8; 3], alpha)
                }

                2 => {
                    let gray = filtered[source];
                    let alpha = filtered[source + 1];

                    ([gray; 3], alpha)
                }

                3 => {
                    let rgb = [filtered[source], filtered[source + 1], filtered[source + 2]];

                    let alpha = if transparent == Some(TransparentKey::Rgb(rgb.map(u16::from))) {
                        0
                    } else {
                        255
                    };

                    (rgb, alpha)
                }

                _ => unreachable!(),
            };

            let destination = i * 4;

            filtered[destination..destination + 3].copy_from_slice(&rgb);
            filtered[destination + 3] = alpha;
        }
    }

    Ok(Rgba8Image {
        width: header.width,
        height: header.height,
        pixels: filtered,
    })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "decode/packed_tests.rs"]
mod packed_tests;

#[cfg(test)]
#[path = "decode/sixteen_tests.rs"]
mod sixteen_tests;
