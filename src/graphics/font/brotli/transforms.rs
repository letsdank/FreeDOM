//! Brotli static dictionary transformations.
//!
//! Specification: RFC 7932, section 8 and appendix B.
//! <https://www.rfc-editor.org/rfc/rfc7932.html#appendix-B>

#[derive(Clone, Copy, Debug)]
enum Operation {
    Identity,
    UpperFirst,
    UpperAll,
    OmitFirst(usize),
    OmitLast(usize),
}

use Operation::{Identity, OmitFirst, OmitLast, UpperAll, UpperFirst};

type Transform = (&'static [u8], Operation, &'static [u8]);

// The index in this array is the transform ID from RFC 7932, appendix B.
const TRANSFORMS: [Transform; 121] = [
    (b"", Identity, b""),
    (b"", Identity, b" "),
    (b" ", Identity, b" "),
    (b"", OmitFirst(1), b""),
    (b"", UpperFirst, b" "),
    (b"", Identity, b" the "),
    (b" ", Identity, b""),
    (b"s ", Identity, b" "),
    (b"", Identity, b" of "),
    (b"", UpperFirst, b""),
    (b"", Identity, b" and "),
    (b"", OmitFirst(2), b""),
    (b"", OmitLast(1), b""),
    (b", ", Identity, b" "),
    (b"", Identity, b", "),
    (b" ", UpperFirst, b" "),
    (b"", Identity, b" in "),
    (b"", Identity, b" to "),
    (b"e ", Identity, b" "),
    (b"", Identity, b"\""),
    (b"", Identity, b"."),
    (b"", Identity, b"\">"),
    (b"", Identity, b"\n"),
    (b"", OmitLast(3), b""),
    (b"", Identity, b"]"),
    (b"", Identity, b" for "),
    (b"", OmitFirst(3), b""),
    (b"", OmitLast(2), b""),
    (b"", Identity, b" a "),
    (b"", Identity, b" that "),
    (b" ", UpperFirst, b""),
    (b"", Identity, b". "),
    (b".", Identity, b""),
    (b" ", Identity, b", "),
    (b"", OmitFirst(4), b""),
    (b"", Identity, b" with "),
    (b"", Identity, b"'"),
    (b"", Identity, b" from "),
    (b"", Identity, b" by "),
    (b"", OmitFirst(5), b""),
    (b"", OmitFirst(6), b""),
    (b" the ", Identity, b""),
    (b"", OmitLast(4), b""),
    (b"", Identity, b". The "),
    (b"", UpperAll, b""),
    (b"", Identity, b" on "),
    (b"", Identity, b" as "),
    (b"", Identity, b" is "),
    (b"", OmitLast(7), b""),
    (b"", OmitLast(1), b"ing "),
    (b"", Identity, b"\n\t"),
    (b"", Identity, b":"),
    (b" ", Identity, b". "),
    (b"", Identity, b"ed "),
    (b"", OmitFirst(9), b""),
    (b"", OmitFirst(7), b""),
    (b"", OmitLast(6), b""),
    (b"", Identity, b"("),
    (b"", UpperFirst, b", "),
    (b"", OmitLast(8), b""),
    (b"", Identity, b" at "),
    (b"", Identity, b"ly "),
    (b" the ", Identity, b" of "),
    (b"", OmitLast(5), b""),
    (b"", OmitLast(9), b""),
    (b" ", UpperFirst, b", "),
    (b"", UpperFirst, b"\""),
    (b".", Identity, b"("),
    (b"", UpperAll, b" "),
    (b"", UpperFirst, b"\">"),
    (b"", Identity, b"=\""),
    (b" ", Identity, b"."),
    (b".com/", Identity, b""),
    (b" the ", Identity, b" of the "),
    (b"", UpperFirst, b"'"),
    (b"", Identity, b". This "),
    (b"", Identity, b","),
    (b".", Identity, b" "),
    (b"", UpperFirst, b"("),
    (b"", UpperFirst, b"."),
    (b"", Identity, b" not "),
    (b" ", Identity, b"=\""),
    (b"", Identity, b"er "),
    (b" ", UpperAll, b" "),
    (b"", Identity, b"al "),
    (b" ", UpperAll, b""),
    (b"", Identity, b"='"),
    (b"", UpperAll, b"\""),
    (b"", UpperFirst, b". "),
    (b" ", Identity, b"("),
    (b"", Identity, b"ful "),
    (b" ", UpperFirst, b". "),
    (b"", Identity, b"ive "),
    (b"", Identity, b"less "),
    (b"", UpperAll, b"'"),
    (b"", Identity, b"est "),
    (b" ", UpperFirst, b"."),
    (b"", UpperAll, b"\">"),
    (b" ", Identity, b"='"),
    (b"", UpperFirst, b","),
    (b"", Identity, b"ize "),
    (b"", UpperAll, b"."),
    (b"\xc2\xa0", Identity, b""),
    (b" ", Identity, b","),
    (b"", UpperFirst, b"=\""),
    (b"", UpperAll, b"=\""),
    (b"", Identity, b"ous "),
    (b"", UpperAll, b", "),
    (b"", UpperFirst, b"='"),
    (b" ", UpperFirst, b","),
    (b" ", UpperAll, b"=\""),
    (b" ", UpperAll, b", "),
    (b"", UpperAll, b","),
    (b"", UpperAll, b"("),
    (b"", UpperAll, b". "),
    (b" ", UpperAll, b"."),
    (b"", UpperAll, b"='"),
    (b" ", UpperAll, b". "),
    (b" ", UpperFirst, b"=\""),
    (b" ", UpperAll, b"='"),
    (b" ", UpperFirst, b"='"),
];

