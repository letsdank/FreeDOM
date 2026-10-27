//! SVG document parsing and rendering.
//!
//! Specification: <https://www.w3.org/TR/SVG2/>

mod clip;
mod gradient;
mod number;
mod paint;
mod render;
mod shapes;
mod stroke;
mod style;
mod viewport;

use self::clip::resolve_clip_paths;
use self::gradient::resolve_linear_gradient;
use self::number::parse_number_list;
use self::paint::{apply_opacity, parse_opacity, parse_paint, parse_solid_color, resolve_paint};
use self::shapes::{parse_circle, parse_ellipse, parse_line, parse_poly_points, parse_rect};
use self::stroke::{
    parse_stroke_dash_array, parse_stroke_dash_offset, parse_stroke_line_cap,
    parse_stroke_line_join, parse_stroke_miter_limit, parse_stroke_width,
};
use self::style::{collect_svg_stylesheets, resolve_style};
use self::viewport::{
    parse_preserve_aspect_ratio, parse_svg_length, parse_svg_viewport_dimension, parse_view_box,
    view_box_transform, viewport_path,
};

use crate::{
    css::{CascadeRule, build_cascade_rules, parse_declaration_list},
    graphics::{
        flatten::FlattenedPath,
        geometry::{Path, Point, Transform},
        raster::{Color, FillRule, Paint, StrokeLineCap, StrokeLineJoin},
        svg_path::parse_path_data,
        svg_transform::parse_transform_list,
    },
    html::{Document as HtmlDocument, Element as HtmlElement, Node as HtmlNode},
    url::Url,
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
struct ClipPath {
    path: Path,
    fill_rule: FillRule,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum StrokeLength {
    UserUnits(f32),
    Percentage(f32),
}

impl StrokeLength {
    fn resolve(self, percentage_basis: f32) -> f32 {
        match self {
            Self::UserUnits(value) => value,
            Self::Percentage(value) => value * percentage_basis,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum SvgPaintReference {
    Solid(Color),
    Url {
        reference: String,
        fallback: Option<Color>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct SvgShape {
    pub path: Path,
    pub fill: Option<Color>,
    fill_paint: Option<Paint>,
    pub fill_rule: FillRule,
    pub stroke: Option<Color>,
    pub stroke_width: f32,
    stroke_transform_scale: f32,
    clip_paths: Vec<ClipPath>,
    stroke_line_cap: StrokeLineCap,
    stroke_line_join: StrokeLineJoin,
    stroke_miter_limit: f32,
    stroke_dash_array: Vec<f32>,
    stroke_dash_offset: f32,
    path_length: Option<f32>,
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
    let base_url = Url::parse("freedom://inline-svg").expect("inline SVG base URL must be valid");

    parse_svg_at(input, &base_url)
}

pub fn parse_svg_at(input: &str, base_url: &Url) -> Result<SvgDocument, SvgError> {
    let html = HtmlDocument::parse(input);
    let root = html
        .first_element("svg")
        .ok_or_else(|| SvgError::new("missing <svg> root element"))?;

    parse_svg_element_at(root, base_url)
}

pub fn parse_svg_element(root: &HtmlElement) -> Result<SvgDocument, SvgError> {
    let base_url = Url::parse("freedom://inline-svg/").expect("inline SVG base URL must be valid");

    parse_svg_element_at(root, &base_url)
}

pub fn parse_svg_element_at(root: &HtmlElement, base_url: &Url) -> Result<SvgDocument, SvgError> {
    if !root.name.eq_ignore_ascii_case("svg") {
        return Err(SvgError::new("expected an <svg> element"));
    }

    let width = match attribute(root, "width") {
        Some(value) => parse_svg_viewport_dimension(value)?,
        None => None,
    };

    let height = match attribute(root, "height") {
        Some(value) => parse_svg_viewport_dimension(value)?,
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

    let stylesheets = collect_svg_stylesheets(root);
    let css_rules = build_cascade_rules(&stylesheets);
    let mut shapes = Vec::new();
    let mut reference_stack = Vec::new();
    let stroke_percentage_basis = normalized_viewport_diagonal(view_box, width, height);

    collect_shapes(
        root,
        root,
        base_url,
        &css_rules,
        SvgStyle::default(),
        Transform::IDENTITY,
        stroke_percentage_basis,
        &[],
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

#[derive(Clone)]
struct SvgStyle {
    fill: Option<SvgPaintReference>,
    fill_rule: FillRule,
    stroke: Option<SvgPaintReference>,
    stroke_width: StrokeLength,
    fill_opacity: f32,
    stroke_opacity: f32,
    stroke_line_cap: StrokeLineCap,
    stroke_line_join: StrokeLineJoin,
    stroke_miter_limit: f32,
    stroke_dash_array: Vec<StrokeLength>,
    stroke_dash_offset: StrokeLength,
}

impl Default for SvgStyle {
    fn default() -> Self {
        Self {
            fill: Some(SvgPaintReference::Solid(Color::BLACK)),
            fill_rule: FillRule::NonZero,
            stroke: None,
            stroke_width: StrokeLength::UserUnits(1.0),
            fill_opacity: 1.0,
            stroke_opacity: 1.0,
            stroke_line_cap: StrokeLineCap::Butt,
            stroke_line_join: StrokeLineJoin::Miter,
            stroke_miter_limit: 4.0,
            stroke_dash_array: Vec::new(),
            stroke_dash_offset: StrokeLength::UserUnits(0.0),
        }
    }
}

fn normalized_viewport_diagonal(
    view_box: Option<ViewBox>,
    width: Option<f32>,
    height: Option<f32>,
) -> f32 {
    let (width, height) = view_box
        .map(|view_box| (view_box.width, view_box.height))
        .unwrap_or((width.unwrap_or(300.0), height.unwrap_or(150.0)));

    normalized_diagonal(width, height)
}

fn normalized_diagonal(width: f32, height: f32) -> f32 {
    ((width * width + height * height) * 0.5).sqrt()
}

fn collect_shapes(
    root: &HtmlElement,
    element: &HtmlElement,
    base_url: &Url,
    css_rules: &[CascadeRule],
    inherited_style: SvgStyle,
    parent_transform: Transform,
    stroke_percentage_basis: f32,
    clip_paths: &[ClipPath],
    shapes: &mut Vec<SvgShape>,
    reference_stack: &mut Vec<String>,
) -> Result<(), SvgError> {
    // SVG conditional processing prevents rendering elements that require an
    // unsupported extension.
    if attribute(element, "requiredextensions").is_some() {
        return Ok(());
    }

    if matches!(
        element.name.as_str(),
        "defs" | "clippath" | "mask" | "lineargradient" | "radialgradient" | "pattern" | "symbol"
    ) {
        return Ok(());
    }

    let style = resolve_style(element, inherited_style, css_rules)?;

    let transform = element_transform(element)?.then(parent_transform);

    let mut element_clip_paths = clip_paths.to_vec();
    let mut clip_path_references = Vec::new();

    if let Some(clip_paths) = resolve_clip_paths(
        root,
        element,
        base_url,
        transform,
        &mut clip_path_references,
    )? {
        element_clip_paths.extend(clip_paths);
    }

    if element.name.eq_ignore_ascii_case("switch") {
        for child in &element.children {
            let HtmlNode::Element(child) = child else {
                continue;
            };

            if !switch_child_is_supported(child) {
                continue;
            }

            collect_shapes(
                root,
                child,
                base_url,
                css_rules,
                style,
                transform,
                stroke_percentage_basis,
                &element_clip_paths,
                shapes,
                reference_stack,
            )?;

            break;
        }

        return Ok(());
    }

    if element.name.eq_ignore_ascii_case("svg") && !std::ptr::eq(root, element) {
        let x = number_attribute(element, "x", 0.0)?;
        let y = number_attribute(element, "y", 0.0)?;

        let width = attribute(element, "width")
            .map(parse_svg_length)
            .transpose()?
            .unwrap_or(300.0);

        let height = attribute(element, "height")
            .map(parse_svg_length)
            .transpose()?
            .unwrap_or(150.0);

        if width == 0.0 || height == 0.0 {
            return Ok(());
        }

        let placement = Transform::translate(x, y).then(transform);

        let (child_transform, child_stroke_percentage_basis) = match attribute(element, "viewbox") {
            Some(value) => match parse_view_box(value)? {
                ParsedViewBox::Valid(view_box) => {
                    let preserve_aspect_ratio = match attribute(element, "preserveaspectratio") {
                        Some(value) => parse_preserve_aspect_ratio(value)?,
                        None => PreserveAspectRatio::default(),
                    };

                    (
                        view_box_transform(view_box, preserve_aspect_ratio, width, height)
                            .then(placement),
                        normalized_diagonal(view_box.width, view_box.height),
                    )
                }
                ParsedViewBox::DisablesRendering => return Ok(()),
            },
            None => (placement, normalized_diagonal(width, height)),
        };

        let mut child_clip_paths = element_clip_paths;
        child_clip_paths.push(ClipPath {
            path: viewport_path(width, height).transformed(placement),
            fill_rule: FillRule::NonZero,
        });

        for child in &element.children {
            if let HtmlNode::Element(child) = child {
                collect_shapes(
                    root,
                    child,
                    base_url,
                    css_rules,
                    style.clone(),
                    child_transform,
                    child_stroke_percentage_basis,
                    &child_clip_paths,
                    shapes,
                    reference_stack,
                )?;
            }
        }

        return Ok(());
    }

    if element.name.eq_ignore_ascii_case("use") {
        let Some(reference_id) = use_reference_id(base_url, element)? else {
            return Ok(());
        };

        if reference_stack.iter().any(|id| id == &reference_id) {
            return Err(SvgError::new("circular SVG use reference"));
        }

        let referenced_element = find_element_by_id(root, &reference_id)
            .ok_or_else(|| SvgError::new("SVG use target was not found"))?;

        let x = number_attribute(element, "x", 0.0)?;
        let y = number_attribute(element, "y", 0.0)?;
        let reference_transform = Transform::translate(x, y).then(transform);

        reference_stack.push(reference_id.to_owned());

        let result = collect_shapes(
            root,
            referenced_element,
            base_url,
            css_rules,
            style,
            reference_transform,
            stroke_percentage_basis,
            &element_clip_paths,
            shapes,
            reference_stack,
        );

        reference_stack.pop();

        return result;
    }

    let path = path_for_element(element)?;

    if let Some(path) = path {
        let (fill, fill_paint) = resolve_paint(
            root,
            base_url,
            &style.fill,
            &path,
            transform,
            style.fill_opacity,
        )?;

        let (stroke, _) = resolve_paint(
            root,
            base_url,
            &style.stroke,
            &path,
            transform,
            style.stroke_opacity,
        )?;

        if fill.is_some() || fill_paint.is_some() || stroke.is_some() {
            shapes.push(SvgShape {
                path: path.transformed(transform),
                fill,
                fill_paint,
                fill_rule: style.fill_rule,
                stroke,
                stroke_width: style.stroke_width.resolve(stroke_percentage_basis),
                stroke_transform_scale: stroke_scale(transform),
                clip_paths: element_clip_paths.clone(),
                stroke_line_cap: style.stroke_line_cap,
                stroke_line_join: style.stroke_line_join,
                stroke_miter_limit: style.stroke_miter_limit,
                stroke_dash_array: style
                    .stroke_dash_array
                    .iter()
                    .map(|length| length.resolve(stroke_percentage_basis))
                    .collect(),
                stroke_dash_offset: style.stroke_dash_offset.resolve(stroke_percentage_basis),
                path_length: path_length_attribute(element)?,
            });
        }
    }

    for child in &element.children {
        if let HtmlNode::Element(child) = child {
            collect_shapes(
                root,
                child,
                base_url,
                css_rules,
                style.clone(),
                transform,
                stroke_percentage_basis,
                &element_clip_paths,
                shapes,
                reference_stack,
            )?;
        }
    }

    Ok(())
}

fn element_transform(element: &HtmlElement) -> Result<Transform, SvgError> {
    match attribute(element, "transform") {
        Some(value) => {
            parse_transform_list(value).map_err(|error| SvgError::new(error.to_string()))
        }
        None => Ok(Transform::IDENTITY),
    }
}

fn path_for_element(element: &HtmlElement) -> Result<Option<Path>, SvgError> {
    match element.name.as_str() {
        "path" => attribute(element, "d")
            .map(|data| parse_path_data(data).map_err(|error| SvgError::new(error.to_string())))
            .transpose(),

        "rect" => parse_rect(element),
        "circle" => parse_circle(element),
        "ellipse" => parse_ellipse(element),
        "polygon" => parse_poly_points(element, true),
        "polyline" => parse_poly_points(element, false),
        "line" => parse_line(element),

        _ => Ok(None),
    }
}

fn use_reference_id(base_url: &Url, element: &HtmlElement) -> Result<Option<String>, SvgError> {
    let Some(value) = attribute(element, "href").or_else(|| attribute(element, "xlink:href"))
    else {
        return Ok(None);
    };

    let Some(reference_id) = local_reference_id(base_url, value.trim())? else {
        return Ok(None);
    };

    if reference_id.is_empty() {
        return Ok(None);
    }

    Ok(Some(reference_id))
}

fn local_reference_id(base_url: &Url, reference: &str) -> Result<Option<String>, SvgError> {
    base_url
        .local_fragment(reference)
        .map_err(|error| SvgError::new(format!("invalid SVG reference URL: {error}")))
}

fn attribute<'a>(element: &'a HtmlElement, name: &str) -> Option<&'a str> {
    element
        .attributes
        .iter()
        .find(|attribute| attribute.name.eq_ignore_ascii_case(name))
        .map(|attribute| attribute.value.as_str())
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

fn switch_child_is_supported(element: &HtmlElement) -> bool {
    attribute(element, "requiredextensions").is_none()
        && attribute(element, "requiredfeatures").is_none()
        && attribute(element, "systemlanguage").is_none()
}

fn path_length_attribute(element: &HtmlElement) -> Result<Option<f32>, SvgError> {
    let Some(value) = attribute(element, "pathlength") else {
        return Ok(None);
    };

    let values = parse_number_list(value)?;

    if values.len() != 1 || !values[0].is_finite() || values[0] <= 0.0 {
        return Err(SvgError::new("pathLength must be a positive finite number"));
    }

    Ok(Some(values[0]))
}

fn flattened_path_length(path: &FlattenedPath) -> f32 {
    let mut total = 0.0;

    for contour in path.contours() {
        let points = contour.points();

        for segment in points.windows(2) {
            total += point_distance(segment[0], segment[1]);
        }

        if contour.is_closed() && points.len() > 1 {
            total += point_distance(points[points.len() - 1], points[0]);
        }
    }

    total
}

fn point_distance(from: Point, to: Point) -> f32 {
    let delta_x = to.x - from.x;
    let delta_y = to.y - from.y;

    (delta_x * delta_x + delta_y * delta_y).sqrt()
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

fn parse_fill_rule(value: &str) -> Result<FillRule, SvgError> {
    if value.trim().eq_ignore_ascii_case("nonzero") {
        Ok(FillRule::NonZero)
    } else if value.trim().eq_ignore_ascii_case("evenodd") {
        Ok(FillRule::EvenOdd)
    } else {
        Err(SvgError::new("unsupported fill-rule"))
    }
}

fn stroke_scale(transform: Transform) -> f32 {
    let x_axis_scale = (transform.a * transform.a + transform.b * transform.b).sqrt();
    let y_axis_scale = (transform.c * transform.c + transform.d * transform.d).sqrt();

    (x_axis_scale + y_axis_scale) * 0.5
}

#[cfg(test)]
mod tests;
