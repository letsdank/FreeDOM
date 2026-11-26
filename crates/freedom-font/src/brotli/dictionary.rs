//! Access to the Brotli static dictionary.
//!
//! Dictionary bytes and word counts are defined by RFC 7932, appendix A.
//! <https://www.rfc-editor.org/rfc/rfc7932.html#appendix-A>
//!
//! The binary data was obtained from the official Brotli repository.
//! Its license is stored alongside `dictionary.bin`.

use super::{FontError, error};

const DICTIONARY: &[u8; 122_784] = include_bytes!("dictionary.bin");

// RFC 7932, appendix A: number of index bits for each word length.
const WORD_INDEX_BITS: [u8; 25] = [
    0, 0, 0, 0, 10, 10, 11, 11, 10, 10, 10, 10, 10, 9, 9, 8, 7, 7, 8, 7, 7, 6, 6, 5, 5,
];

/// Returns a base dictionary word before any Brotli transformation.
pub(super) fn base_word(length: usize, index: usize) -> Option<&'static [u8]> {
    if !(4..=24).contains(&length) {
        return None;
    }

    let word_count = 1_usize << WORD_INDEX_BITS[length];

    if index >= word_count {
        return None;
    }

    let offset = (4..length)
        .map(|word_length| word_length * (1_usize << WORD_INDEX_BITS[word_length]))
        .sum::<usize>();

    let start = offset.checked_add(index.checked_mul(length)?)?;
    let end = start.checked_add(length)?;

    DICTIONARY.get(start..end)
}

/// Resolves a static dictionary reference and applies its transformation.
pub(super) fn expand_word(
    length: usize,
    distance: usize,
    maximum_backward_distance: usize,
    offset: usize,
) -> Result<Vec<u8>, FontError> {
    if !(4..=24).contains(&length) {
        return Err(error(offset, "invalid Brotli dictionary word length"));
    }

    let word_id = distance
        .checked_sub(maximum_backward_distance)
        .and_then(|value| value.checked_sub(1))
        .ok_or_else(|| error(offset, "invalid Brotli dictionary distance"))?;

    let bits = WORD_INDEX_BITS[length];
    let index = word_id & ((1_usize << bits) - 1);
    let transform = word_id >> bits;

    if transform >= 121 {
        return Err(error(offset, "invalid Brotli dictionary transform"));
    }

    let word = base_word(length, index)
        .ok_or_else(|| error(offset, "invalid Brotli dictionary word index"))?;

    super::transforms::transform_word(transform, word)
        .ok_or_else(|| error(offset, "invalid Brotli dictionary transform"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_dictionary_has_the_rfc_checksum() {
        let mut crc = 0xffff_ffff_u32;

        for &byte in DICTIONARY {
            crc ^= u32::from(byte);

            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }

        assert_eq!(!crc, 0x5136_cb04);
    }

    #[test]
    fn reads_static_dictionary_base_words() {
        assert_eq!(base_word(4, 0), Some(&b"time"[..]));
        assert_eq!(base_word(4, 1), Some(&b"down"[..]));
        assert_eq!(base_word(4, 277), Some(&b"test"[..]));

        assert_eq!(base_word(24, 31).unwrap().len(), 24);
    }

    #[test]
    fn rejects_invalid_static_dictionary_word_indexes() {
        assert_eq!(base_word(3, 0), None);
        assert_eq!(base_word(4, 1024), None);
        assert_eq!(base_word(24, 32), None);
        assert_eq!(base_word(25, 0), None);
    }

    #[test]
    fn resolves_identity_and_ferment_first_dictionary_words() {
        // Word 277 is "test"; transform 0 is Identity.
        assert_eq!(expand_word(4, 278, 0, 0).unwrap(), b"test",);

        // Transform 9 is FermentFirst.
        assert_eq!(expand_word(4, 9494, 0, 0).unwrap(), b"Test",);
    }

    #[test]
    fn rejects_invalid_dictionary_references() {
        assert!(expand_word(3, 1, 0, 0).is_err());
        assert!(expand_word(4, 10, 10, 0).is_err());

        // Transform IDs are in the range 0..121.
        assert!(expand_word(4, 1 + 121 * 1024, 0, 0).is_err());
    }
}
