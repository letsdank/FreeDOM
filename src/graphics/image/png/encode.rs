//! Non-interlaced RGBA8 PNG encoding using DEFLATE.
//!
//! PNG Third Edition, sections 5, 7, 9, 10 and 11:
//! <https://www.w3.org/TR/png-3>

use std::{
    error::Error,
    fmt,
    io::{self, Write},
};

use super::{
    MAX_CHUNK_LENGTH, PngFilterError, SIGNATURE, encode_zlib_fixed_lz77, encode_zlib_stored,
    filter_row, update_crc,
};

#[derive(Debug)]
pub enum PngEncodeError {
    InvalidDimensions,
    SizeOverflow,
    PixelLength { expected: usize, actual: usize },
    ChunkTooLarge,
    AllocationFailed,
    Filter(PngFilterError),
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
            Self::Filter(error) => Some(error),
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

/// Collects a compressed candidate until it reaches the size limit.
struct CandidateWriter {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
    allocation_failed: bool,
}

impl Write for CandidateWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit - self.bytes.len() {
            self.exceeded = true;

            return Err(io::Error::other("compressed candidate exceeds limit"));
        }

        if self.bytes.try_reserve(bytes.len()).is_err() {
            self.allocation_failed = true;

            return Err(io::Error::other("compressed candidate allocation failed"));
        }

        self.bytes.extend_from_slice(bytes);

        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// Sum absolute signed residuals, as suggested in PNG Third Edition, 12.7.
fn filter_score(row: &[u8]) -> u64 {
    row.iter()
        .map(|&byte| u64::from((byte as i8).unsigned_abs()))
        .sum()
}

/// Encodes unassociated RGBA8 samples as a non-interlaced PNG.
///
/// Chooses a filter for each row and the smaller of fixed-Huffman LZ77
/// and stored DEFLATE. No color metadata is written.
pub fn encode_rgba8(width: u32, height: u32, pixels: &[u8]) -> Result<Vec<u8>, PngEncodeError> {
    encode_rgba(width, height, pixels, 8)
}

/// Encodes unassociated RGBA16 samples in big-endian byte order.
///
/// The input contains eight bytes per pixel in R, G, B, A order.
/// No color metadata is written.
pub fn encode_rgba16_be(width: u32, height: u32, pixels: &[u8]) -> Result<Vec<u8>, PngEncodeError> {
    encode_rgba(width, height, pixels, 16)
}

fn encode_rgba(
    width: u32,
    height: u32,
    pixels: &[u8],
    bit_depth: u8,
) -> Result<Vec<u8>, PngEncodeError> {
    if width == 0 || height == 0 || width > 0x7fff_ffff || height > 0x7fff_ffff {
        return Err(PngEncodeError::InvalidDimensions);
    }

    let width = usize::try_from(width).map_err(|_| PngEncodeError::SizeOverflow)?;
    let height = usize::try_from(height).map_err(|_| PngEncodeError::SizeOverflow)?;

    let bytes_per_pixel = 4 * usize::from(bit_depth / 8);

    let row_bytes = width
        .checked_mul(bytes_per_pixel)
        .ok_or(PngEncodeError::SizeOverflow)?;

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
    let stored_len = blocks
        .checked_mul(5)
        .and_then(|size| size.checked_add(filtered_len))
        .and_then(|size| size.checked_add(6))
        .ok_or(PngEncodeError::SizeOverflow)?;

    if stored_len > MAX_CHUNK_LENGTH as usize {
        return Err(PngEncodeError::ChunkTooLarge);
    }

    let mut filtered = Vec::new();

    filtered
        .try_reserve_exact(filtered_len)
        .map_err(|_| PngEncodeError::AllocationFailed)?;

    let mut scratch = Vec::new();

    scratch
        .try_reserve_exact(row_bytes)
        .map_err(|_| PngEncodeError::AllocationFailed)?;

    scratch.resize(row_bytes, 0);

    for (y, row) in pixels.chunks_exact(row_bytes).enumerate() {
        let previous = if y == 0 {
            None
        } else {
            Some(&pixels[(y - 1) * row_bytes..y * row_bytes])
        };

        let start = filtered.len();

        // Start with filter 0, None.
        filtered.push(0);
        filtered.extend_from_slice(row);

        let mut best_score = filter_score(row);

        for filter in 1..=4 {
            if best_score == 0 {
                break;
            }

            filter_row(filter, row, previous, bytes_per_pixel, &mut scratch)
                .map_err(PngEncodeError::Filter)?;

            let score = filter_score(&scratch);

            if score < best_score {
                best_score = score;

                filtered[start] = filter;

                filtered[start + 1..start + 1 + row_bytes].copy_from_slice(&scratch);
            }
        }
    }

    // Stop once the candidate cannot beat the stored representation.
    let mut candidate = CandidateWriter {
        bytes: Vec::new(),
        limit: stored_len - 1,
        exceeded: false,
        allocation_failed: false,
    };

    let compressed = match encode_zlib_fixed_lz77(&filtered, &mut candidate) {
        Ok(()) => Some(candidate.bytes),

        Err(_) if candidate.exceeded => None,

        Err(_) if candidate.allocation_failed => {
            return Err(PngEncodeError::AllocationFailed);
        }

        Err(error) => {
            return Err(PngEncodeError::Io(error));
        }
    };

    let zlib_len = compressed.as_ref().map_or(stored_len, Vec::len);

    // Signature + IHDR + IDAT framing + IEND.
    let capacity = zlib_len
        .checked_add(57)
        .ok_or(PngEncodeError::SizeOverflow)?;

    let mut output = Vec::new();

    output
        .try_reserve_exact(capacity)
        .map_err(|_| PngEncodeError::AllocationFailed)?;

    output.extend_from_slice(SIGNATURE);

    let mut ihdr = [0u8; 13];

    ihdr[..4].copy_from_slice(&(width as u32).to_be_bytes());
    ihdr[4..8].copy_from_slice(&(height as u32).to_be_bytes());

    ihdr[8] = bit_depth; // Bit depth.
    ihdr[9] = 6; // RGBA.

    write_chunk(&mut output, b"IHDR", &ihdr);

    output.extend_from_slice(&(zlib_len as u32).to_be_bytes());
    output.extend_from_slice(b"IDAT");

    let data_start = output.len();

    if let Some(compressed) = compressed {
        output.extend_from_slice(&compressed);
    } else {
        encode_zlib_stored(&filtered, &mut output).map_err(PngEncodeError::Io)?;
    }

    let crc = update_crc(update_crc(0xffff_ffff, b"IDAT"), &output[data_start..]) ^ 0xffff_ffff;

    output.extend_from_slice(&crc.to_be_bytes());

    write_chunk(&mut output, b"IEND", &[]);

    Ok(output)
}

#[cfg(test)]
#[path = "encode_tests.rs"]
mod tests;
