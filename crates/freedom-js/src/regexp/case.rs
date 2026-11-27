//! Case-insensitive matching for RegExp without Unicode mode.
//!
//! <https://tc39.es/ecma262/#sec-runtime-semantics-canonicalize>
//! Uppercase mappings come from Rust's Unicode tables.

use std::{collections::HashMap, sync::OnceLock};

pub(super) fn canonicalize(unit: u16) -> u16 {
    let Some(character) = char::from_u32(u32::from(unit)) else {
        return unit;
    };

    let mut uppercase = character.to_uppercase();

    let Some(first) = uppercase.next() else {
        return unit;
    };

    if uppercase.next().is_some() || first.len_utf16() != 1 {
        return unit;
    }

    let mapped = first as u16;

    if unit >= 0x80 && mapped < 0x80 {
        return unit;
    }

    mapped
}

pub(super) fn code_units_equal(left: u16, right: u16, ignore_case: bool) -> bool {
    left == right || (ignore_case && canonicalize(left) == canonicalize(right))
}

pub(super) fn range_contains(start: u16, end: u16, unit: u16, ignore_case: bool) -> bool {
    if (start..=end).contains(&unit) {
        return true;
    }

    if !ignore_case {
        return false;
    }

    let canonical = canonicalize(unit);

    if (start..=end).contains(&canonical) {
        return true;
    }

    if unit < 0x80 && end < 0x80 {
        let byte = unit as u8;

        let other = if byte.is_ascii_lowercase() {
            byte.to_ascii_uppercase()
        } else if byte.is_ascii_uppercase() {
            byte.to_ascii_lowercase()
        } else {
            return false;
        };

        return (start..=end).contains(&(other as u16));
    }

    reverse_mappings()
        .get(&canonical)
        .is_some_and(|units| units.iter().any(|value| (start..=end).contains(value)))
}

fn reverse_mappings() -> &'static HashMap<u16, Vec<u16>> {
    static MAPPINGS: OnceLock<HashMap<u16, Vec<u16>>> = OnceLock::new();

    MAPPINGS.get_or_init(|| {
        let mut mappings: HashMap<u16, Vec<u16>> = HashMap::new();

        for unit in 0..=u16::MAX {
            let canonical = canonicalize(unit);

            if canonical != unit {
                mappings.entry(canonical).or_default().push(unit);
            }
        }

        mappings
    })
}

#[cfg(test)]
mod tests {
    use super::{canonicalize, code_units_equal, range_contains};

    #[test]
    fn follows_legacy_canonicalization_rules() {
        assert!(code_units_equal('é' as u16, 'É' as u16, true));
        assert!(code_units_equal('σ' as u16, 'ς' as u16, true));

        assert!(!code_units_equal('ſ' as u16, 'S' as u16, true));
        assert!(!code_units_equal('K' as u16, 'K' as u16, true));

        // Uppercase of ß contains more than one character.
        assert_eq!(canonicalize('ß' as u16), 'ß' as u16);

        // A lone surrogate is not a Rust char.
        assert_eq!(canonicalize(0xD800), 0xD800);
    }

    #[test]
    fn finds_all_equivalent_members_of_a_range() {
        assert!(range_contains('à' as u16, 'ö' as u16, 'Ä' as u16, true,));
        assert!(range_contains('À' as u16, 'Ö' as u16, 'ä' as u16, true,));
        assert!(!range_contains('a' as u16, 'z' as u16, 'ſ' as u16, true,));
        assert!(!range_contains('à' as u16, 'ö' as u16, 'Ä' as u16, false,));
    }
}