pub(super) fn transform_word(id: usize, word: &[u8]) -> Option<Vec<u8>> {
    let &(prefix, operation, suffix) = TRANSFORMS.get(id)?;

    let word = match operation {
        OmitFirst(count) => &word[count.min(word.len())..],
        OmitLast(count) => &word[..word.len().saturating_sub(count)],
        _ => word,
    };

    let mut result = Vec::with_capacity(prefix.len() + word.len() + suffix.len());

    result.extend_from_slice(prefix);
    result.extend_from_slice(word);

    // Ferment applies only to the word, not to the prefix or suffix.
    let transformed_word = &mut result[prefix.len()..];

    match operation {
        UpperFirst if !transformed_word.is_empty() => {
            ferment(transformed_word, 0);
        }
        UpperAll => {
            let mut position = 0;

            while position < transformed_word.len() {
                position += ferment(transformed_word, position);
            }
        }
        _ => {}
    }

    result.extend_from_slice(suffix);

    Some(result)
}

// RFC 7932 section 8 defines Ferment as a byte operation, not Unicode casing.
fn ferment(word: &mut [u8], position: usize) -> usize {
    let first = word[position];

    if first < 192 {
        if first.is_ascii_lowercase() {
            word[position] ^= 32;
        }

        1
    } else if first < 224 {
        if let Some(byte) = word.get_mut(position + 1) {
            *byte ^= 32;
        }

        2
    } else {
        if let Some(byte) = word.get_mut(position + 2) {
            *byte ^= 5;
        }

        3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transformation_table_matches_rfc_checksum() {
        let mut bytes = Vec::new();

        for &(prefix, operation, suffix) in &TRANSFORMS {
            bytes.extend_from_slice(prefix);
            bytes.push(0);

            let code = match operation {
                Identity => 0,
                UpperFirst => 1,
                UpperAll => 2,
                OmitFirst(count) => 2 + count as u8,
                OmitLast(count) => 11 + count as u8,
            };

            bytes.push(code);
            bytes.extend_from_slice(suffix);
            bytes.push(0);
        }

        assert_eq!(bytes.len(), 648);

        let mut crc = 0xffff_ffff_u32;

        for byte in bytes {
            crc ^= u32::from(byte);

            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }

        assert_eq!(!crc, 0x3d96_5f81);
    }

    #[test]
    fn applies_representative_transforms() {
        let cases: &[(usize, &[u8])] = &[
            (0, b"test"),
            (1, b"test "),
            (2, b" test "),
            (3, b"est"),
            (9, b"Test"),
            (12, b"tes"),
            (32, b".test"),
            (44, b"TEST"),
            (64, b""),
            (102, b"\xc2\xa0test"),
            (120, b" Test='"),
        ];

        for &(id, expected) in cases {
            assert_eq!(
                transform_word(id, b"test").as_deref(),
                Some(expected),
                "transform {id}"
            );
        }

        assert_eq!(transform_word(121, b"test"), None);
    }

    #[test]
    fn ferments_multibyte_sequences_according_to_brotli() {
        assert_eq!(transform_word(9, &[0xc3, 0xa9]), Some(vec![0xc3, 0x89]));

        assert_eq!(
            transform_word(44, &[0xe0, 0xa4, 0x95, b'a']),
            Some(vec![0xe0, 0xa4, 0x90, b'A'])
        );
    }
}
