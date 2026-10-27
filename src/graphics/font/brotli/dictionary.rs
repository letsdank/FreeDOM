//! Access to the Brotli static dictionary.
//!
//! Dictionary bytes and word counts are defined by RFC 7932, appendix A.
//! <https://www.rfc-editor.org/rfc/rfc7932.html#appendix-A>
//!
//! The binary data was obtained from the official Brotli repository.
//! Its license is stored alongside `dictionary.bin`.

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
}
