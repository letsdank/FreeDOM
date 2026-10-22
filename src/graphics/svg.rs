use crate::{
    graphics::{
        flatten::{FlattenError, flatten_path},
        geometry::{Path, Point, Transform},
        raster::{Color, FillRule, StrokeStyle, Surface, fill_path, stroke_path},
        svg_path::parse_path_data,
        svg_transform::parse_transform_list,
    },
    html::{Document as HtmlDocument, Element as HtmlElement, Node as HtmlNode},
};
use std::{error::Error, fmt};

#[derive(Clone, Debug, PartialEq)]
pub struct SvgDocument {
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub view_box: Option<ViewBox>,
    pub shapes: Vec<SvgShape>,
    view_box_disables_rendering: bool,
    preserve_aspect_ratio: PreserveAspectRatio,
}

impl SvgDocument {
    pub fn render(
        &self,
        width: usize,
        height: usize,
        background: Color,
    ) -> Result<Surface, FlattenError> {
        let mut surface = Surface::new(width, height, background);

        if self.view_box_disables_rendering {
            return Ok(surface);
        }

        let viewport_transform = self
            .view_box
            .map(|view_box| view_box_transform(view_box, self.preserve_aspect_ratio, width, height))
            .unwrap_or(Transform::IDENTITY);

        let viewport_stroke_scale = stroke_scale(viewport_transform);

        for shape in &self.shapes {
            let transformed_path = shape.path.transformed(viewport_transform);
            let flattened = flatten_path(&transformed_path, 0.25)?;

            if let Some(fill) = shape.fill {
                fill_path(&mut surface, &flattened, shape.fill_rule, fill);
            }

            if let Some(stroke) = shape.stroke {
                stroke_path(
                    &mut surface,
                    &flattened,
                    StrokeStyle {
                        width: shape.stroke_width * viewport_stroke_scale,
                    },
                    stroke,
                );
            }
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

enum ParsedViewBox {
    Valid(ViewBox),
    DisablesRendering,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreserveAspectRatio {
    None,
    Align {
        horizontal: ViewBoxAlignment,
        vertical: ViewBoxAlignment,
        meet_or_slice: MeetOrSlice,
    },
}

impl Default for PreserveAspectRatio {
    fn default() -> Self {
        Self::Align {
            horizontal: ViewBoxAlignment::Mid,
            vertical: ViewBoxAlignment::Mid,
            meet_or_slice: MeetOrSlice::Meet,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ViewBoxAlignment {
    Min,
    Mid,
    Max,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MeetOrSlice {
    Meet,
    Slice,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SvgShape {
    pub path: Path,
    pub fill: Option<Color>,
    pub fill_rule: FillRule,
    pub stroke: Option<Color>,
    pub stroke_width: f32,
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

    let (view_box, view_box_disables_rendering) = match attribute(root, "viewbox") {
        Some(value) => match parse_view_box(value)? {
            ParsedViewBox::Valid(view_box) => (Some(view_box), false),
            ParsedViewBox::DisablesRendering => (None, true),
        },
        None => (None, false),
    };

    let preserve_aspect_ratio = match attribute(root, "preserveaspectratio") {
        Some(value) => parse_preserve_aspect_ratio(value)?,
        None => PreserveAspectRatio::default(),
    };

    let mut shapes = Vec::new();
    let mut reference_stack = Vec::new();

    collect_shapes(
        root,
        root,
        SvgStyle::default(),
        Transform::IDENTITY,
        &mut shapes,
        &mut reference_stack,
    )?;

    Ok(SvgDocument {
        width,
        height,
        view_box,
        view_box_disables_rendering,
        preserve_aspect_ratio,
        shapes,
    })
}

#[derive(Clone, Copy)]
struct SvgStyle {
    fill: Option<Color>,
    fill_rule: FillRule,
    stroke: Option<Color>,
    stroke_width: f32,
}

impl Default for SvgStyle {
    fn default() -> Self {
        Self {
            fill: Some(Color::BLACK),
            fill_rule: FillRule::NonZero,
            stroke: None,
            stroke_width: 1.0,
        }
    }
}

fn collect_shapes(
    root: &HtmlElement,
    element: &HtmlElement,
    inherited_style: SvgStyle,
    parent_transform: Transform,
    shapes: &mut Vec<SvgShape>,
    reference_stack: &mut Vec<String>,
) -> Result<(), SvgError> {
    // FreeDOM does not support SVG extensions yet. Under SVG conditional processing,
    // an element with requiredExtensions is not rendered when the required
    // extension is unsupported.
    if attribute(element, "requiredextensions").is_some() {
        return Ok(());
    }

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

    if element.name.eq_ignore_ascii_case("use") {
        let Some(reference_id) = use_reference_id(element) else {
            return Ok(());
        };

        if reference_stack.iter().any(|id| id == reference_id) {
            return Err(SvgError::new("circular SVG use reference"));
        }

        let referenced_element = find_element_by_id(root, reference_id)
            .ok_or_else(|| SvgError::new("SVG use target was not found"))?;

        let x = number_attribute(element, "x", 0.0)?;
        let y = number_attribute(element, "y", 0.0)?;
        let reference_transform = Transform::translate(x, y).then(transform);

        reference_stack.push(reference_id.to_owned());

        let result = collect_shapes(
            root,
            referenced_element,
            style,
            reference_transform,
            shapes,
            reference_stack,
        );

        reference_stack.pop();

        return result;
    }

    let path = match element.name.as_str() {
        "path" => attribute(element, "d")
            .map(|data| parse_path_data(data).map_err(|error| SvgError::new(error.to_string())))
            .transpose()?,

        "rect" => parse_rect(element)?,
        "circle" => parse_circle(element)?,
        "ellipse" => parse_ellipse(element)?,
        "polygon" => parse_poly_points(element, true)?,
        "polyline" => parse_poly_points(element, false)?,
        "line" => parse_line(element)?,

        _ => None,
    };

    if let Some(path) = path {
        if style.fill.is_some() || style.stroke.is_some() {
            shapes.push(SvgShape {
                path: path.transformed(transform),
                fill: style.fill,
                fill_rule: style.fill_rule,
                stroke: style.stroke,
                stroke_width: style.stroke_width,
            });
        }
    }

    for child in &element.children {
        if let HtmlNode::Element(child) = child {
            collect_shapes(root, child, style, transform, shapes, reference_stack)?;
        }
    }

    Ok(())
}

fn resolve_style(element: &HtmlElement, mut style: SvgStyle) -> Result<SvgStyle, SvgError> {
    if let Some(value) = attribute(element, "fill") {
        style.fill = parse_fill(value)?;
    }

    if let Some(value) = attribute(element, "stroke") {
        style.stroke = parse_fill(value)?;
    }

    if let Some(value) = attribute(element, "stroke-width") {
        style.stroke_width = parse_stroke_width(value)?;
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
                "stroke" => style.stroke = parse_fill(value)?,
                "stroke-width" => style.stroke_width = parse_stroke_width(value)?,
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

fn use_reference_id(element: &HtmlElement) -> Option<&str> {
    attribute(element, "href")
        .or_else(|| attribute(element, "xlink:href"))
        .and_then(|value| value.strip_prefix('#'))
        .filter(|id| !id.is_empty())
}

fn find_element_by_id<'a>(element: &'a HtmlElement, id: &str) -> Option<&'a HtmlElement> {
    if attribute(element, "id") == Some(id) {
        return Some(element);
    }

    element.children.iter().find_map(|child| {
        let HtmlNode::Element(child) = child else {
            return None;
        };

        find_element_by_id(child, id)
    })
}

fn parse_line(element: &HtmlElement) -> Result<Option<Path>, SvgError> {
    let x1 = number_attribute(element, "x1", 0.0)?;
    let y1 = number_attribute(element, "y1", 0.0)?;
    let x2 = number_attribute(element, "x2", 0.0)?;
    let y2 = number_attribute(element, "y2", 0.0)?;

    let mut path = Path::new();
    path.move_to(Point::new(x1, y1));
    path.line_to(Point::new(x2, y2));

    Ok(Some(path))
}

fn parse_poly_points(element: &HtmlElement, close_path: bool) -> Result<Option<Path>, SvgError> {
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

    if let Some(after_url_start) = value.strip_prefix("url(") {
        let close = after_url_start
            .find(')')
            .ok_or_else(|| SvgError::new("unterminated paint server URL"))?;

        let fallback = after_url_start[close + 1..].trim();

        // FreeDOM does not implement SVG extensions yet. An element with
        // requiredExtensions is not rendered until all requested extensions
        // are supported.
        return if fallback.is_empty() {
            Ok(None)
        } else {
            parse_fill(fallback)
        };
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

fn parse_stroke_width(value: &str) -> Result<f32, SvgError> {
    let values = parse_number_list(value)?;

    if values.len() != 1 || values[0] < 0.0 {
        return Err(SvgError::new("invalid stroke-width"));
    }

    Ok(values[0])
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

fn parse_view_box(value: &str) -> Result<ParsedViewBox, SvgError> {
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

fn parse_preserve_aspect_ratio(value: &str) -> Result<PreserveAspectRatio, SvgError> {
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

fn stroke_scale(transform: Transform) -> f32 {
    let x_axis_scale = (transform.a * transform.a + transform.b * transform.b).sqrt();
    let y_axis_scale = (transform.c * transform.c + transform.d * transform.d).sqrt();

    (x_axis_scale + y_axis_scale) * 0.5
}

fn view_box_transform(
    view_box: ViewBox,
    preserve_aspect_ratio: PreserveAspectRatio,
    width: usize,
    height: usize,
) -> Transform {
    if width == 0 || height == 0 {
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
        assert_eq!(document.shapes[0].fill, Some(Color::rgba(0, 255, 0, 255)));

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

        assert_eq!(document.shapes[0].fill, Some(Color::rgba(0, 0, 255, 255)));
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
        assert_eq!(
            document.shapes[0].fill,
            Some(Color::rgba(0x12, 0x34, 0x56, 255))
        );

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

    #[test]
    fn parses_polygon_and_polyline_points() {
        let document = parse_svg(
            r#"
                <svg>
                    <polygon points="1,2 3,4 5,6"/>
                    <polyline points="10 20, 30 40, 50 60"/>
                </svg>
            "#,
        )
            .unwrap();

        assert_eq!(document.shapes.len(), 2);

        assert_eq!(
            document.shapes[0].path.commands(),
            &[
                PathCommand::MoveTo(Point::new(1.0, 2.0)),
                PathCommand::LineTo(Point::new(3.0, 4.0)),
                PathCommand::LineTo(Point::new(5.0, 6.0)),
                PathCommand::Close,
            ]
        );

        assert_eq!(
            document.shapes[1].path.commands(),
            &[
                PathCommand::MoveTo(Point::new(10.0, 20.0)),
                PathCommand::LineTo(Point::new(30.0, 40.0)),
                PathCommand::LineTo(Point::new(50.0, 60.0)),
            ]
        );
    }

    #[test]
    fn rejects_odd_polygon_coordinate_counts() {
        let error = parse_svg(r#"<svg><polygon points="1 2 3"/></svg>"#).unwrap_err();

        assert_eq!(
            error.message,
            "polygon and polyline points must contain coordinate pairs"
        );
    }

    #[test]
    fn renders_stroked_line_elements() {
        let document = parse_svg(
            r##"
                <svg>
                    <line
                        x1="2"
                        y1="4"
                        x2="8"
                        y2="4"
                        fill="none"
                        stroke="#000000"
                        stroke-width="2"
                    />
                </svg>
            "##,
        )
            .unwrap();

        assert_eq!(document.shapes.len(), 1);
        assert_eq!(document.shapes[0].fill, None);
        assert_eq!(document.shapes[0].stroke, Some(Color::BLACK));
        assert_eq!(document.shapes[0].stroke_width, 2.0);

        let surface = document.render(10, 10, Color::TRANSPARENT).unwrap();

        assert_eq!(surface.pixel(4, 4), Some(Color::BLACK));
        assert_eq!(surface.pixel(0, 4), Some(Color::TRANSPARENT));
    }

    #[test]
    fn skips_elements_that_require_unsupported_extensions() {
        let document = parse_svg(
            r##"
                <svg>
                    <rect width="100" height="100" fill="red" requiredExtensions=""/>
                </svg>
            "##,
        )
            .unwrap();

        assert!(document.shapes.is_empty());
    }

    #[test]
    fn zero_sized_viewbox_disables_rendering() {
        let document = parse_svg(
            r##"
                <svg viewBox="0 0 0 10">
                    <rect width="10" height="10" fill="red"/>
                </svg>
            "##,
        )
            .unwrap();

        let surface = document.render(20, 20, Color::WHITE).unwrap();

        assert_eq!(surface.pixel(10, 10), Some(Color::WHITE));
    }

    #[test]
    fn uses_fallback_color_when_paint_server_is_unavailable() {
        let without_fallback =
            parse_svg(r##"<svg><rect width="10" height="10" fill="url(#missing)"/></svg>"##)
                .unwrap();

        assert!(without_fallback.shapes.is_empty());

        let with_fallback =
            parse_svg(r##"<svg><rect width="10" height="10" fill="url(#missing) green"/></svg>"##)
                .unwrap();

        assert_eq!(with_fallback.shapes.len(), 1);
        assert_eq!(
            with_fallback.shapes[0].fill,
            Some(Color::rgba(0, 128, 0, 255))
        );
    }

    #[test]
    fn parses_preserve_aspect_ratio_values() {
        assert_eq!(
            parse_preserve_aspect_ratio("xMaxYMin slice").unwrap(),
            PreserveAspectRatio::Align {
                horizontal: ViewBoxAlignment::Max,
                vertical: ViewBoxAlignment::Min,
                meet_or_slice: MeetOrSlice::Slice,
            }
        );

        assert_eq!(
            parse_preserve_aspect_ratio("none").unwrap(),
            PreserveAspectRatio::None
        );
    }

    #[test]
    fn renders_non_uniform_viewbox_with_preserve_aspect_ratio_none() {
        let document = parse_svg(
            r##"
                <svg viewBox="0 0 10 10" preserveAspectRatio="none">
                    <rect width="10" height="10" fill="red"/>
                </svg>
            "##,
        )
            .unwrap();

        let surface = document.render(20, 10, Color::WHITE).unwrap();

        assert_eq!(surface.pixel(19, 9), Some(Color::rgba(255, 0, 0, 255)));
    }

    #[test]
    fn aligns_meet_viewbox_to_maximum_edges() {
        let transform = view_box_transform(
            ViewBox {
                min_x: 0.0,
                min_y: 0.0,
                width: 10.0,
                height: 10.0,
            },
            PreserveAspectRatio::Align {
                horizontal: ViewBoxAlignment::Max,
                vertical: ViewBoxAlignment::Max,
                meet_or_slice: MeetOrSlice::Meet,
            },
            20,
            10,
        );

        assert_eq!(
            transform.apply_point(Point::new(0.0, 0.0)),
            Point::new(10.0, 0.0)
        );
    }

    #[test]
    fn expands_local_use_references() {
        let document = parse_svg(
            r##"
                <svg>
                    <defs>
                        <rect id="tile" width="4" height="5" fill="green"/>
                    </defs>
                    <use href="#tile" x="2" y="3"/>
                </svg>
            "##,
        )
            .unwrap();

        assert_eq!(document.shapes.len(), 1);
        assert_eq!(
            document.shapes[0].path.commands()[0],
            PathCommand::MoveTo(Point::new(2.0, 3.0))
        );
        assert_eq!(document.shapes[0].fill, Some(Color::rgba(0, 128, 0, 255)));
    }
}
