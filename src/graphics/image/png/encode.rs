//! Non-interlaced RGBA8 PNG encoding using stored DEFLATE blocks.
//!
//! PNG Third Edition, sections 5, 7, 9, 10 and 11:
//! <https://www.w3.org/TR/png-3>

use std::{error::Error, fmt, io};

use super::{MAX_CHUNK_LENGTH, SIGNATURE, encode_zlib_stored, update_crc};

#[derive(Debug)]
pub enum PngEncodeError {
    InvalidDimensions,
    SizeOverflow,
    PixelLength { expected: usize, actual: usize },
    ChunkTooLarge,
    AllocationFailed,
    Io(io::Error),
}

impl fmt::Display for PngEncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PNG encode error: {self:?}")
    }
}

impl Error for PngEncodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

fn write_chunk(output: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    output.extend_from_slice(&(data.len() as u32).to_be_bytes());

    output.extend_from_slice(kind);
    output.extend_from_slice(data);

    let crc = update_crc(update_crc(0xffff_ffff, kind), data) ^ 0xffff_ffff;

    output.extend_from_slice(&crc.to_be_bytes());
}

/// Encodes unassociated RGBA8 samples as a non-interlaced PNG.
///
/// Uses filter 0 and uncompressed DEFLATE. No color metadata is written.
pub fn encode_rgba8(width: u32, height: u32, pixels: &[u8]) -> Result<Vec<u8>, PngEncodeError> {
    if width == 0 || height == 0 || width > 0x7fff_ffff || height > 0x7fff_ffff {
        return Err(PngEncodeError::InvalidDimensions);
    }

    let width = usize::try_from(width).map_err(|_| PngEncodeError::SizeOverflow)?;
    let height = usize::try_from(height).map_err(|_| PngEncodeError::SizeOverflow)?;

    let row_bytes = width.checked_mul(4).ok_or(PngEncodeError::SizeOverflow)?;

    let expected = row_bytes
        .checked_mul(height)
        .ok_or(PngEncodeError::SizeOverflow)?;

    if pixels.len() != expected {
        return Err(PngEncodeError::PixelLength {
            expected,
            actual: pixels.len(),
        });
    }

    // Each scanline has one additional filter byte.
    let filtered_len = row_bytes
        .checked_add(1)
        .and_then(|stride| stride.checked_mul(height))
        .ok_or(PngEncodeError::SizeOverflow)?;

    let blocks = filtered_len / 65_535 + usize::from(filtered_len % 65_535 != 0);

    // Each stored block adds five bytes; zlib adds a two-byte
    // header and a four-byte Adler32- checksum.
    let zlib_len = blocks
        .checked_mul(5)
        .and_then(|size| size.checked_add(filtered_len))
        .and_then(|size| size.checked_add(6))
        .ok_or(PngEncodeError::SizeOverflow)?;

    if zlib_len > MAX_CHUNK_LENGTH as usize {
        return Err(PngEncodeError::ChunkTooLarge);
    }

    // Signature + IHDR + IDAT framing + IEND.
    let capacity = zlib_len
        .checked_add(57)
        .ok_or(PngEncodeError::SizeOverflow)?;

    let mut filtered = Vec::new();

    filtered
        .try_reserve_exact(filtered_len)
        .map_err(|_| PngEncodeError::AllocationFailed)?;

    for row in pixels.chunks_exact(row_bytes) {
        filtered.push(0);
        filtered.extend_from_slice(row);
    }

    let mut output = Vec::new();

    output
        .try_reserve_exact(capacity)
        .map_err(|_| PngEncodeError::AllocationFailed)?;

    output.extend_from_slice(SIGNATURE);

    let mut ihdr = [0u8; 13];

    ihdr[..4].copy_from_slice(&(width as u32).to_be_bytes());
    ihdr[4..8].copy_from_slice(&(height as u32).to_be_bytes());

    ihdr[8] = 8; // Bit depth.
    ihdr[9] = 6; // RGBA.

    write_chunk(&mut output, b"IHDR", &ihdr);

    output.extend_from_slice(&(zlib_len as u32).to_be_bytes());
    output.extend_from_slice(b"IDAT");

    let data_start = output.len();

    encode_zlib_stored(&filtered, &mut output).map_err(PngEncodeError::Io)?;

    let crc = update_crc(update_crc(0xffff_ffff, b"IDAT"), &output[data_start..]) ^ 0xffff_ffff;

    output.extend_from_slice(&crc.to_be_bytes());

    write_chunk(&mut output, b"IEND", &[]);

    Ok(output)
}

#[cfg(test)]
#[path = "encode_tests.rs"]
mod tests;
