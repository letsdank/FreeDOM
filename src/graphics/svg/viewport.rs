//! SVG viewport dimensions and viewBox transformations.
//!
//! Specification: <https://www.w3.org/TR/SVG2/coords.html>

use super::{
    MeetOrSlice, ParsedViewBox, Path, Point, PreserveAspectRatio, SvgError, Transform, ViewBox,
    ViewBoxAlignment, parse_number_list,
};

pub(super) fn parse_svg_viewport_dimension(value: &str) -> Result<Option<f32>, SvgError> {
    let value = value.trim();

    if let Some(percent) = value.strip_suffix('%') {
        let values = parse_number_list(percent)?;

        if values.len() != 1 || !values[0].is_finite() || values[0] < 0.0 {
            return Err(SvgError::new("invalid SVG viewport dimension"));
        }

        return Ok(None);
    }

    parse_svg_length(value).map(Some)
}

pub(super) fn parse_svg_length(value: &str) -> Result<f32, SvgError> {
    let value = value.trim();
    let value = value.strip_suffix("px").unwrap_or(value);
    let values = parse_number_list(value)?;

    if values.len() != 1 || values[0] < 0.0 {
        return Err(SvgError::new("invalid SVG length"));
    }

    Ok(values[0])
}

pub(super) fn parse_view_box(value: &str) -> Result<ParsedViewBox, SvgError> {
    let values = parse_number_list(value)?;

    let [min_x, min_y, width, height] = values.as_slice() else {
        return Err(SvgError::new("viewBox must contain four numbers"));
    };

    if *width < 0.0 || *height < 0.0 {
        return Err(SvgError::new("viewBox width and height cannot be negative"));
    }

    if *width == 0.0 || *height == 0.0 {
        return Ok(ParsedViewBox::DisablesRendering);
    }

    Ok(ParsedViewBox::Valid(ViewBox {
        min_x: *min_x,
        min_y: *min_y,
        width: *width,
        height: *height,
    }))
}

pub(super) fn parse_preserve_aspect_ratio(value: &str) -> Result<PreserveAspectRatio, SvgError> {
    let mut tokens = value.split_ascii_whitespace();

    let mut alignment = tokens
        .next()
        .ok_or_else(|| SvgError::new("preserveAspectRatio cannot be empty"))?;

    if alignment.eq_ignore_ascii_case("defer") {
        alignment = tokens
            .next()
            .ok_or_else(|| SvgError::new("expected preserveAspectRatio alignment after defer"))?;
    }

    let alignment = alignment.to_ascii_lowercase();

    if alignment == "none" {
        if let Some(mode) = tokens.next() {
            if !mode.eq_ignore_ascii_case("meet") && !mode.eq_ignore_ascii_case("slice") {
                return Err(SvgError::new("invalid preserveAspectRatio mode"));
            }
        }

        if tokens.next().is_some() {
            return Err(SvgError::new("too many preserveAspectRatio values"));
        }

        return Ok(PreserveAspectRatio::None);
    }

    let (horizontal, vertical) = match alignment.as_str() {
        "xminymin" => (ViewBoxAlignment::Min, ViewBoxAlignment::Min),
        "xmidymin" => (ViewBoxAlignment::Mid, ViewBoxAlignment::Min),
        "xmaxymin" => (ViewBoxAlignment::Max, ViewBoxAlignment::Min),
        "xminymid" => (ViewBoxAlignment::Min, ViewBoxAlignment::Mid),
        "xmidymid" => (ViewBoxAlignment::Mid, ViewBoxAlignment::Mid),
        "xmaxymid" => (ViewBoxAlignment::Max, ViewBoxAlignment::Mid),
        "xminymax" => (ViewBoxAlignment::Min, ViewBoxAlignment::Max),
        "xmidymax" => (ViewBoxAlignment::Mid, ViewBoxAlignment::Max),
        "xmaxymax" => (ViewBoxAlignment::Max, ViewBoxAlignment::Max),
        _ => return Err(SvgError::new("invalid preserveAspectRatio alignment")),
    };

    let meet_or_slice = match tokens.next() {
        None => MeetOrSlice::Meet,
        Some(value) if value.eq_ignore_ascii_case("meet") => MeetOrSlice::Meet,
        Some(value) if value.eq_ignore_ascii_case("slice") => MeetOrSlice::Slice,
        Some(_) => return Err(SvgError::new("invalid preserveAspectRatio mode")),
    };

    if tokens.next().is_some() {
        return Err(SvgError::new("too many preserveAspectRatio values"));
    }

    Ok(PreserveAspectRatio::Align {
        horizontal,
        vertical,
        meet_or_slice,
    })
}

pub(super) fn view_box_transform(
    view_box: ViewBox,
    preserve_aspect_ratio: PreserveAspectRatio,
    width: f32,
    height: f32,
) -> Transform {
    if width == 0.0 || height == 0.0 {
        return Transform::scale(0.0, 0.0);
    }

    let scale_x = width as f32 / view_box.width;
    let scale_y = height as f32 / view_box.height;

    let (scale_x, scale_y, offset_x, offset_y) = match preserve_aspect_ratio {
        PreserveAspectRatio::None => (scale_x, scale_y, 0.0, 0.0),

        PreserveAspectRatio::Align {
            horizontal,
            vertical,
            meet_or_slice,
        } => {
            let scale = match meet_or_slice {
                MeetOrSlice::Meet => scale_x.min(scale_y),
                MeetOrSlice::Slice => scale_x.max(scale_y),
            };

            let offset_x = alignment_offset(horizontal, width as f32 - view_box.width * scale);
            let offset_y = alignment_offset(vertical, height as f32 - view_box.height * scale);

            (scale, scale, offset_x, offset_y)
        }
    };

    Transform::translate(-view_box.min_x, -view_box.min_y)
        .then(Transform::scale(scale_x, scale_y))
        .then(Transform::translate(offset_x, offset_y))
}

fn alignment_offset(alignment: ViewBoxAlignment, remaining_space: f32) -> f32 {
    match alignment {
        ViewBoxAlignment::Min => 0.0,
        ViewBoxAlignment::Mid => remaining_space * 0.5,
        ViewBoxAlignment::Max => remaining_space,
    }
}

pub(super) fn viewport_path(width: f32, height: f32) -> Path {
    let mut path = Path::new();
    path.move_to(Point::new(0.0, 0.0));
    path.line_to(Point::new(width, 0.0));
    path.line_to(Point::new(width, height));
    path.line_to(Point::new(0.0, height));
    path.close();
    path
}
