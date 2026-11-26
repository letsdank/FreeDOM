//! Conversion of SVG basic shapes into paths.
//!
//! Specification: SVG 2, chapter 10, "Basic Shapes".
//! <https://www.w3.org/TR/SVG2/shapes.html>

use super::{
    HtmlElement, Path, Point, SvgError, attribute, number_attribute, optional_number_attribute,
    parse_number_list,
};

pub(super) fn parse_line(element: &HtmlElement) -> Result<Option<Path>, SvgError> {
    let x1 = number_attribute(element, "x1", 0.0)?;
    let y1 = number_attribute(element, "y1", 0.0)?;
    let x2 = number_attribute(element, "x2", 0.0)?;
    let y2 = number_attribute(element, "y2", 0.0)?;

    let mut path = Path::new();
    path.move_to(Point::new(x1, y1));
    path.line_to(Point::new(x2, y2));

    Ok(Some(path))
}

pub(super) fn parse_poly_points(
    element: &HtmlElement,
    close_path: bool,
) -> Result<Option<Path>, SvgError> {
    let Some(value) = attribute(element, "points") else {
        return Ok(None);
    };

    let values = parse_number_list(value)?;

    if values.len() % 2 != 0 {
        return Err(SvgError::new(
            "polygon and polyline points must contain coordinate pairs",
        ));
    }

    if values.len() < 4 {
        return Ok(None);
    }

    let mut pairs = values.chunks_exact(2);
    let first = pairs.next().unwrap();

    let mut path = Path::new();
    path.move_to(Point::new(first[0], first[1]));

    for pair in pairs {
        path.line_to(Point::new(pair[0], pair[1]));
    }

    if close_path {
        path.close();
    }

    Ok(Some(path))
}

pub(super) fn parse_circle(element: &HtmlElement) -> Result<Option<Path>, SvgError> {
    let center_x = number_attribute(element, "cx", 0.0)?;
    let center_y = number_attribute(element, "cy", 0.0)?;
    let radius = number_attribute(element, "r", 0.0)?;

    ellipse_path(center_x, center_y, radius, radius)
}

pub(super) fn parse_ellipse(element: &HtmlElement) -> Result<Option<Path>, SvgError> {
    let center_x = number_attribute(element, "cx", 0.0)?;
    let center_y = number_attribute(element, "cy", 0.0)?;
    let radius_x = number_attribute(element, "rx", 0.0)?;
    let radius_y = number_attribute(element, "ry", 0.0)?;

    ellipse_path(center_x, center_y, radius_x, radius_y)
}

fn ellipse_path(
    center_x: f32,
    center_y: f32,
    radius_x: f32,
    radius_y: f32,
) -> Result<Option<Path>, SvgError> {
    if radius_x < 0.0 || radius_y < 0.0 {
        return Err(SvgError::new("ellipse radii must not be negative"));
    }

    if radius_x == 0.0 || radius_y == 0.0 {
        return Ok(None);
    }

    let left = center_x - radius_x;
    let right = center_x + radius_x;
    let top = center_y - radius_y;
    let bottom = center_y + radius_y;

    if !left.is_finite() || !right.is_finite() || !top.is_finite() || !bottom.is_finite() {
        return Err(SvgError::new("ellipse geometry is not finite"));
    }

    const KAPPA: f32 = 0.552_284_8;

    let control_x = radius_x * KAPPA;
    let control_y = radius_y * KAPPA;

    let mut path = Path::new();

    path.move_to(Point::new(right, center_y));

    path.cubic_to(
        Point::new(right, center_y + control_y),
        Point::new(center_x + control_x, bottom),
        Point::new(center_x, bottom),
    );

    path.cubic_to(
        Point::new(center_x - control_x, bottom),
        Point::new(left, center_y + control_y),
        Point::new(left, center_y),
    );

    path.cubic_to(
        Point::new(left, center_y - control_y),
        Point::new(center_x - control_x, top),
        Point::new(center_x, top),
    );

    path.cubic_to(
        Point::new(center_x + control_x, top),
        Point::new(right, center_y - control_y),
        Point::new(right, center_y),
    );

    path.close();

    Ok(Some(path))
}

pub(super) fn parse_rect(element: &HtmlElement) -> Result<Option<Path>, SvgError> {
    let x = number_attribute(element, "x", 0.0)?;
    let y = number_attribute(element, "y", 0.0)?;
    let width = number_attribute(element, "width", 0.0)?;
    let height = number_attribute(element, "height", 0.0)?;

    if width < 0.0 || height < 0.0 {
        return Err(SvgError::new(
            "rectangle width and height must not be negative",
        ));
    }

    if width == 0.0 || height == 0.0 {
        return Ok(None);
    }

    let requested_radius_x = optional_number_attribute(element, "rx")?;
    let requested_radius_y = optional_number_attribute(element, "ry")?;

    let (radius_x, radius_y) = match (requested_radius_x, requested_radius_y) {
        (None, None) => (0.0, 0.0),
        (Some(radius_x), None) => (radius_x, radius_x),
        (None, Some(radius_y)) => (radius_y, radius_y),
        (Some(radius_x), Some(radius_y)) => (radius_x, radius_y),
    };

    if radius_x < 0.0 || radius_y < 0.0 {
        return Err(SvgError::new("rectangle corner radii must not be negative"));
    }

    let radius_x = radius_x.min(width * 0.5);
    let radius_y = radius_y.min(height * 0.5);

    let right = x + width;
    let bottom = y + height;

    if !right.is_finite() || !bottom.is_finite() {
        return Err(SvgError::new("rectangle geometry is not finite"));
    }

    let mut path = Path::new();

    if radius_x == 0.0 || radius_y == 0.0 {
        path.move_to(Point::new(x, y));
        path.line_to(Point::new(right, y));
        path.line_to(Point::new(right, bottom));
        path.line_to(Point::new(x, bottom));
        path.close();

        return Ok(Some(path));
    }

    const KAPPA: f32 = 0.552_284_8;

    let control_x = radius_x * KAPPA;
    let control_y = radius_y * KAPPA;

    path.move_to(Point::new(x + radius_x, y));
    path.line_to(Point::new(right - radius_x, y));
    path.cubic_to(
        Point::new(right - radius_x + control_x, y),
        Point::new(right, y + radius_y - control_y),
        Point::new(right, y + radius_y),
    );

    path.line_to(Point::new(right, bottom - radius_y));
    path.cubic_to(
        Point::new(right, bottom - radius_y + control_y),
        Point::new(right - radius_x + control_x, bottom),
        Point::new(right - radius_x, bottom),
    );

    path.line_to(Point::new(x + radius_x, bottom));
    path.cubic_to(
        Point::new(x + radius_x - control_x, bottom),
        Point::new(x, bottom - radius_y + control_y),
        Point::new(x, bottom - radius_y),
    );

    path.line_to(Point::new(x, y + radius_y));
    path.cubic_to(
        Point::new(x, y + radius_y - control_y),
        Point::new(x + radius_x - control_x, y),
        Point::new(x + radius_x, y),
    );

    path.close();

    Ok(Some(path))
}
