//! Number parsing shared by SVG attributes.
//!
//! SVG 2 data types: <https://www.w3.org/TR/SVG2/types.html>

use super::SvgError;

pub(super) fn parse_number_list(input: &str) -> Result<Vec<f32>, SvgError> {
    let mut values = Vec::new();
    let mut cursor = 0;

    while cursor < input.len() {
        skip_svg_number_separators(input, &mut cursor);

        if cursor == input.len() {
            break;
        }

        values.push(parse_svg_number(input, &mut cursor)?);
    }

    Ok(values)
}

pub(super) fn skip_svg_number_separators(input: &str, cursor: &mut usize) {
    while input
        .as_bytes()
        .get(*cursor)
        .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b',')
    {
        *cursor += 1;
    }
}

pub(super) fn parse_svg_number(input: &str, cursor: &mut usize) -> Result<f32, SvgError> {
    let bytes = input.as_bytes();
    let start = *cursor;

    if matches!(bytes.get(*cursor), Some(b'+' | b'-')) {
        *cursor += 1;
    }

    let integer_start = *cursor;

    while matches!(bytes.get(*cursor),Some(byte)if byte.is_ascii_digit()) {
        *cursor += 1;
    }

    let has_integer = *cursor != integer_start;

    if bytes.get(*cursor) == Some(&b'.') {
        *cursor += 1;
    }

    let fraction_start = *cursor;

    while matches!(bytes.get(*cursor),Some(byte)if byte.is_ascii_digit()) {
        *cursor += 1;
    }

    let has_fraction = *cursor != fraction_start;

    if !has_integer && !has_fraction {
        return Err(SvgError::new("expected SVG number"));
    }

    if matches!(bytes.get(*cursor), Some(b'e' | b'E')) {
        *cursor += 1;

        if matches!(bytes.get(*cursor), Some(b'+' | b'-')) {
            *cursor += 1;
        }

        let exponent_start = *cursor;

        while matches!(bytes.get(*cursor), Some(byte)if byte.is_ascii_digit()) {
            *cursor += 1;
        }

        if *cursor == exponent_start {
            return Err(SvgError::new("expected exponent digits"));
        }
    }

    let value = input[start..*cursor]
        .parse::<f32>()
        .map_err(|_| SvgError::new("invalid SVG number"))?;

    if !value.is_finite() {
        return Err(SvgError::new("SVG number must be finite"));
    }

    Ok(value)
}
