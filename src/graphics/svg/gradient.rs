//! SVG linear gradient resolution.
//!
//! Specification: <https://www.w3.org/TR/SVG2/pservers.html>

use super::{
    HtmlElement, HtmlNode, Path, Point, SvgError, Transform, Url, apply_opacity, attribute,
    find_element_by_id, local_reference_id, parse_declaration_list, parse_number_list,
    parse_opacity, parse_solid_color, parse_transform_list,
};

use crate::graphics::raster::{Color, GradientSpread, GradientStop, LinearGradient};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GradientUnits {
    ObjectBoundingBox,
    UserSpaceOnUse,
}

pub(super) fn resolve_linear_gradient(
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
    let spread = inherited_gradient_spread(&gradients)?;

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

    let Some(mut gradient) = LinearGradient::with_spread(
        gradient_transform.apply_point(start),
        gradient_transform.apply_point(end),
        stops,
        spread,
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

fn inherited_gradient_spread(gradients: &[&HtmlElement]) -> Result<GradientSpread, SvgError> {
    match inherited_gradient_attribute(gradients, "spreadmethod") {
        None => Ok(GradientSpread::Pad),

        Some(value) if value.trim().eq_ignore_ascii_case("pad") => Ok(GradientSpread::Pad),

        Some(value) if value.trim().eq_ignore_ascii_case("repeat") => Ok(GradientSpread::Repeat),

        Some(value) if value.trim().eq_ignore_ascii_case("reflect") => Ok(GradientSpread::Reflect),

        Some(_) => Err(SvgError::new("unsupported gradient spreadMethod value")),
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
        let mut declarations = parse_declaration_list(style);
        declarations.sort_by_key(|declaration| declaration.important);

        for declaration in declarations {
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
