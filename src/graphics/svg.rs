use std::{error::Error, fmt};

use crate::{
    graphics::{
        flatten::{FlattenError, flatten_path},
        geometry::{Path, Point, Transform},
        raster::{Color, FillRule, Surface, fill_path},
        svg_path::parse_path_data,
        svg_transform::parse_transform_list,
    },
    html::{Document as HtmlDocument, Element as HtmlElement, Node as HtmlNode},
};

#[derive(Clone, Debug, PartialEq)]
pub struct SvgDocument {
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub view_box: Option<ViewBox>,
    pub shapes: Vec<SvgShape>,
}

impl SvgDocument {
    pub fn render(
        &self,
        width: usize,
        height: usize,
        background: Color,
    ) -> Result<Surface, FlattenError> {
        let mut surface = Surface::new(width, height, background);

        let viewport_transform = self
            .view_box
            .map(|view_box| view_box_transform(view_box, width, height))
            .unwrap_or(Transform::IDENTITY);

        for shape in &self.shapes {
            let transformed_path = shape.path.transformed(viewport_transform);
            let flattened = flatten_path(&transformed_path, 0.25)?;

            fill_path(&mut surface, &flattened, shape.fill_rule, shape.fill);
        }

        Ok(surface)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewBox {
    pub min_x: f32,
    pub min_y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SvgShape {
    pub path: Path,
    pub fill: Color,
    pub fill_rule: FillRule,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SvgError {
    pub message: String,
}

impl SvgError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SvgError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid SVG: {}", self.message)
    }
}

impl Error for SvgError {}

pub fn parse_svg(input: &str) -> Result<SvgDocument, SvgError> {
    let html = HtmlDocument::parse(input);
    let root = html
        .first_element("svg")
        .ok_or_else(|| SvgError::new("missing <svg> root element"))?;

    parse_svg_element(root)
}

pub fn parse_svg_element(root: &HtmlElement) -> Result<SvgDocument, SvgError> {
    if !root.name.eq_ignore_ascii_case("svg") {
        return Err(SvgError::new("expected an <svg> element"));
    }

    let width = match attribute(root, "width") {
        Some(value) => Some(parse_svg_length(value)?),
        None => None,
    };

    let height = match attribute(root, "height") {
        Some(value) => Some(parse_svg_length(value)?),
        None => None,
    };

    let view_box = match attribute(root, "viewbox") {
        Some(value) => Some(parse_view_box(value)?),
        None => None,
    };

    let mut shapes = Vec::new();
    collect_shapes(root, SvgStyle::default(), Transform::IDENTITY, &mut shapes)?;

    Ok(SvgDocument {
        width,
        height,
        view_box,
        shapes,
    })
}

#[derive(Clone, Copy)]
struct SvgStyle {
    fill: Option<Color>,
    fill_rule: FillRule,
}

impl Default for SvgStyle {
    fn default() -> Self {
        Self {
            fill: Some(Color::BLACK),
            fill_rule: FillRule::NonZero,
        }
    }
}

fn collect_shapes(
    element: &HtmlElement,
    inherited_style: SvgStyle,
    parent_transform: Transform,
    shapes: &mut Vec<SvgShape>,
) -> Result<(), SvgError> {
    if matches!(
        element.name.as_str(),
        "defs" | "clippath" | "mask" | "lineargradient" | "radialgradient" | "pattern" | "symbol"
    ) {
        return Ok(());
    }

    let style = resolve_style(element, inherited_style)?;

    let local_transform = match attribute(element, "transform") {
        Some(value) => {
            parse_transform_list(value).map_err(|error| SvgError::new(error.to_string()))?
        }
        None => Transform::IDENTITY,
    };

    let transform = local_transform.then(parent_transform);

    let path = match element.name.as_str() {
        "path" => attribute(element, "d")
            .map(|data| parse_path_data(data).map_err(|error| SvgError::new(error.to_string())))
            .transpose()?,

        "rect" => parse_rect(element)?,
        "circle" => parse_circle(element)?,
        "ellipse" => parse_ellipse(element)?,

        _ => None,
    };

    if let (Some(path), Some(fill)) = (path, style.fill) {
        shapes.push(SvgShape {
            path: path.transformed(transform),
            fill,
            fill_rule: style.fill_rule,
        });
    }

    for child in &element.children {
        if let HtmlNode::Element(child) = child {
            collect_shapes(child, style, transform, shapes)?;
        }
    }

    Ok(())
}

fn resolve_style(element: &HtmlElement, mut style: SvgStyle) -> Result<SvgStyle, SvgError> {
    if let Some(value) = attribute(element, "fill") {
        style.fill = parse_fill(value)?;
    }

    if let Some(value) = attribute(element, "fill-rule") {
        style.fill_rule = parse_fill_rule(value)?;
    }

    if let Some(value) = attribute(element, "style") {
        for declaration in value.split(';') {
            let Some((property, value)) = declaration.split_once(':') else {
                continue;
            };

            match property.trim().to_ascii_lowercase().as_str() {
                "fill" => style.fill = parse_fill(value)?,
                "fill-rule" => style.fill_rule = parse_fill_rule(value)?,
                _ => {}
            }
        }
    }

    Ok(style)
}

fn attribute<'a>(element: &'a HtmlElement, name: &str) -> Option<&'a str> {
    element
        .attributes
        .iter()
        .find(|attribute| attribute.name.eq_ignore_ascii_case(name))
        .map(|attribute| attribute.value.as_str())
}

