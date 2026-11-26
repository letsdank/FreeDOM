//! Parsing of SVG stroke properties.
//!
//! Specification: <https://www.w3.org/TR/SVG2/painting.html#StrokeProperties>

use super::number::{parse_number_list, parse_svg_number, skip_svg_number_separators};
use super::{StrokeLength, StrokeLineCap, StrokeLineJoin, SvgError};

pub(super) fn parse_stroke_line_cap(value: &str) -> Result<StrokeLineCap, SvgError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "butt" => Ok(StrokeLineCap::Butt),
        "round" => Ok(StrokeLineCap::Round),
        "square" => Ok(StrokeLineCap::Square),
        _ => Err(SvgError::new("unsupported stroke-linecap")),
    }
}

pub(super) fn parse_stroke_line_join(value: &str) -> Result<StrokeLineJoin, SvgError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "miter" => Ok(StrokeLineJoin::Miter),
        "round" => Ok(StrokeLineJoin::Round),
        "bevel" => Ok(StrokeLineJoin::Bevel),
        _ => Err(SvgError::new("unsupported stroke-linejoin")),
    }
}

pub(super) fn parse_stroke_miter_limit(value: &str) -> Result<f32, SvgError> {
    let values = parse_number_list(value)?;

    if values.len() != 1 || !values[0].is_finite() || values[0] < 1.0 {
        return Err(SvgError::new(
            "stroke-miterlimit must be a finite number no smaller than one",
        ));
    }

    Ok(values[0])
}

pub(super) fn parse_stroke_dash_array(value: &str) -> Result<Vec<StrokeLength>, SvgError> {
    if value.trim().eq_ignore_ascii_case("none") {
        return Ok(Vec::new());
    }

    let values = parse_stroke_length_list(value, false)?;

    if values.is_empty() {
        return Err(SvgError::new("invalid stroke-dasharray"));
    }

    Ok(values)
}

pub(super) fn parse_stroke_dash_offset(value: &str) -> Result<StrokeLength, SvgError> {
    let values = parse_stroke_length_list(value, true)?;

    let [value] = values.as_slice() else {
        return Err(SvgError::new("invalid stroke-dashoffset"));
    };

    Ok(*value)
}

pub(super) fn parse_stroke_width(value: &str) -> Result<StrokeLength, SvgError> {
    let values = parse_stroke_length_list(value, false)?;

    let [value] = values.as_slice() else {
        return Err(SvgError::new("invalid stroke-width"));
    };

    Ok(*value)
}

fn parse_stroke_length_list(
    input: &str,
    allows_negative: bool,
) -> Result<Vec<StrokeLength>, SvgError> {
    let mut values = Vec::new();
    let mut cursor = 0;

    while cursor < input.len() {
        skip_svg_number_separators(input, &mut cursor);

        if cursor == input.len() {
            break;
        }

        let value = parse_svg_number(input, &mut cursor)?;
        let bytes = input.as_bytes();

        let length = if bytes.get(cursor) == Some(&b'%') {
            cursor += 1;
            StrokeLength::Percentage(value / 100.0)
        } else if matches!(bytes.get(cursor), Some(b'p' | b'P'))
            && matches!(bytes.get(cursor + 1), Some(b'x' | b'X'))
        {
            cursor += 2;
            StrokeLength::UserUnits(value)
        } else {
            StrokeLength::UserUnits(value)
        };

        let numeric_value = match length {
            StrokeLength::UserUnits(value) => value,
            StrokeLength::Percentage(value) => value,
        };

        if !numeric_value.is_finite() || (!allows_negative && numeric_value < 0.0) {
            return Err(SvgError::new("invalid SVG stroke length"));
        }

        values.push(length);
    }

    Ok(values)
}
