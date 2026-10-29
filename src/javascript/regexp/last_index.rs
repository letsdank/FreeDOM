//! Conversion to a length for RegExp lastIndex.
//!
//! <https://tc39.es/ecma262/#sec-stringtonumber>
//! <https://tc39.es/ecma262/#sec-tolength>

const MAX_LENGTH: u64 = 9_007_199_254_740_991;

pub(crate) fn from_number(number: f64) -> usize {
    if number.is_nan() || number <= 0.0 {
        return 0;
    }

    number.floor().min(MAX_LENGTH as f64) as usize
}

pub(crate) fn from_string(input: &str) -> usize {
    from_number(crate::javascript::number::string_to_number(input))
}

#[cfg(test)]
mod tests {
    use super::{from_number, from_string};

    #[test]
    fn parses_string_to_length() {
        for (input, expected) in [
            ("", 0),
            ("  \u{feff}2.9\n", 2),
            ("+.8", 0),
            ("12e-1", 1),
            ("0x10", 16),
            ("0b101", 5),
            ("0O10", 8),
            ("0x", 0),
            ("0b102", 0),
            ("-0x1", 0),
            ("1e", 0),
            ("1_0", 0),
            ("inf", 0),
            ("2\u{0085}", 0),
            ("nonsense", 0),
        ] {
            assert_eq!(from_string(input), expected, "{input:?}");
        }

        assert_eq!(from_string("Infinity"), from_number(f64::INFINITY));

        assert_eq!(
            from_string("0xffffffffffffffffffffffff"),
            from_number(f64::INFINITY)
        );

        assert_eq!(from_string("0xffffffffffffffffffffffffz"), 0);
    }
}
