//! String-to-number conversion for the JavaScript interpreter.
//!
//! <https://tc39.es/ecma262/#sec-stringtonumber>
//!
//! The caller is responsible for converting non-string JavaScript values.

pub(super) fn string_to_number(input: &str) -> f64 {
    let input = input.trim_matches(is_ecmascript_whitespace);

    if input.is_empty() {
        return 0.0;
    }

    match input {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
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
        return radix_number(&input[2..], radix);
    }

    if !is_decimal(input) {
        return f64::NAN;
    }

    input.parse::<f64>().unwrap_or(f64::NAN)
}

// The non-decimal grammar contains only binary, octal, and hexadecimal
// integers. Keep the leading 53 bits, then round ties to even.
fn radix_number(digits: &str, radix: u32) -> f64 {
    if digits.is_empty() {
        return f64::NAN;
    }

    let width = match radix {
        2 => 1,
        8 => 3,
        16 => 4,
        _ => return f64::NAN,
    };

    let mut top = 0_u64;
    let mut bit_count = 0_usize;
    let mut round_bit = false;
    let mut sticky = false;

    for byte in digits.bytes() {
        let digit = match byte {
            b'0'..=b'9' => u32::from(byte - b'0'),
            b'a'..=b'f' => u32::from(byte - b'a') + 10,
            b'A'..=b'F' => u32::from(byte - b'A') + 10,
            _ => return f64::NAN,
        };

        if digit >= radix {
            return f64::NAN;
        }

        for shift in (0..width).rev() {
            let bit = (digit >> shift) & 1;

            if bit_count == 0 && bit == 0 {
                continue;
            }

            bit_count = bit_count.saturating_add(1);

            if bit_count <= 53 {
                top = (top << 1) | u64::from(bit);
            } else if bit_count == 54 {
                round_bit = bit != 0;
            } else {
                sticky |= bit != 0;
            }
        }
    }

    if bit_count <= 53 {
        return top as f64;
    }

    if round_bit && (sticky || top & 1 != 0) {
        top += 1;
    }

    let shift = bit_count - 53;

    if shift > 1023 {
        return f64::INFINITY;
    }

    top as f64 * 2.0_f64.powi(shift as i32)
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
    use super::string_to_number;

    #[test]
    fn parses_numbers_and_ecmascript_whitespace() {
        for (input, expected) in [
            ("", 0.0),
            ("  \u{feff}2.9\n", 2.9),
            ("+.8", 0.8),
            ("12e-1", 1.2),
            ("0x10", 16.0),
            ("0b101", 5.0),
            ("0O10", 8.0),
        ] {
            assert_eq!(string_to_number(input), expected, "{input:?}");
        }

        assert!(string_to_number("-0").is_sign_negative());

        assert_eq!(string_to_number("Infinity"), f64::INFINITY,);
        assert_eq!(string_to_number("-Infinity"), f64::NEG_INFINITY);

        for input in [
            "0x",
            "0b102",
            "-0x1",
            "1e",
            "1_0",
            "inf",
            "2\u{0085}",
            "nonsense",
        ] {
            assert!(string_to_number(input).is_nan(), "{input:?}");
        }
    }

    #[test]
    fn rounds_large_radix_numbers_to_even() {
        assert_eq!(
            string_to_number("0x1fffffffffffff"),
            9_007_199_254_740_991.0,
        );

        assert_eq!(
            string_to_number("0x20000000000001"),
            9_007_199_254_740_992.0,
        );

        assert_eq!(
            string_to_number("0x20000000000003"),
            9_007_199_254_740_996.0,
        );

        assert_eq!(
            string_to_number(&format!("0x{}", "f".repeat(256))),
            f64::INFINITY,
        );

        assert!(string_to_number(&format!("0x{}z", "f".repeat(256))).is_nan());
    }
}