fn parse_circle(element: &HtmlElement) -> Result<Option<Path>, SvgError> {
    let center_x = number_attribute(element, "cx", 0.0)?;
    let center_y = number_attribute(element, "cy", 0.0)?;
    let radius = number_attribute(element, "r", 0.0)?;

    ellipse_path(center_x, center_y, radius, radius)
}

fn parse_ellipse(element: &HtmlElement) -> Result<Option<Path>, SvgError> {
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

fn parse_rect(element: &HtmlElement) -> Result<Option<Path>, SvgError> {
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
        Point::new(x + radius_x - control_y, y),
        Point::new(x + radius_x, y),
    );

    path.close();

    Ok(Some(path))
}

fn number_attribute(element: &HtmlElement, name: &str, default: f32) -> Result<f32, SvgError> {
    let Some(value) = attribute(element, name) else {
        return Ok(default);
    };

    let values = parse_number_list(value)?;

    if values.len() != 1 {
        return Err(SvgError::new("expected one numeric SVG attribute value"));
    }

    Ok(values[0])
}

fn optional_number_attribute(element: &HtmlElement, name: &str) -> Result<Option<f32>, SvgError> {
    if attribute(element, name).is_none() {
        return Ok(None);
    }

    Ok(Some(number_attribute(element, name, 0.0)?))
}

