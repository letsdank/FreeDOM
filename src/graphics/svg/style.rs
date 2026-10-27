//! SVG presentation attributes and stylesheet declarations.
//!
//! Specification: <https://www.w3.org/TR/SVG2/styling.html>

use super::{
    HtmlElement, HtmlNode, SvgError, SvgStyle, attribute, parse_fill_rule, parse_opacity,
    parse_paint, parse_stroke_dash_array, parse_stroke_dash_offset, parse_stroke_line_cap,
    parse_stroke_line_join, parse_stroke_miter_limit, parse_stroke_width,
};
use crate::css::{
    CascadeRule, Element as CssElement, matching_declarations, parse_declaration_list,
};

pub(super) fn resolve_style(
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

pub(super) fn collect_svg_stylesheets(root: &HtmlElement) -> Vec<String> {
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
