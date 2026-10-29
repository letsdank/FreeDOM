//! String-to-string conversion for RegExp lastIndex.
//!
//! <https://tc39.es/ecma262/#sec-tonumber-applied-to-the-string-type>
//! <https://tc39.es/ecma262/#sec-tolength>

const MAX_LENGTH: u64 = 9_007_199_254_740_991;

pub(crate) fn from_number(number: f64) -> usize {
    if number.is_nan() || number <= 0.0 {
        return 0;
    }

    number.floor().min(MAX_LENGTH as f64) as usize
}

pub(crate) fn from_string(input: &str) -> usize {
    let input = input.trim_matches(is_ecmascript_whitespace);

    if input.is_empty() {
        return 0;
    }

    match input {
        "Infinity" | "+Infinity" => {
            return from_number(f64::INFINITY);
        }

        "-Infinity" => return 0,

        _ => {}
    }

    let radix = if input.starts_with("0x") || input.starts_with("0X") {
        Some(16)
    } else if input.starts_with("0o") || input.starts_with("0O") {
        Some(8)
    } else if input.starts_with("0b") || input.starts_with("0B") {
        Some(2)
    } else {
        None
    };

    if let Some(radix) = radix {
        return from_radix(&input[2..], radix);
    }

    if !is_decimal(input) {
        return 0;
    }

    from_number(input.parse::<f64>().unwrap_or(f64::NAN))
}

fn from_radix(digits: &str, radix: u32) -> usize {
    if digits.is_empty() {
        return 0;
    }

    let mut number = 0_u64;

    for byte in digits.bytes() {
        let digit = match byte {
            b'0'..=b'9' => u32::from(byte - b'0'),
            b'a'..=b'f' => u32::from(byte - b'a') + 10,
            b'A'..=b'F' => u32::from(byte - b'A') + 10,
            _ => return 0,
        };

        if digit >= radix {
            return 0;
        }

        number = number
            .saturating_mul(radix as u64)
            .saturating_add(digit as u64)
            .min(MAX_LENGTH);
    }

    usize::try_from(number).unwrap_or(usize::MAX)
}

fn is_decimal(input: &str) -> bool {
    let bytes = input.as_bytes();
    let mut cursor = 0;

    if matches!(bytes.first(), Some(b'+') | Some(b'-')) {
        cursor += 1;
    }

    let mut digits = 0;

    while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
        cursor += 1;
        digits += 1;
    }

    if bytes.get(cursor) == Some(&b'.') {
        cursor += 1;

        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
            digits += 1;
        }
    }

    if digits == 0 {
        return false;
    }

    if matches!(bytes.get(cursor), Some(b'e') | Some(b'E')) {
        cursor += 1;

        if matches!(bytes.get(cursor), Some(b'+') | Some(b'-')) {
            cursor += 1;
        }

        let start = cursor;

        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }

        if cursor == start {
            return false;
        }
    }

    cursor == bytes.len()
}

fn is_ecmascript_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    ) || ('\u{2000}'..='\u{200A}').contains(&character)
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
