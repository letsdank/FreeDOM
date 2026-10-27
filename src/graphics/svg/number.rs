//! Number parsing shared by SVG attributes, paths, and transforms.
//!
//! SVG 2 data types: <https://www.w3.org/TR/SVG2/types.html>

use super::SvgError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NumberErrorKind {
    ExpectedNumber,
    ExpectedExponentDigits,
    InvalidNumber,
    NonFinite,
}

impl NumberErrorKind {
    pub(super) fn parser_message(self) -> &'static str {
        match self {
            Self::ExpectedNumber => "expected a number",
            Self::ExpectedExponentDigits => "expected exponent digits",
            Self::InvalidNumber => "invalid number",
            Self::NonFinite => "number must be finite",
        }
    }

    fn attribute_message(self) -> &'static str {
        match self {
            Self::ExpectedNumber => "expected SVG number",
            Self::ExpectedExponentDigits => "expected exponent digits",
            Self::InvalidNumber => "invalid SVG number",
            Self::NonFinite => "SVG number must be finite",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct NumberError {
    pub(super) offset: usize,
    pub(super) kind: NumberErrorKind,
}

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
    parse_number(input, cursor).map_err(|error| SvgError::new(error.kind.attribute_message()))
}

pub(super) fn parse_number(input: &str, cursor: &mut usize) -> Result<f32, NumberError> {
    let bytes = input.as_bytes();
    let start = *cursor;

    if matches!(bytes.get(*cursor), Some(b'+' | b'-')) {
        *cursor += 1;
    }

    let integer_start = *cursor;

    while bytes.get(*cursor).is_some_and(u8::is_ascii_digit) {
        *cursor += 1;
    }

    let has_integer = *cursor != integer_start;

    if bytes.get(*cursor) == Some(&b'.') {
        *cursor += 1;
    }

    let fraction_start = *cursor;

    while bytes.get(*cursor).is_some_and(u8::is_ascii_digit) {
        *cursor += 1;
    }

    let has_fraction = *cursor != fraction_start;

    if !has_integer && !has_fraction {
        return Err(NumberError {
            offset: start,
            kind: NumberErrorKind::ExpectedNumber,
        });
    }

    if matches!(bytes.get(*cursor), Some(b'e' | b'E')) {
        *cursor += 1;

        if matches!(bytes.get(*cursor), Some(b'+' | b'-')) {
            *cursor += 1;
        }

        let exponent_start = *cursor;

        while bytes.get(*cursor).is_some_and(u8::is_ascii_digit) {
            *cursor += 1;
        }

        if *cursor == exponent_start {
            return Err(NumberError {
                offset: *cursor,
                kind: NumberErrorKind::ExpectedExponentDigits,
            });
        }
    }

    let value = input[start..*cursor]
        .parse::<f32>()
        .map_err(|_| NumberError {
            offset: start,
            kind: NumberErrorKind::InvalidNumber,
        })?;

    if !value.is_finite() {
        return Err(NumberError {
            offset: start,
            kind: NumberErrorKind::NonFinite,
        });
    }

    Ok(value)
}
