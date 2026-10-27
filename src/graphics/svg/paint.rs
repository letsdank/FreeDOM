//! SVG paint references, colors, and opacity.
//!
//! Specification: <https://www.w3.org/TR/SVG2/painting.html>

use super::{
    Color, HtmlElement, Paint, Path, SvgError, SvgPaintReference, Transform, Url,
    local_reference_id, parse_number_list, resolve_linear_gradient,
};

pub(super) fn resolve_paint(
    root: &HtmlElement,
    base_url: &Url,
    paint: &Option<SvgPaintReference>,
    object_path: &Path,
    transform: Transform,
    opacity: f32,
) -> Result<(Option<Color>, Option<Paint>), SvgError> {
    let Some(paint) = paint else {
        return Ok((None, None));
    };

    match paint {
        SvgPaintReference::Solid(color) => Ok((Some(apply_opacity(*color, opacity)), None)),

        SvgPaintReference::Url {
            reference,
            fallback,
        } => {
            let fallback = fallback.map(|color| apply_opacity(color, opacity));

            let Some(reference_id) = local_reference_id(base_url, reference)? else {
                return Ok((fallback, None));
            };

            let Some(gradient) = resolve_linear_gradient(
                root,
                base_url,
                &reference_id,
                object_path,
                transform,
                opacity,
            )?
            else {
                return Ok((fallback, None));
            };

            Ok((None, Some(Paint::LinearGradient(gradient))))
        }
    }
}

pub(super) fn parse_opacity(value: &str) -> Result<f32, SvgError> {
    let values = parse_number_list(value)?;

    if values.len() != 1 || !(0.0..=1.0).contains(&values[0]) {
        return Err(SvgError::new("opacity must be a number from zero to one"));
    }

    Ok(values[0])
}

pub(super) fn apply_opacity(mut color: Color, opacity: f32) -> Color {
    color.alpha = (color.alpha as f32 * opacity).round() as u8;
    color
}

pub(super) fn parse_paint(value: &str) -> Result<Option<SvgPaintReference>, SvgError> {
    let value = value.trim();

    if value
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("url("))
    {
        let after_url_start = &value[4..];
        let close = after_url_start
            .find(')')
            .ok_or_else(|| SvgError::new("unterminated paint server URL"))?;

        let reference = after_url_start[..close]
            .trim()
            .trim_matches(['"', '\''])
            .to_owned();

        let fallback = after_url_start[close + 1..].trim();

        return Ok(Some(SvgPaintReference::Url {
            reference,
            fallback: if fallback.is_empty() {
                None
            } else {
                parse_solid_color(fallback)?
            },
        }));
    }

    Ok(parse_solid_color(value)?.map(SvgPaintReference::Solid))
}

pub(super) fn parse_solid_color(value: &str) -> Result<Option<Color>, SvgError> {
    let value = value.trim().to_ascii_lowercase();

    if value == "none" {
        return Ok(None);
    }

    let color = match value.as_str() {
        "black" => Color::BLACK,
        "white" => Color::WHITE,
        "red" => Color::rgba(255, 0, 0, 255),
        "green" => Color::rgba(0, 128, 0, 255),
        "blue" => Color::rgba(0, 0, 255, 255),
        "transparent" => Color::TRANSPARENT,
        _ if value.starts_with('#') => parse_hex_color(&value)?,
        _ => return Err(SvgError::new("unsupported fill color")),
    };

    Ok(Some(color))
}

fn parse_hex_color(value: &str) -> Result<Color, SvgError> {
    let hex = &value[1..];
    let bytes = hex.as_bytes();

    let component = |index: usize| {
        hexadecimal_digit(bytes[index])
            .zip(hexadecimal_digit(bytes[index + 1]))
            .map(|(high, low)| high * 16 + low)
    };

    match bytes.len() {
        3 => {
            let red =
                hexadecimal_digit(bytes[0]).ok_or_else(|| SvgError::new("invalid hex color"))?;
            let green =
                hexadecimal_digit(bytes[1]).ok_or_else(|| SvgError::new("invalid hex color"))?;
            let blue =
                hexadecimal_digit(bytes[2]).ok_or_else(|| SvgError::new("invalid hex color"))?;

            Ok(Color::rgba(red * 17, green * 17, blue * 17, 255))
        }

        4 => {
            let red =
                hexadecimal_digit(bytes[0]).ok_or_else(|| SvgError::new("invalid hex color"))?;
            let green =
                hexadecimal_digit(bytes[1]).ok_or_else(|| SvgError::new("invalid hex color"))?;
            let blue =
                hexadecimal_digit(bytes[2]).ok_or_else(|| SvgError::new("invalid hex color"))?;
            let alpha =
                hexadecimal_digit(bytes[3]).ok_or_else(|| SvgError::new("invalid hex color"))?;

            Ok(Color::rgba(red * 17, green * 17, blue * 17, alpha * 17))
        }

        6 => Ok(Color::rgba(
            component(0).ok_or_else(|| SvgError::new("invalid hex color"))?,
            component(2).ok_or_else(|| SvgError::new("invalid hex color"))?,
            component(4).ok_or_else(|| SvgError::new("invalid hex color"))?,
            255,
        )),

        8 => Ok(Color::rgba(
            component(0).ok_or_else(|| SvgError::new("invalid hex color"))?,
            component(2).ok_or_else(|| SvgError::new("invalid hex color"))?,
            component(4).ok_or_else(|| SvgError::new("invalid hex color"))?,
            component(6).ok_or_else(|| SvgError::new("invalid hex color"))?,
        )),

        _ => Err(SvgError::new("unsupported hex color length")),
    }
}

fn hexadecimal_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
