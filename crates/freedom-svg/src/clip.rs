//! SVG clip-path reference and geometry resolution.
//!
//! Specification: <https://www.w3.org/TR/css-masking-1/#svg-clipping-path>

use super::{
    ClipPath, FillRule, HtmlElement, HtmlNode, Path, SvgError, Transform, Url, attribute,
    element_transform, find_element_by_id, local_reference_id, number_attribute, parse_fill_rule,
    path_for_element, use_reference_id,
};

pub(super) fn resolve_clip_paths(
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
