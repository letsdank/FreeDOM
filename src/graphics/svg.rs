use crate::page::BlockContent::Svg;
use crate::{
    css::{
        CascadeRule, Element as CssElement, build_cascade_rules, matching_declarations,
        parse_declaration_list,
    },
    graphics::{
        flatten::{FlattenError, FlattenedPath, flatten_path},
        geometry::{Path, Point, Transform},
        raster::{
            Color, FillRule, GradientStop, LinearGradient, Paint, StrokeLineCap, StrokeLineJoin,
            StrokeStyle, Surface, fill_path_clipped, fill_path_with_paint_clipped,
            stroke_path_clipped,
        },
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
            .map(|view_box| {
                view_box_transform(
                    view_box,
                    self.preserve_aspect_ratio,
                    width as f32,
                    height as f32,
                )
            })
            .unwrap_or(Transform::IDENTITY);

        let viewport_stroke_scale = stroke_scale(viewport_transform);

        for shape in &self.shapes {
            let transformed_path = shape.path.transformed(viewport_transform);
            let flattened = flatten_path(&transformed_path, 0.25)?;

            let clip_paths = shape
                .clip_paths
                .iter()
                .map(|clip_path| {
                    flatten_path(&clip_path.path.transformed(viewport_transform), 0.25)
                        .map(|path| (path, clip_path.fill_rule))
                })
                .collect::<Result<Vec<_>, _>>()?;

            if let Some(gradient) = &shape.fill_gradient {
                let gradient = LinearGradient::new(
                    viewport_transform.apply_point(gradient.start),
                    viewport_transform.apply_point(gradient.end),
                    gradient.stops.clone(),
                )
                .expect("validated SVG gradient must stay valid after viewport transform");

                fill_path_with_paint_clipped(
                    &mut surface,
                    &flattened,
                    shape.fill_rule,
                    &Paint::LinearGradient(gradient),
                    &clip_paths,
                );
            } else if let Some(fill) = shape.fill {
                fill_path_clipped(&mut surface, &flattened, shape.fill_rule, fill, &clip_paths);
            }

            if let Some(stroke) = shape.stroke {
                let shape_stroke_scale = shape.stroke_transform_scale * viewport_stroke_scale;
                let dash_scale = match shape.path_length {
                    Some(path_length) => {
                        let actual_length = flattened_path_length(&flattened);

                        if actual_length.is_finite() && actual_length > 0.0 {
                            actual_length / path_length
                        } else {
                            0.0
                        }
                    }

                    None => shape_stroke_scale,
                };

                stroke_path_clipped(
                    &mut surface,
                    &flattened,
                    StrokeStyle {
                        width: shape.stroke_width * shape_stroke_scale,
                        line_cap: shape.stroke_line_cap,
                        line_join: shape.stroke_line_join,
                        miter_limit: shape.stroke_miter_limit,
                        dash_array: shape
                            .stroke_dash_array
                            .iter()
                            .map(|length| length * dash_scale)
                            .collect(),
                        dash_offset: shape.stroke_dash_offset * dash_scale,
                    },
                    stroke,
                    &clip_paths,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GradientUnits {
    ObjectBoundingBox,
    UserSpaceOnUse,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SvgShape {
    pub path: Path,
    pub fill: Option<Color>,
    fill_gradient: Option<LinearGradient>,
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
        let (fill, fill_gradient) = resolve_paint(
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

        if fill.is_some() || fill_gradient.is_some() || stroke.is_some() {
            shapes.push(SvgShape {
                path: path.transformed(transform),
                fill,
                fill_gradient,
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

fn resolve_paint(
    root: &HtmlElement,
    base_url: &Url,
    paint: &Option<SvgPaintReference>,
    object_path: &Path,
    transform: Transform,
    opacity: f32,
) -> Result<(Option<Color>, Option<LinearGradient>), SvgError> {
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

            Ok((None, Some(gradient)))
        }
    }
}

fn resolve_linear_gradient(
    root: &HtmlElement,
    base_url: &Url,
    reference_id: &str,
    object_path: &Path,
    transform: Transform,
    opacity: f32,
) -> Result<Option<LinearGradient>, SvgError> {
    let mut reference_stack = Vec::new();

    let Some(gradients) =
        linear_gradient_chain(root, base_url, reference_id, &mut reference_stack)?
    else {
        return Ok(None);
    };

    let units = inherited_gradient_units(&gradients)?;

    let gradient_transform = inherited_gradient_attribute(&gradients, "gradienttransform")
        .map(|value| parse_transform_list(value).map_err(|error| SvgError::new(error.to_string())))
        .transpose()?
        .unwrap_or(Transform::IDENTITY);

    if gradient_transform.inverse().is_none() {
        return Ok(None);
    }

    let (start, end) = match units {
        GradientUnits::UserSpaceOnUse => (
            Point::new(
                inherited_gradient_number(&gradients, "x1", 0.0)?,
                inherited_gradient_number(&gradients, "y1", 0.0)?,
            ),
            Point::new(
                inherited_gradient_number(&gradients, "x2", 1.0)?,
                inherited_gradient_number(&gradients, "y2", 0.0)?,
            ),
        ),

        GradientUnits::ObjectBoundingBox => {
            let Some(bounds) = object_path.control_bounds() else {
                return Ok(None);
            };

            let x1 = inherited_object_bounding_box_coordinate(&gradients, "x1", 0.0)?;
            let y1 = inherited_object_bounding_box_coordinate(&gradients, "y1", 0.0)?;
            let x2 = inherited_object_bounding_box_coordinate(&gradients, "x2", 1.0)?;
            let y2 = inherited_object_bounding_box_coordinate(&gradients, "y2", 0.0)?;

            (
                Point::new(
                    bounds.min.x + x1 * bounds.width(),
                    bounds.min.y + y1 * bounds.height(),
                ),
                Point::new(
                    bounds.min.x + x2 * bounds.width(),
                    bounds.min.y + y2 * bounds.height(),
                ),
            )
        }
    };

    let stops = inherited_gradient_stops(&gradients)?;

    let gradient_transform = gradient_transform.then(transform);

    let Some(mut gradient) = LinearGradient::new(
        gradient_transform.apply_point(start),
        gradient_transform.apply_point(end),
        stops,
    ) else {
        return Ok(None);
    };

    for stop in &mut gradient.stops {
        stop.color = apply_opacity(stop.color, opacity);
    }

    Ok(Some(gradient))
}

fn linear_gradient_chain<'a>(
    root: &'a HtmlElement,
    base_url: &Url,
    reference_id: &str,
    reference_stack: &mut Vec<String>,
) -> Result<Option<Vec<&'a HtmlElement>>, SvgError> {
    if reference_stack.iter().any(|id| id == reference_id) {
        return Err(SvgError::new("circular linearGradient reference"));
    }

    let Some(element) = find_element_by_id(root, reference_id) else {
        return Ok(None);
    };

    if !element.name.eq_ignore_ascii_case("lineargradient") {
        return Ok(None);
    }

    reference_stack.push(reference_id.to_owned());

    let result = (|| {
        let mut gradients = vec![element];

        let href = attribute(element, "href").or_else(|| attribute(element, "xlink:href"));

        if let Some(href) = href
            && let Some(parent_id) = local_reference_id(base_url, href)?
        {
            let Some(parent_gradients) =
                linear_gradient_chain(root, base_url, &parent_id, reference_stack)?
            else {
                return Ok(None);
            };

            gradients.extend(parent_gradients);
        }

        Ok(Some(gradients))
    })();

    reference_stack.pop();
    result
}

fn inherited_gradient_attribute<'a>(gradients: &[&'a HtmlElement], name: &str) -> Option<&'a str> {
    gradients
        .iter()
        .find_map(|gradient| attribute(gradient, name))
}

fn inherited_gradient_units(gradients: &[&HtmlElement]) -> Result<GradientUnits, SvgError> {
    match inherited_gradient_attribute(gradients, "gradientunits") {
        None => Ok(GradientUnits::ObjectBoundingBox),

        Some(value) if value.trim().eq_ignore_ascii_case("objectboundingbox") => {
            Ok(GradientUnits::ObjectBoundingBox)
        }

        Some(value) if value.trim().eq_ignore_ascii_case("userspaceonuse") => {
            Ok(GradientUnits::UserSpaceOnUse)
        }

        Some(_) => Err(SvgError::new("unsupported gradientUnits value")),
    }
}

fn inherited_gradient_number(
    gradients: &[&HtmlElement],
    name: &str,
    default: f32,
) -> Result<f32, SvgError> {
    let Some(value) = inherited_gradient_attribute(gradients, name) else {
        return Ok(default);
    };

    let values = parse_number_list(value)?;

    if values.len() != 1 || !values[0].is_finite() {
        return Err(SvgError::new("expected one finite SVG gradient coordinate"));
    }

    Ok(values[0])
}

fn inherited_object_bounding_box_coordinate(
    gradients: &[&HtmlElement],
    name: &str,
    default: f32,
) -> Result<f32, SvgError> {
    let Some(value) = inherited_gradient_attribute(gradients, name) else {
        return Ok(default);
    };

    let value = value.trim();

    let (number_source, scale) = match value.strip_suffix('%') {
        Some(percent) => (percent, 0.01),
        None => (value, 1.0),
    };

    let values = parse_number_list(number_source)?;

    if values.len() != 1 || !values[0].is_finite() {
        return Err(SvgError::new(
            "expected one finite objectBoundingBox gradient coordinate",
        ));
    }

    Ok(values[0] * scale)
}

fn inherited_gradient_stops(gradients: &[&HtmlElement]) -> Result<Vec<GradientStop>, SvgError> {
    for gradient in gradients {
        let stops = gradient
            .children
            .iter()
            .filter_map(|child| match child {
                HtmlNode::Element(element) if element.name.eq_ignore_ascii_case("stop") => {
                    Some(parse_gradient_stop(element))
                }
                _ => None,
            })
            .collect::<Result<Vec<_>, _>>()?;

        if !stops.is_empty() {
            return Ok(stops);
        }
    }

    Ok(Vec::new())
}

fn parse_gradient_stop(element: &HtmlElement) -> Result<GradientStop, SvgError> {
    let mut color = attribute(element, "stop-color")
        .map(parse_solid_color)
        .transpose()?
        .flatten()
        .unwrap_or(Color::BLACK);

    let mut opacity = attribute(element, "stop-opacity")
        .map(parse_opacity)
        .transpose()?
        .unwrap_or(1.0);

    if let Some(style) = attribute(element, "style") {
        for declaration in parse_declaration_list(style) {
            match declaration.name.as_str() {
                "stop-color" => {
                    color = parse_solid_color(&declaration.value)?.unwrap_or(Color::TRANSPARENT);
                }
                "stop-opacity" => opacity = parse_opacity(&declaration.value)?,
                _ => {}
            }
        }
    }

    let offset = attribute(element, "offset")
        .map(parse_gradient_stop_offset)
        .transpose()?
        .unwrap_or(0.0);

    Ok(GradientStop {
        offset,
        color: apply_opacity(color, opacity),
    })
}

fn parse_gradient_stop_offset(value: &str) -> Result<f32, SvgError> {
    let value = value.trim();

    let values = if let Some(percent) = value.strip_suffix('%') {
        parse_number_list(percent)?
            .into_iter()
            .map(|value| value / 100.0)
            .collect::<Vec<_>>()
    } else {
        parse_number_list(value)?
    };

    let [offset] = values.as_slice() else {
        return Err(SvgError::new("invalid gradient stop offset"));
    };

    Ok(offset.clamp(0.0, 1.0))
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

fn resolve_clip_paths(
    root: &HtmlElement,
    element: &HtmlElement,
    base_url: &Url,
    transform: Transform,
    reference_stack: &mut Vec<String>,
) -> Result<Option<Vec<ClipPath>>, SvgError> {
    let Some(reference_id) = clip_path_reference_id(base_url, element)? else {
        return Ok(None);
    };

    if reference_stack.iter().any(|id| id == &reference_id) {
        return Err(SvgError::new("circular SVG clip-path reference"));
    }

    reference_stack.push(reference_id.clone());

    let result = (|| -> Result<Option<Vec<ClipPath>>, SvgError> {
        let clip_path = find_element_by_id(root, &reference_id)
            .ok_or_else(|| SvgError::new("SVG clip-path target was not found"))?;

        if !clip_path.name.eq_ignore_ascii_case("clippath") {
            return Err(SvgError::new(
                "SVG clip-path target is not a clipPath element",
            ));
        }

        if let Some(units) = attribute(clip_path, "clippathunits") {
            if !units.eq_ignore_ascii_case("userspaceonuse") {
                return Err(SvgError::new("unsupported clipPathUnits value"));
            }
        }

        let clip_transform = element_transform(clip_path)?.then(transform);
        let mut path = Path::new();
        let mut fill_rule = FillRule::NonZero;
        let mut nested_clip_paths = Vec::new();

        for child in &clip_path.children {
            if let HtmlNode::Element(child) = child {
                collect_clip_path_geometry(
                    root,
                    child,
                    base_url,
                    clip_transform,
                    &mut path,
                    &mut fill_rule,
                    &mut nested_clip_paths,
                    reference_stack,
                )?;
            }
        }

        let mut resolved = vec![ClipPath { path, fill_rule }];
        resolved.append(&mut nested_clip_paths);

        if let Some(mut nested) =
            resolve_clip_paths(root, clip_path, base_url, clip_transform, reference_stack)?
        {
            resolved.append(&mut nested);
        }

        Ok(Some(resolved))
    })();

    reference_stack.pop();

    result
}

fn clip_path_reference_id(
    base_url: &Url,
    element: &HtmlElement,
) -> Result<Option<String>, SvgError> {
    let Some(value) = attribute(element, "clip-path") else {
        return Ok(None);
    };

    let value = value.trim();

    if value.eq_ignore_ascii_case("none") {
        return Ok(None);
    }

    let Some(reference) = value
        .strip_prefix("url(")
        .and_then(|value| value.strip_suffix(')'))
    else {
        return Err(SvgError::new("unsupported clip-path reference"));
    };

    let Some(reference_id) = local_reference_id(base_url, reference.trim())? else {
        return Err(SvgError::new(
            "external SVG clip-path references are not supported",
        ));
    };

    if reference_id.is_empty() {
        return Err(SvgError::new("clip-path reference is empty"));
    }

    Ok(Some(reference_id))
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

fn collect_clip_path_geometry(
    root: &HtmlElement,
    element: &HtmlElement,
    base_url: &Url,
    parent_transform: Transform,
    output: &mut Path,
    fill_rule: &mut FillRule,
    nested_clip_paths: &mut Vec<ClipPath>,
    reference_stack: &mut Vec<String>,
) -> Result<(), SvgError> {
    let transform = element_transform(element)?.then(parent_transform);

    if element.name.eq_ignore_ascii_case("use") {
        let Some(reference_id) = use_reference_id(base_url, element)? else {
            return Ok(());
        };

        if reference_stack.iter().any(|id| id == &reference_id) {
            return Err(SvgError::new("circular SVG use reference in clipPath"));
        }

        let reference_element = find_element_by_id(root, &reference_id)
            .ok_or_else(|| SvgError::new("SVG use target was not found"))?;

        let x = number_attribute(element, "x", 0.0)?;
        let y = number_attribute(element, "y", 0.0)?;
        let reference_transform = Transform::translate(x, y).then(transform);

        reference_stack.push(reference_id);

        let result = collect_clip_path_geometry(
            root,
            reference_element,
            base_url,
            reference_transform,
            output,
            fill_rule,
            nested_clip_paths,
            reference_stack,
        );

        reference_stack.pop();

        return result;
    }

    if let Some(mut nested) =
        resolve_clip_paths(root, element, base_url, transform, reference_stack)?
    {
        nested_clip_paths.append(&mut nested);
    }

    if let Some(value) = attribute(element, "clip-rule") {
        *fill_rule = parse_fill_rule(value)?
    }

    if let Some(path) = path_for_element(element)? {
        output.append(&path.transformed(transform));
    }

    for child in &element.children {
        if let HtmlNode::Element(child) = child {
            collect_clip_path_geometry(
                root,
                child,
                base_url,
                transform,
                output,
                fill_rule,
                nested_clip_paths,
                reference_stack,
            )?;
        }
    }

    Ok(())
}

fn resolve_style(
    element: &HtmlElement,
    mut style: SvgStyle,
    css_rules: &[CascadeRule],
) -> Result<SvgStyle, SvgError> {
    for property in [
        "fill",
        "fill-rule",
        "stroke",
        "stroke-width",
        "fill-opacity",
        "stroke-opacity",
        "stroke-linecap",
        "stroke-linejoin",
        "stroke-miterlimit",
        "stroke-dasharray",
        "stroke-dashoffset",
    ] {
        if let Some(value) = attribute(element, property) {
            apply_svg_declaration(&mut style, property, value)?;
        }
    }

    let classes: Vec<String> = attribute(element, "class")
        .map(|value| {
            value
                .split_ascii_whitespace()
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();

    let css_element = CssElement {
        tag: &element.name,
        id: attribute(element, "id"),
        classes: &classes,
        inline_style: None,
    };

    for declaration in matching_declarations(css_rules, &css_element) {
        apply_svg_declaration(&mut style, &declaration.name, &declaration.value)?;
    }

    if let Some(value) = attribute(element, "style") {
        for declaration in parse_declaration_list(value) {
            apply_svg_declaration(&mut style, &declaration.name, &declaration.value)?;
        }
    }

    Ok(style)
}

fn apply_svg_declaration(
    style: &mut SvgStyle,
    property: &str,
    value: &str,
) -> Result<(), SvgError> {
    match property {
        "fill" => style.fill = parse_paint(value)?,
        "fill-rule" => style.fill_rule = parse_fill_rule(value)?,
        "stroke" => style.stroke = parse_paint(value)?,
        "stroke-width" => style.stroke_width = parse_stroke_width(value)?,
        "fill-opacity" => style.fill_opacity = parse_opacity(value)?,
        "stroke-opacity" => style.stroke_opacity = parse_opacity(value)?,
        "stroke-linecap" => style.stroke_line_cap = parse_stroke_line_cap(value)?,
        "stroke-linejoin" => style.stroke_line_join = parse_stroke_line_join(value)?,
        "stroke-miterlimit" => style.stroke_miter_limit = parse_stroke_miter_limit(value)?,
        "stroke-dasharray" => style.stroke_dash_array = parse_stroke_dash_array(value)?,
        "stroke-dashoffset" => style.stroke_dash_offset = parse_stroke_dash_offset(value)?,
        _ => {}
    }

    Ok(())
}

fn collect_svg_stylesheets(root: &HtmlElement) -> Vec<String> {
    let mut stylesheets = Vec::new();
    collect_svg_stylesheets_from_element(root, &mut stylesheets);
    stylesheets
}

fn collect_svg_stylesheets_from_element(element: &HtmlElement, stylesheets: &mut Vec<String>) {
    if element.name.eq_ignore_ascii_case("style") {
        stylesheets.push(svg_element_text_content(element));
    }

    for child in &element.children {
        if let HtmlNode::Element(child) = child {
            collect_svg_stylesheets_from_element(child, stylesheets);
        }
    }
}

fn svg_element_text_content(element: &HtmlElement) -> String {
    let mut output = String::new();

    for child in &element.children {
        append_svg_node_text(child, &mut output);
    }

    output
}

fn append_svg_node_text(node: &HtmlNode, output: &mut String) {
    match node {
        HtmlNode::Text(text) => output.push_str(text),

        HtmlNode::Element(element) => {
            for child in &element.children {
                append_svg_node_text(child, output);
            }
        }

        HtmlNode::Doctype(_) | HtmlNode::Comment(_) => {}
    }
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

fn parse_stroke_line_cap(value: &str) -> Result<StrokeLineCap, SvgError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "butt" => Ok(StrokeLineCap::Butt),
        "round" => Ok(StrokeLineCap::Round),
        "square" => Ok(StrokeLineCap::Square),
        _ => Err(SvgError::new("unsupported stroke-linecap")),
    }
}

fn parse_stroke_line_join(value: &str) -> Result<StrokeLineJoin, SvgError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "miter" => Ok(StrokeLineJoin::Miter),
        "round" => Ok(StrokeLineJoin::Round),
        "bevel" => Ok(StrokeLineJoin::Bevel),
        _ => Err(SvgError::new("unsupported stroke-linejoin")),
    }
}

fn parse_stroke_miter_limit(value: &str) -> Result<f32, SvgError> {
    let values = parse_number_list(value)?;

    if values.len() != 1 || !values[0].is_finite() || values[0] < 1.0 {
        return Err(SvgError::new(
            "stroke-miterlimit must be a finite number no smaller than one",
        ));
    }

    Ok(values[0])
}

fn parse_stroke_dash_array(value: &str) -> Result<Vec<StrokeLength>, SvgError> {
    if value.trim().eq_ignore_ascii_case("none") {
        return Ok(Vec::new());
    }

    let values = parse_stroke_length_list(value, false)?;

    if values.is_empty() {
        return Err(SvgError::new("invalid stroke-dasharray"));
    }

    Ok(values)
}

fn parse_stroke_dash_offset(value: &str) -> Result<StrokeLength, SvgError> {
    let values = parse_stroke_length_list(value, true)?;

    let [value] = values.as_slice() else {
        return Err(SvgError::new("invalid stroke-dashoffset"));
    };

    Ok(*value)
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

fn parse_opacity(value: &str) -> Result<f32, SvgError> {
    let values = parse_number_list(value)?;

    if values.len() != 1 || !(0.0..=1.0).contains(&values[0]) {
        return Err(SvgError::new("opacity must be a number from zero to one"));
    }

    Ok(values[0])
}

fn apply_opacity(mut color: Color, opacity: f32) -> Color {
    color.alpha = (color.alpha as f32 * opacity).round() as u8;
    color
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

fn object_bounding_box_coordinate(
    element: &HtmlElement,
    name: &str,
    default: f32,
) -> Result<f32, SvgError> {
    let Some(value) = attribute(element, name) else {
        return Ok(default);
    };

    let value = value.trim();

    let (number_source, scale) = match value.strip_suffix('%') {
        Some(percent) => (percent, 0.01),
        None => (value, 1.0),
    };

    let values = parse_number_list(number_source)?;

    if values.len() != 1 || !values[0].is_finite() {
        return Err(SvgError::new(
            "expected one finite objectBoundingBox gradient coordinate",
        ));
    }

    Ok(values[0] * scale)
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

fn parse_paint(value: &str) -> Result<Option<SvgPaintReference>, SvgError> {
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

fn parse_solid_color(value: &str) -> Result<Option<Color>, SvgError> {
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

fn parse_stroke_width(value: &str) -> Result<StrokeLength, SvgError> {
    let values = parse_stroke_length_list(value, false)?;

    let [value] = values.as_slice() else {
        return Err(SvgError::new("invalid stroke-width"));
    };

    Ok(*value)
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

fn parse_svg_viewport_dimension(value: &str) -> Result<Option<f32>, SvgError> {
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

fn parse_number_list(input: &str) -> Result<Vec<f32>, SvgError> {
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

fn skip_svg_number_separators(input: &str, cursor: &mut usize) {
    while input
        .as_bytes()
        .get(*cursor)
        .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b',')
    {
        *cursor += 1;
    }
}

fn parse_svg_number(input: &str, cursor: &mut usize) -> Result<f32, SvgError> {
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

fn stroke_scale(transform: Transform) -> f32 {
    let x_axis_scale = (transform.a * transform.a + transform.b * transform.b).sqrt();
    let y_axis_scale = (transform.c * transform.c + transform.d * transform.d).sqrt();

    (x_axis_scale + y_axis_scale) * 0.5
}

fn view_box_transform(
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

fn viewport_path(width: f32, height: f32) -> Path {
    let mut path = Path::new();
    path.move_to(Point::new(0.0, 0.0));
    path.line_to(Point::new(width, 0.0));
    path.line_to(Point::new(width, height));
    path.line_to(Point::new(0.0, height));
    path.close();
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::geometry::PathCommand;
    use crate::url::Url;

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
            20.0,
            10.0,
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

    #[test]
    fn maps_nested_svg_viewboxes_into_their_viewports() {
        let document = parse_svg(
            r##"
                <svg>
                    <svg
                        x="10"
                        y="20"
                        width="20"
                        height="10"
                        viewBox="0 0 10 10"
                        preserveAspectRatio="none"
                    >
                        <rect width="10" height="10" fill="green"/>
                    </svg>
                </svg>
            "##,
        )
        .unwrap();

        let surface = document.render(50, 50, Color::WHITE).unwrap();

        assert_eq!(surface.pixel(25, 25), Some(Color::rgba(0, 128, 0, 255)));
    }

    #[test]
    fn clips_nested_svg_to_its_viewport() {
        let document = parse_svg(
            r##"
                <svg>
                    <svg
                        x="10"
                        y="20"
                        width="20"
                        height="10"
                        viewBox="0 0 10 10"
                        preserveAspectRatio="none"
                    >
                        <rect width="20" height="10" fill="red"/>
                    </svg>
                </svg>
            "##,
        )
        .unwrap();

        let surface = document.render(50, 50, Color::WHITE).unwrap();

        assert_eq!(surface.pixel(25, 25), Some(Color::rgba(255, 0, 0, 255)));
        assert_eq!(surface.pixel(35, 25), Some(Color::WHITE));
    }

    #[test]
    fn applies_fill_and_stroke_opacity() {
        let document = parse_svg(
            r##"
                <svg>
                    <rect
                        width="10"
                        height="10"
                        fill="red"
                        fill-opacity="0.5"
                        stroke="green"
                        stroke-opacity="0.25"
                    />
                </svg>
            "##,
        )
        .unwrap();

        assert_eq!(document.shapes[0].fill, Some(Color::rgba(255, 0, 0, 128)));

        assert_eq!(document.shapes[0].stroke, Some(Color::rgba(0, 128, 0, 64)));

        let surface = document.render(12, 12, Color::WHITE).unwrap();

        assert_eq!(surface.pixel(5, 5), Some(Color::rgba(255, 127, 127, 255)));
    }

    #[test]
    fn decodes_percent_encoded_use_fragment_identifiers() {
        let document = parse_svg(
            r##"
                <svg>
                    <defs>
                        <rect id="tile one" width="4" height="5" fill="green"/>
                    </defs>
                    <use href="#tile%20one"/>
                </svg>
            "##,
        )
        .unwrap();

        assert_eq!(document.shapes.len(), 1);
        assert_eq!(document.shapes[0].fill, Some(Color::rgba(0, 128, 0, 255)));
    }

    #[test]
    fn selects_the_first_supported_switch_child() {
        let document = parse_svg(
            r##"
                <svg>
                    <switch>
                        <rect width="10" height="10" fill="red"/>
                        <rect width="10" height="10" fill="green"/>
                    </switch>
                </svg>
            "##,
        )
        .unwrap();

        assert_eq!(document.shapes.len(), 1);
        assert_eq!(document.shapes[0].fill, Some(Color::rgba(255, 0, 0, 255)));
    }

    #[test]
    fn skips_unsupported_switch_children() {
        let document = parse_svg(
            r##"
                <svg>
                    <switch>
                        <rect
                            width="10"
                            height="10"
                            fill="red"
                            requiredExtensions="https://example.invalid/extension"
                        />
                        <rect width="10" height="10" fill="green"/>
                    </switch>
                </svg>
            "##,
        )
        .unwrap();

        assert_eq!(document.shapes.len(), 1);
        assert_eq!(document.shapes[0].fill, Some(Color::rgba(0, 128, 0, 255)));
    }

    #[test]
    fn applies_local_clip_paths() {
        let document = parse_svg(
            r##"
                <svg>
                    <clipPath id="center">
                        <rect x="2" y="2" width="4" height="4"/>
                    </clipPath>
                    <rect width="8" height="8" fill="red" clip-path="url(#center)"/>
                </svg>
            "##,
        )
        .unwrap();

        let surface = document.render(8, 8, Color::WHITE).unwrap();

        assert_eq!(surface.pixel(3, 3), Some(Color::rgba(255, 0, 0, 255)));
        assert_eq!(surface.pixel(1, 1), Some(Color::WHITE));
    }

    #[test]
    fn resolves_double_encoded_use_identifiers_through_document_url() {
        let html = crate::html::Document::parse(
            r##"
                <svg>
                    <defs>
                        <rect id="tile%20one" width="4" height="5" fill="green"/>
                    </defs>
                    <use href="#tile%2520one"/>
                </svg>
            "##,
        );

        let root = html.first_element("svg").unwrap();
        let base_url = Url::parse("https://example.test/assets/scene.svg").unwrap();

        let document = parse_svg_element_at(root, &base_url).unwrap();

        assert_eq!(document.shapes.len(), 1);
        assert_eq!(document.shapes[0].fill, Some(Color::rgba(0, 128, 0, 255)));
    }

    #[test]
    fn parses_stroke_join_properties() {
        let document = parse_svg(
            r#"
                <svg>
                    <path
                        d="M 2 6 L 5 2 L 8 6"
                        fill="none"
                        stroke="black"
                        stroke-linejoin="round"
                        style="stroke-miterlimit: 7"
                    />
                </svg>
            "#,
        )
        .unwrap();

        assert_eq!(document.shapes.len(), 1);
        assert_eq!(document.shapes[0].stroke_line_join, StrokeLineJoin::Round);
        assert_eq!(document.shapes[0].stroke_miter_limit, 7.0);
    }

    #[test]
    fn applies_embedded_stylesheet_rules() {
        let document = parse_svg(
            r#"
                <svg>
                    <style>
                        line { stroke: red; stroke-width: 2; }
                        #target { stroke: blue; stroke-width: 4px; }
                    </style>
                    <line id="target" x1="1" y1="1" x2="9" y2="1"/>
                </svg>
            "#,
        )
        .unwrap();

        assert_eq!(document.shapes.len(), 1);
        assert_eq!(document.shapes[0].stroke, Some(Color::rgba(0, 0, 255, 255)));
        assert_eq!(document.shapes[0].stroke_width, 4.0);
    }

    #[test]
    fn parses_dash_properties_and_path_length() {
        let document = parse_svg(
            r#"
                <svg>
                    <path
                        d="M 0 0 H 100"
                        fill="none"
                        stroke="blue"
                        pathLength="25"
                        style="stroke-dasharray: 3 1; stroke-dashoffset: 2"
                    />
                </svg>
            "#,
        )
        .unwrap();

        assert_eq!(document.shapes.len(), 1);
        assert_eq!(document.shapes[0].stroke_dash_array, vec![3.0, 1.0]);
        assert_eq!(document.shapes[0].stroke_dash_offset, 2.0);
        assert_eq!(document.shapes[0].path_length, Some(25.0));
    }

    #[test]
    fn renders_user_space_linear_gradients() {
        let document = parse_svg(
            r#"
                <svg width="10" height="2">
                    <defs>
                        <linearGradient
                            id="sky"
                            gradientUnits="userSpaceOnUse"
                            x1="0"
                            y1="0"
                            x2="10"
                            y2="0"
                        >
                            <stop offset="0" stop-color="red"/>
                            <stop offset="1" style="stop-color: blue"/>
                        </linearGradient>
                    </defs>
                    <rect width="10" height="2" fill="url(#sky)"/>
                </svg>
            "#,
        )
        .unwrap();

        let surface = document.render(10, 2, Color::TRANSPARENT).unwrap();

        let left = surface.pixel(1, 1).unwrap();
        let right = surface.pixel(8, 1).unwrap();

        assert!(left.red > left.blue);
        assert!(right.blue > right.red);
    }

    #[test]
    fn renders_object_bounding_box_linear_gradients() {
        let document = parse_svg(
            r#"
                <svg width="10" height="2">
                    <defs>
                        <linearGradient id="sky">
                            <stop offset="0" stop-color="red"/>
                            <stop offset="1" stop-color="blue"/>
                        </linearGradient>
                    </defs>
                    <rect x="2" y="0" width="6" height="2" fill="url(#sky)"/>
                </svg>
            "#,
        )
        .unwrap();

        let surface = document.render(10, 2, Color::TRANSPARENT).unwrap();

        assert_eq!(surface.pixel(1, 1), Some(Color::TRANSPARENT));

        let left = surface.pixel(3, 1).unwrap();
        let right = surface.pixel(7, 1).unwrap();

        assert!(left.red > left.blue);
        assert!(right.blue > right.red);
    }

    #[test]
    fn inherits_linear_gradient_definitions() {
        let document = parse_svg(
            r##"
                <svg width="10" height="2">
                    <defs>
                        <linearGradient id="base" x1="0" y1="0" x2="1" y2="0">
                            <stop offset="0" stop-color="red"/>
                            <stop offset="1" stop-color="blue"/>
                        </linearGradient>
                        <linearGradient id="derived" href="#base"/>
                    </defs>
                    <rect width="10" height="2" fill="url(#derived)"/>
                </svg>
            "##,
        )
        .unwrap();

        let surface = document.render(10, 2, Color::TRANSPARENT).unwrap();

        let left = surface.pixel(1, 1).unwrap();
        let right = surface.pixel(8, 1).unwrap();

        assert!(left.red > left.blue);
        assert!(right.blue > right.red);
    }
}