fn parse_fill(value: &str) -> Result<Option<Color>, SvgError> {
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

fn parse_fill_rule(value: &str) -> Result<FillRule, SvgError> {
    if value.trim().eq_ignore_ascii_case("nonzero") {
        Ok(FillRule::NonZero)
    } else if value.trim().eq_ignore_ascii_case("evenodd") {
        Ok(FillRule::EvenOdd)
    } else {
        Err(SvgError::new("unsupported fill-rule"))
    }
}

fn parse_svg_length(value: &str) -> Result<f32, SvgError> {
    let value = value.trim();
    let value = value.strip_suffix("px").unwrap_or(value);
    let values = parse_number_list(value)?;

    if values.len() != 1 || values[0] < 0.0 {
        return Err(SvgError::new("invalid SVG length"));
    }

    Ok(values[0])
}

fn parse_view_box(value: &str) -> Result<ViewBox, SvgError> {
    let values = parse_number_list(value)?;

    let [min_x, min_y, width, height] = values.as_slice() else {
        return Err(SvgError::new("viewBox must contain four numbers"));
    };

    if *width <= 0.0 || *height <= 0.0 {
        return Err(SvgError::new("viewBox width and height must be positive"));
    }

    Ok(ViewBox {
        min_x: *min_x,
        min_y: *min_y,
        width: *width,
        height: *height,
    })
}

fn parse_number_list(input: &str) -> Result<Vec<f32>, SvgError> {
    let mut values = Vec::new();
    let bytes = input.as_bytes();
    let mut cursor = 0;

    while cursor < bytes.len() {
        while cursor < bytes.len() && (bytes[cursor].is_ascii_whitespace() || bytes[cursor] == b',')
        {
            cursor += 1;
        }

        if cursor == bytes.len() {
            break;
        }

        let start = cursor;

        if matches!(bytes.get(cursor), Some(b'+' | b'-')) {
            cursor += 1;
        }

        let integer_start = cursor;
        while matches!(bytes.get(cursor),Some(byte)if byte.is_ascii_digit()) {
            cursor += 1;
        }
        let has_integer = cursor != integer_start;

        if bytes.get(cursor) == Some(&b'.') {
            cursor += 1;
        }

        let fraction_start = cursor;
        while matches!(bytes.get(cursor),Some(byte)if byte.is_ascii_digit()) {
            cursor += 1;
        }
        let has_fraction = cursor != fraction_start;

        if !has_integer && !has_fraction {
            return Err(SvgError::new("expected SVG number"));
        }

        if matches!(bytes.get(cursor), Some(b'e' | b'E')) {
            cursor += 1;

            if matches!(bytes.get(cursor), Some(b'+' | b'-')) {
                cursor += 1;
            }

            let exponent_start = cursor;
            while matches!(bytes.get(cursor),Some(byte)if byte.is_ascii_digit()) {
                cursor += 1;
            }

            if cursor == exponent_start {
                return Err(SvgError::new("expected exponent digits"));
            }
        }

        let value = input[start..cursor]
            .parse::<f32>()
            .map_err(|_| SvgError::new("invalid SVG number"))?;

        if !value.is_finite() {
            return Err(SvgError::new("SVG number must be finite"));
        }

        values.push(value);
    }

    Ok(values)
}

fn view_box_transform(view_box: ViewBox, width: usize, height: usize) -> Transform {
    if width == 0 || height == 0 {
        return Transform::scale(0.0, 0.0);
    }

    let scale = (width as f32 / view_box.width).min(height as f32 / view_box.height);
    let offset_x = (width as f32 - view_box.width * scale) * 0.5;
    let offset_y = (height as f32 - view_box.height * scale) * 0.5;

    Transform::translate(-view_box.min_x, -view_box.min_y)
        .then(Transform::scale(scale, scale))
        .then(Transform::translate(offset_x, offset_y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::geometry::PathCommand;

    #[test]
    fn parses_path_fill_and_group_transform() {
        let document = parse_svg(
            r##"
                <svg>
                    <g fill="#00ff00" transform="translate(2 3)">
                        <path d="M 0 0 L 4 0 L 4 4 Z"/>
                    </g>
                </svg>
            "##,
        )
            .unwrap();

        assert_eq!(document.shapes.len(), 1);
        assert_eq!(document.shapes[0].fill, Color::rgba(0, 255, 0, 255));

        assert_eq!(
            document.shapes[0].path.commands()[0],
            PathCommand::MoveTo(Point::new(2.0, 3.0))
        );
    }

    #[test]
    fn renders_svg_paths_using_viewbox_scaling() {
        let document = parse_svg(
            r##"
                <svg viewBox="0 0 10 10">
                    <path fill="#ff0000" d="M 0 0 H 10 V 10 H 0 Z"/>
                </svg>
            "##,
        )
            .unwrap();

        let surface = document.render(20, 20, Color::WHITE).unwrap();

        assert_eq!(surface.pixel(0, 0), Some(Color::rgba(255, 0, 0, 255)));
        assert_eq!(surface.pixel(10, 10), Some(Color::rgba(255, 0, 0, 255)));
        assert_eq!(surface.pixel(19, 19), Some(Color::rgba(255, 0, 0, 255)));
    }

    #[test]
    fn inline_style_overrides_presentation_fill() {
        let document = parse_svg(
            r##"
                <svg>
                    <path
                        fill="#ff0000"
                        style="fill: #0000ff"
                        d="M 0 0 H 10 V 10 Z"
                    />
                </svg>
            "##,
        )
            .unwrap();

        assert_eq!(document.shapes[0].fill, Color::rgba(0, 0, 255, 255));
    }

    #[test]
    fn parses_svg_rectangles() {
        let document = parse_svg(
            r##"
                <svg>
                    <rect x="2" y="3" width="10" height="20" fill="#123456"/>
                </svg>
            "##,
        )
            .unwrap();

        assert_eq!(document.shapes.len(), 1);
        assert_eq!(document.shapes[0].fill, Color::rgba(0x12, 0x34, 0x56, 255));

        assert_eq!(
            document.shapes[0].path.commands(),
            &[
                PathCommand::MoveTo(Point::new(2.0, 3.0)),
                PathCommand::LineTo(Point::new(12.0, 3.0)),
                PathCommand::LineTo(Point::new(12.0, 23.0)),
                PathCommand::LineTo(Point::new(2.0, 23.0)),
                PathCommand::Close,
            ]
        );
    }

    #[test]
    fn converts_rounded_rectangle_to_cubic_curves() {
        let document = parse_svg(r#"<svg><rect width="20" height="10" rx="3"/></svg>"#).unwrap();

        let commands = document.shapes[0].path.commands();

        assert_eq!(commands.len(), 10);
        assert!(matches!(commands[2], PathCommand::CubicTo { .. }));
        assert!(matches!(commands[4], PathCommand::CubicTo { .. }));
        assert!(matches!(commands[6], PathCommand::CubicTo { .. }));
        assert!(matches!(commands[8], PathCommand::CubicTo { .. }));
        assert_eq!(commands[9], PathCommand::Close);
    }

    #[test]
    fn converts_circles_and_ellipses_to_cubic_paths() {
        let document = parse_svg(
            r#"
                <svg>
                    <circle cx="10" cy="20" r="5"/>
                    <ellipse cx="30" cy="40" rx="8" ry="3"/>
                </svg>
            "#,
        )
            .unwrap();

        assert_eq!(document.shapes.len(), 2);

        let circle = document.shapes[0].path.commands();
        assert_eq!(circle.len(), 6);
        assert_eq!(circle[0], PathCommand::MoveTo(Point::new(15.0, 20.0)));
        assert!(matches!(circle[1], PathCommand::CubicTo { .. }));
        assert_eq!(circle[5], PathCommand::Close);

        let ellipse = document.shapes[1].path.commands();
        assert_eq!(ellipse[0], PathCommand::MoveTo(Point::new(38.0, 40.0)));
        assert_eq!(ellipse[5], PathCommand::Close);
    }

    #[test]
    fn ignores_zero_radius_ellipses() {
        let document = parse_svg(r#"<svg><circle cx="10" cy="10" r="0"/></svg>"#).unwrap();

        assert!(document.shapes.is_empty());
    }
}
