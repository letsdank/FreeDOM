//! Minimal non-interlaced 8-bit RGBA PNG decoding.
//!
//! PNG Third Edition, sections 9-11 and 13:
//! <https://www.w3.org/TR/png-3>

use std::{error::Error, fmt, io};

use super::{
    ChunkReader, ColorType, IdatReader, Ihdr, InterlaceMethod, PngError, PngFilterError,
    decode_zlib, unfilter_row,
};

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
    Filter { row: usize, source: PngFilterError },
    UnsupportedFormat(Ihdr),
    UnknownCriticalChunk([u8; 4]),
    InvalidTransparencyChunk,
    InvalidPalette,
    CriticalCrcMismatch { kind: [u8; 4], offset: usize },
    SizeOverflow,
    LimitExceeded,
    AllocationFailed,
    ScanlineLength { expected: usize, actual: usize },
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
            _ => None,
        }
    }
}

impl From<PngError> for PngDecodeError {
    fn from(error: PngError) -> Self {
        Self::Container(error)
    }
}

/// Decodes the default image only, with strict critical-chunk CRC checks.
///
/// Supports only non-interlaced 8-bit RGBA. Ancillary chunks and trailing data
/// are ignored except that tRNS is rejected for RGBA. Color profiles, gamma,
/// HDR and animation are not interpreted. Limits bound output sizes, not input
/// size or CPU time.
pub fn decode_rgba8(input: &[u8], limits: PngDecodeLimits) -> Result<Rgba8Image, PngDecodeError> {
    let idat = IdatReader::new(input)?;
    let header = idat.structure().header;

    if header.color_type != ColorType::TruecolorAlpha
        || header.bit_depth != 8
        || header.interlace != InterlaceMethod::None
    {
        return Err(PngDecodeError::UnsupportedFormat(header));
    }

    if header.width > limits.max_width || header.height > limits.max_height {
        return Err(PngDecodeError::LimitExceeded);
    }

    let width = usize::try_from(header.width).map_err(|_| PngDecodeError::SizeOverflow)?;
    let height = usize::try_from(header.height).map_err(|_| PngDecodeError::SizeOverflow)?;

    let pixels = width
        .checked_mul(height)
        .ok_or(PngDecodeError::SizeOverflow)?;

    let row_bytes = width.checked_mul(4).ok_or(PngDecodeError::SizeOverflow)?;

    let stride = row_bytes
        .checked_add(1)
        .ok_or(PngDecodeError::SizeOverflow)?;

    let filtered_len = stride
        .checked_mul(height)
        .ok_or(PngDecodeError::SizeOverflow)?;

    if pixels > limits.max_pixels || filtered_len > limits.max_filtered_bytes {
        return Err(PngDecodeError::LimitExceeded);
    }

    let mut reader = ChunkReader::new(input)?;
    let mut seen_idat = false;
    let mut seen_palette = false;

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
                seen_idat = true;
            }

            b"IEND" => break,

            b"PLTE" => {
                if seen_palette
                    || seen_idat
                    || chunk.data.is_empty()
                    || chunk.data.len() > 768
                    || chunk.data.len() % 3 != 0
                {
                    return Err(PngDecodeError::InvalidPalette);
                }

                seen_palette = true;
            }

            b"tRNS" => {
                return Err(PngDecodeError::InvalidTransparencyChunk);
            }

            _ if chunk.is_critical() => {
                return Err(PngDecodeError::UnknownCriticalChunk(chunk.kind));
            }

            _ => {}
        }
    }

    let mut buffer = Vec::new();

    buffer
        .try_reserve_exact(filtered_len)
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

        unfilter_row(filter, &mut after[..row_bytes], previous, 4)
            .map_err(|source| PngDecodeError::Filter { row: y, source })?;
    }

    filtered.truncate(pixels * 4);

    Ok(Rgba8Image {
        width: header.width,
        height: header.height,
        pixels: filtered,
    })
}

#[cfg(test)]
mod tests;
