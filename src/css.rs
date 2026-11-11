//! CSS rules, declarations, and style computation.
//!
//! Syntax: <https://www.w3.org/TR/css-syntax-3/>
//! Cascade: <https://www.w3.org/TR/css-cascade-4/>

mod color;
mod comments;
mod ident;
mod input;
mod specificity;
mod syntax;

use self::color::parse_color;
use self::comments::remove_css_comments;
use self::ident::consume_identifier;
pub(crate) use self::input::preprocess_input;
use self::specificity::Specificity;
pub(crate) use self::syntax::parse_declaration_list;

// Temporary light-theme CanvasText until system colors are implemented.
const DEFAULT_TEXT_COLOR: u32 = 0x00_00_00_00;
const DEFAULT_LINK_COLOR: u32 = 0x00_00_00_EE;

// https://www.w3.org/TR/css-fonts-4/#font-size-prop
const RELATIVE_FONT_SIZE_SCALE: f32 = 1.2;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Display {
    Inline,
    Block,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FontWeight {
    Normal,
    Bold,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FontStyle {
    Normal,
    Italic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CssSize {
    Auto,
    Pixels(i32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BorderEdge {
    pub(crate) width: i32,
    pub(crate) color: u32,
}

#[derive(Clone)]
pub(crate) struct TextStyle {
    pub(crate) color: u32,
    pub(crate) font_size: f32,
    pub(crate) font_weight: FontWeight,
    pub(crate) font_style: FontStyle,
    pub(crate) font_family: Option<String>,
    pub(crate) margin_before: i32,
    pub(crate) margin_after: i32,
    pub(crate) display: Display,
    pub(crate) width: CssSize,
    pub(crate) height: CssSize,
    pub(crate) background_color: BackgroundColor,
    pub(crate) border_top: BorderEdge,
    pub(crate) border_bottom: BorderEdge,
}

impl TextStyle {
    pub(crate) fn initial() -> Self {
        Self {
            color: DEFAULT_TEXT_COLOR,
            font_size: 16.0,
            font_weight: FontWeight::Normal,
            font_family: None,
            font_style: FontStyle::Normal,
            margin_before: 0,
            margin_after: 0,
            display: Display::Inline,
            width: CssSize::Auto,
            height: CssSize::Auto,
            background_color: BackgroundColor::Transparent,
            border_top: BorderEdge::default(),
            border_bottom: BorderEdge::default(),
        }
    }
}

#[derive(Clone, Copy)]
enum FontSize {
    Pixels(f32),
    Em(f32),
    Percentage(f32),
    Inherit,
    Initial,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextColor {
    Value(u32),
    Inherit,
    Initial,
}

#[derive(Clone, Default)]
struct Declarations {
    color: Option<TextColor>,
    font_size: Option<FontSize>,
    font_weight: Option<FontWeight>,
    font_family: Option<String>,
    font_style: Option<FontStyle>,
    display: Option<Display>,
    margin_before: Option<i32>,
    margin_after: Option<i32>,
    width: Option<CssSize>,
    height: Option<CssSize>,
    background_color: Option<BackgroundColor>,
    border_top: Option<BorderEdge>,
    border_bottom: Option<BorderEdge>,
}

pub(crate) struct CssRule {
    selector: Selector,
    specificity: Specificity,
    source_order: usize,
    declarations: Vec<CssDeclaration>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackgroundColor {
    Transparent,
    Opaque(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CssDeclaration {
    pub(crate) name: String,
    pub(crate) value: String,
    pub(crate) important: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FontFaceRule {
    pub(crate) family: String,
    pub(crate) sources: Vec<String>,
    pub(crate) font_weight: FontWeight,
    pub(crate) font_style: FontStyle,
}

pub(crate) struct CascadeRule {
    selector: Selector,
    specificity: Specificity,
    source_order: usize,
    declarations: Vec<CssDeclaration>,
}

#[derive(Clone, Default)]
struct Selector {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
    ancestors: Vec<Selector>,
}

pub(crate) fn build_author_rules(stylesheets: &[String]) -> Vec<CssRule> {
    build_cascade_rules(stylesheets)
        .into_iter()
        .map(|rule| CssRule {
            selector: rule.selector,
            specificity: rule.specificity,
            source_order: rule.source_order,
            declarations: rule.declarations,
        })
        .collect()
}

pub(crate) fn build_cascade_rules(stylesheets: &[String]) -> Vec<CascadeRule> {
    let mut rules = Vec::new();
    let mut source_order = 0;

    for stylesheet in stylesheets {
        parse_stylesheet(stylesheet, &mut rules, &mut source_order);
    }

    rules
}

pub(crate) fn parse_font_faces(stylesheets: &[String]) -> Vec<FontFaceRule> {
    stylesheets
        .iter()
        .flat_map(|stylesheet| parse_font_faces_in_stylesheet(stylesheet))
        .collect()
}

fn parse_font_faces_in_stylesheet(source: &str) -> Vec<FontFaceRule> {
    let source = remove_css_comments(&preprocess_input(source));
    let mut faces = Vec::new();

    for rule_text in source.split('}') {
        let Some((selector_text, declaration_text)) = rule_text.split_once('{') else {
            continue;
        };

        if !selector_text.trim().eq_ignore_ascii_case("@font-face") {
            continue;
        }

        let declarations = parse_declaration_list(declaration_text);

        let Some(family) =
            declaration_value(&declarations, "font-family").and_then(parse_font_face_family)
        else {
            continue;
        };

        let sources = declaration_value(&declarations, "src")
            .map(parse_font_face_sources)
            .unwrap_or_default();

        if sources.is_empty() {
            continue;
        }

        let font_weight = declaration_value(&declarations, "font-weight")
            .and_then(parse_font_weight)
            .unwrap_or(FontWeight::Normal);

        let font_style = declaration_value(&declarations, "font-style")
            .and_then(parse_font_style)
            .unwrap_or(FontStyle::Normal);

        faces.push(FontFaceRule {
            family,
            sources,
            font_weight,
            font_style,
        });
    }

    faces
}

fn declaration_value<'a>(declarations: &'a [CssDeclaration], name: &str) -> Option<&'a str> {
    declarations
        .iter()
        .rev()
        .find(|declaration| declaration.name == name && !declaration.important)
        .map(|declaration| declaration.value.as_str())
}

fn parse_font_face_family(value: &str) -> Option<String> {
    let value = value.trim();

    let (value, quoted) = match (value.as_bytes().first(), value.as_bytes().last()) {
        (Some(b'"'), Some(b'"')) | (Some(b'\''), Some(b'\'')) if value.len() >= 2 => {
            (&value[1..value.len() - 1], true)
        }
        _ => (value, false),
    };

    (!value.is_empty() && (quoted || !value.contains(','))).then(|| value.to_owned())
}

fn parse_text_font_family(value: &str) -> Option<String> {
    let mut quote = None;
    let mut escaped = false;
    let mut end = value.len();

    for (index, character) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }

        if quote.is_some() && character == '\\' {
            escaped = true;
        } else if quote == Some(character) {
            quote = None;
        } else if quote.is_none() && matches!(character, '\'' | '"') {
            quote = Some(character);
        } else if quote.is_none() && character == ',' {
            end = index;
            break;
        }
    }

    if quote.is_some() {
        return None;
    }

    let first_family = value[..end].trim();

    parse_font_face_family(first_family)
}

fn parse_font_face_sources(value: &str) -> Vec<String> {
    let mut sources = Vec::new();
    let mut remaining = value;

    loop {
        let lowercase = remaining.to_ascii_lowercase();
        let Some(position) = lowercase.find("url(") else {
            break;
        };

        let after_opening_parenthesis = &remaining[position + 4..];

        let Some((url, tail)) = take_font_face_url(after_opening_parenthesis) else {
            break;
        };

        if !url.is_empty() {
            sources.push(url.to_owned());
        }

        remaining = tail;
    }

    sources
}

fn take_font_face_url(input: &str) -> Option<(&str, &str)> {
    let input = input.trim_start();
    let first = *input.as_bytes().first()?;

    if first == b'"' || first == b'\'' {
        let quote = first as char;
        let content = &input[1..];
        let closing_quote = content.find(quote)?;
        let url = &content[..closing_quote];
        let after_quote = content[closing_quote + 1..].trim_start();
        let tail = after_quote.strip_prefix(')')?;

        return Some((url, tail));
    }

    let closing_parenthesis = input.find(')')?;
    let url = input[..closing_parenthesis].trim();

    Some((url, &input[closing_parenthesis + 1..]))
}

fn parse_stylesheet(source: &str, rules: &mut Vec<CascadeRule>, source_order: &mut usize) {
    let source = remove_css_comments(&preprocess_input(source));

    for rule_text in source.split('}') {
        let Some((selector_text, declaration_text)) = rule_text.split_once('{') else {
            continue;
        };

        let declarations = parse_declaration_list(declaration_text);

        let selectors = syntax::split_top_level_commas(selector_text)
            .into_iter()
            .map(parse_selector)
            .collect::<Option<Vec<_>>>();

        let Some(selectors) = selectors else {
            continue;
        };

        for (selector, specificity) in selectors {
            rules.push(CascadeRule {
                selector,
                specificity,
                source_order: *source_order,
                declarations: declarations.clone(),
            });

            *source_order += 1;
        }
    }
}

fn parse_selector(input: &str) -> Option<(Selector, Specificity)> {
    let mut remaining = input.trim_start_matches(is_css_whitespace);

    if remaining.is_empty() {
        return None;
    }

    let mut specificity = Specificity::default();
    let mut compounds = Vec::new();

    loop {
        let (compound, tail) = parse_compound_selector(remaining, &mut specificity)?;

        compounds.push(compound);

        if tail.is_empty() {
            break;
        }

        if !tail.chars().next().is_some_and(is_css_whitespace) {
            return None;
        }

        remaining = tail.trim_start_matches(is_css_whitespace);

        if remaining.is_empty() {
            break;
        }
    }

    let mut selector = compounds.pop()?;
    selector.ancestors = compounds;

    Some((selector, specificity))
}

fn is_css_whitespace(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\n' | '\r' | '\x0c')
}

fn parse_compound_selector<'a>(
    input: &'a str,
    specificity: &mut Specificity,
) -> Option<(Selector, &'a str)> {
    let mut selector = Selector::default();
    let mut remaining = input;
    let mut saw_universal = false;

    while !remaining.is_empty() {
        if remaining.chars().next().is_some_and(is_css_whitespace) {
            break;
        }

        match remaining.as_bytes()[0] {
            b'*' => {
                if saw_universal
                    || selector.tag.is_some()
                    || selector.id.is_some()
                    || !selector.classes.is_empty()
                {
                    return None;
                }

                saw_universal = true;
                remaining = &remaining[1..];
            }

            b'#' => {
                let (id, tail) = take_css_identifier(&remaining[1..])?;

                if selector.id.is_some() {
                    return None;
                }

                selector.id = Some(id);
                specificity.add_id();
                remaining = tail;
            }

            b'.' => {
                let (class, tail) = take_css_identifier(&remaining[1..])?;

                selector.classes.push(class);
                specificity.add_class();
                remaining = tail;
            }

            _ => {
                if selector.tag.is_some() || saw_universal {
                    return None;
                }

                let (tag, tail) = take_css_identifier(remaining)?;

                selector.tag = Some(tag.to_ascii_lowercase());
                specificity.add_type();
                remaining = tail;
            }
        }
    }

    (saw_universal
        || selector.tag.is_some()
        || selector.id.is_some()
        || !selector.classes.is_empty())
    .then_some((selector, remaining))
}

fn take_css_identifier(input: &str) -> Option<(String, &str)> {
    consume_identifier(input)
}

fn parse_text_declarations(declarations: &[CssDeclaration]) -> Declarations {
    let mut parsed = Declarations::default();

    for declaration in declarations {
        match declaration.name.as_str() {
            "color" => {
                if let Some(value) = parse_text_color(&declaration.value) {
                    parsed.color = Some(value);
                }
            }

            "font-size" => {
                if let Some(value) = parse_font_size(&declaration.value) {
                    parsed.font_size = Some(value);
                }
            }

            "font-weight" => {
                if let Some(value) = parse_font_weight(&declaration.value) {
                    parsed.font_weight = Some(value);
                }
            }

            "font-family" => {
                if let Some(value) = parse_text_font_family(&declaration.value) {
                    parsed.font_family = Some(value);
                }
            }

            "font-style" => {
                if let Some(value) = parse_font_style(&declaration.value) {
                    parsed.font_style = Some(value);
                }
            }

            "display" => {
                let value = match declaration.value.to_ascii_lowercase().as_str() {
                    "none" => Some(Display::None),
                    "block" => Some(Display::Block),
                    "inline" => Some(Display::Inline),
                    _ => None,
                };

                if let Some(value) = value {
                    parsed.display = Some(value);
                }
            }

            "margin-top" => {
                if let Some(value) = parse_margin_length(&declaration.value) {
                    parsed.margin_before = Some(value);
                }
            }

            "margin-bottom" => {
                if let Some(value) = parse_margin_length(&declaration.value) {
                    parsed.margin_after = Some(value);
                }
            }

            "margin" => {
                if let Some((top, bottom)) = parse_vertical_margin_shorthand(&declaration.value) {
                    parsed.margin_before = Some(top);
                    parsed.margin_after = Some(bottom);
                }
            }

            "width" => {
                if let Some(value) = parse_size(&declaration.value) {
                    parsed.width = Some(value);
                }
            }

            "height" => {
                if let Some(value) = parse_size(&declaration.value) {
                    parsed.height = Some(value);
                }
            }

            "background" | "background-color" => {
                if let Some(value) = parse_background_color(&declaration.value) {
                    parsed.background_color = Some(value);
                }
            }

            "border-top" => {
                if let Some(value) = parse_solid_border(&declaration.value) {
                    parsed.border_top = Some(value);
                }
            }

            "border-bottom" => {
                if let Some(value) = parse_solid_border(&declaration.value) {
                    parsed.border_bottom = Some(value);
                }
            }

            _ => {}
        }
    }

    parsed
}

fn parse_text_color(value: &str) -> Option<TextColor> {
    match value.trim().to_ascii_lowercase().as_str() {
        "inherit" | "unset" | "currentcolor" => Some(TextColor::Inherit),
        "initial" => Some(TextColor::Initial),
        _ => parse_color(value).map(TextColor::Value),
    }
}

// Only a solid opaque color or transparent is supported in this preview.
// https://www.w3.org/TR/css-backgrounds-3/#background-color
fn parse_background_color(value: &str) -> Option<BackgroundColor> {
    if value.trim().eq_ignore_ascii_case("transparent") {
        Some(BackgroundColor::Transparent)
    } else {
        parse_color(value).map(BackgroundColor::Opaque)
    }
}

// Partial border shorthand: solid, non-negative integer px, opaque color.
// https://www.w3.org/TR/css-backgrounds-3/#the-border-shorthands
fn parse_solid_border(value: &str) -> Option<BorderEdge> {
    let value = value.trim().to_ascii_lowercase();

    if value == "none" {
        return Some(BorderEdge::default());
    }

    let mut width = None;
    let mut color = None;
    let mut solid = false;

    for component in value.split_ascii_whitespace() {
        if component == "solid" {
            if solid {
                return None;
            }

            solid = true;
        } else if let Some(CssSize::Pixels(value)) = parse_size(component) {
            if width.replace(value).is_some() {
                return None;
            }
        } else if let Some(value) = parse_color(component) {
            if color.replace(value).is_some() {
                return None;
            }
        } else {
            return None;
        }
    }

    if !solid {
        return None;
    }

    Some(BorderEdge {
        width: width?,
        color: color?,
    })
}

fn parse_font_size(value: &str) -> Option<FontSize> {
    let value = value.trim().to_ascii_lowercase();

    // font-size is inherited, so unset behaves like inherit.
    // https://www.w3.org/TR/css-cascade-5/#defaulting-keywords
    match value.as_str() {
        "inherit" | "unset" => return Some(FontSize::Inherit),
        "initial" => return Some(FontSize::Initial),

        // FreeDOM currently uses a 16px medium size.
        // https://www.w3.org/TR/css-fonts-4/#absolute-size-mapping
        "xx-small" => return Some(FontSize::Pixels(9.0)),
        "x-small" => return Some(FontSize::Pixels(10.0)),
        "small" => return Some(FontSize::Pixels(13.0)),
        "medium" => return Some(FontSize::Pixels(16.0)),
        "large" => return Some(FontSize::Pixels(18.0)),
        "x-large" => return Some(FontSize::Pixels(24.0)),
        "xx-large" => return Some(FontSize::Pixels(32.0)),
        "xxx-large" => return Some(FontSize::Pixels(48.0)),

        "larger" => return Some(FontSize::Em(RELATIVE_FONT_SIZE_SCALE)),
        "smaller" => return Some(FontSize::Em(1.0 / RELATIVE_FONT_SIZE_SCALE)),

        _ => {}
    }

    if value.parse::<f32>().ok() == Some(0.0) {
        return Some(FontSize::Pixels(0.0));
    }

    if let Some(value) = value.strip_suffix("px") {
        let pixels = value.trim().parse::<f32>().ok()?;
        return (pixels.is_finite() && pixels >= 0.0).then_some(FontSize::Pixels(pixels));
    }

    if let Some(value) = value.strip_suffix("em") {
        let em = value.trim().parse::<f32>().ok()?;
        return (em.is_finite() && em >= 0.0).then_some(FontSize::Em(em));
    }

    if let Some(value) = value.strip_suffix('%') {
        let percent = value.parse::<f32>().ok()?;

        return (percent.is_finite() && percent >= 0.0).then_some(FontSize::Percentage(percent));
    }

    None
}

fn parse_font_weight(value: &str) -> Option<FontWeight> {
    match value.trim().to_ascii_lowercase().as_str() {
        "normal" => Some(FontWeight::Normal),
        "bold" => Some(FontWeight::Bold),
        value => {
            let weight = value.parse::<u16>().ok()?;

            match weight {
                1..=500 => Some(FontWeight::Normal),
                501..=1000 => Some(FontWeight::Bold),
                _ => None,
            }
        }
    }
}

fn parse_font_style(value: &str) -> Option<FontStyle> {
    match value.trim().to_ascii_lowercase().as_str() {
        "normal" => Some(FontStyle::Normal),
        "italic" | "oblique" => Some(FontStyle::Italic),
        _ => None,
    }
}

fn parse_margin_length(value: &str) -> Option<i32> {
    let value = value.trim().to_ascii_lowercase();

    // CSS absolute units: one CSS inch equals 96 CSS pixels.
    // https://www.w3.org/TR/css-values-4/#absolute-lengths
    let (number, scale, has_unit) = if let Some(number) = value.strip_suffix("px") {
        (number, 1.0, true)
    } else if let Some(number) = value.strip_suffix("in") {
        (number, 96.0, true)
    } else {
        (value.as_str(), 1.0, false)
    };

    let number = number.parse::<f64>().ok()?;

    if !number.is_finite() || (!has_unit && number != 0.0) {
        return None;
    }

    let rounded = (number * scale).round();

    if !rounded.is_finite() || rounded < f64::from(i32::MIN) || rounded > f64::from(i32::MAX) {
        return None;
    }

    Some(rounded as i32)
}

// CSS 2.2 sections 10.2 and 10.5:
// https://www.w3.org/TR/CSS22/visudet.html#the-width-property
// https://www.w3.org/TR/CSS22/visudet.html#the-height-property
//
// This subset accepts auto, non-negative integer px, and unitless zero.
fn parse_size(value: &str) -> Option<CssSize> {
    let value = value.trim().to_ascii_lowercase();

    if value == "auto" {
        return Some(CssSize::Auto);
    }

    let (number, has_unit) = match value.strip_suffix("px") {
        Some(number) => (number, true),
        None => (value.as_str(), false),
    };

    let number = number.parse::<i32>().ok()?;

    if number < 0 || (!has_unit && number != 0) {
        return None;
    }

    Some(CssSize::Pixels(number))
}

// CSS 2.2 section 8.3:
// https://www.w3.org/TR/CSS22/box.html#margin-properties
//
// Only px, in and unitless zero are supported. Every component is validated.
fn parse_vertical_margin_shorthand(value: &str) -> Option<(i32, i32)> {
    let components = value
        .split_ascii_whitespace()
        .map(parse_margin_length)
        .collect::<Option<Vec<_>>>()?;

    match components.as_slice() {
        [top] | [top, _] => Some((*top, *top)),

        [top, _, bottom] | [top, _, bottom, _] => Some((*top, *bottom)),

        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn compute_style(
    element: &Element<'_>,
    inherited_style: TextStyle,
    rules: &[CssRule],
) -> TextStyle {
    compute_style_with_ancestors(element, inherited_style, rules, &[])
}

/// Ancestors are ordered from the document root to the element's parent.
pub(crate) fn compute_style_with_ancestors(
    element: &Element<'_>,
    inherited_style: TextStyle,
    rules: &[CssRule],
    ancestors: &[Element<'_>],
) -> TextStyle {
    let parent_font_size = inherited_style.font_size;
    let parent_color = inherited_style.color;
    let mut style = default_style(&element.tag, inherited_style);

    let mut matching_rules: Vec<&CssRule> = rules
        .iter()
        .filter(|rule| selector_matches_with_ancestors(&rule.selector, element, ancestors))
        .collect();

    matching_rules.sort_by_key(|rule| (rule.specificity, rule.source_order));

    let inline_declarations = element
        .inline_style
        .map(parse_declaration_list)
        .unwrap_or_default();

    let mut declarations: Vec<&CssDeclaration> = matching_rules
        .into_iter()
        .flat_map(|rule| &rule.declarations)
        .collect();

    declarations.extend(inline_declarations.iter());

    // Stable sorting keeps specificity, source order, and inline precedence
    // within each importance group.
    declarations.sort_by_key(|declaration| declaration.important);

    for declaration in declarations {
        let parsed = parse_text_declarations(std::slice::from_ref(declaration));

        apply_declarations(&mut style, &parsed, parent_font_size, parent_color);
    }

    style
}

fn default_style(tag: &str, inherited_style: TextStyle) -> TextStyle {
    let mut style = inherited_style;
    style.display = Display::Inline;
    style.margin_before = 0;
    style.margin_after = 0;
    style.width = CssSize::Auto;
    style.height = CssSize::Auto;
    style.background_color = BackgroundColor::Transparent;
    style.border_top = BorderEdge::default();
    style.border_bottom = BorderEdge::default();

    if tag == "a" {
        style.color = DEFAULT_LINK_COLOR;
    }

    if matches!(
        tag,
        "html"
            | "body"
            | "main"
            | "article"
            | "section"
            | "div"
            | "p"
            | "ul"
            | "ol"
            | "li"
            | "h1"
            | "h2"
            | "h3"
            | "pre"
            | "svg"
    ) {
        style.display = Display::Block;
    }

    match tag {
        "h1" => {
            style.font_size = 32.0;
            style.margin_after = 16;
        }

        "h2" | "h3" => {
            style.font_size = 16.0;
            style.margin_after = 12;
        }

        "p" | "li" | "pre" => style.margin_after = 10,

        _ => {}
    }

    style
}

fn selector_matches(selector: &Selector, element: &Element<'_>) -> bool {
    selector_matches_with_ancestors(selector, element, &[])
}

// https://www.w3.org/TR/selectors-4/#descendant-combinators
fn selector_matches_with_ancestors(
    selector: &Selector,
    element: &Element<'_>,
    ancestors: &[Element<'_>],
) -> bool {
    if !compound_matches(selector, element) {
        return false;
    }

    let mut candidates = ancestors.iter().rev();

    for part in selector.ancestors.iter().rev() {
        if !candidates.any(|ancestor| compound_matches(part, ancestor)) {
            return false;
        }
    }

    true
}

fn compound_matches(selector: &Selector, element: &Element<'_>) -> bool {
    let tag_matches = selector
        .tag
        .as_ref()
        .map_or(true, |tag| element.tag == *tag);

    let id_matches = selector
        .id
        .as_ref()
        .is_none_or(|id| element.id == Some(id.as_str()));

    let classes_match = selector.classes.iter().all(|class| {
        element
            .classes
            .iter()
            .any(|element_class| element_class == class)
    });

    tag_matches && id_matches && classes_match
}

pub(crate) fn matching_declarations<'a>(
    rules: &'a [CascadeRule],
    element: &Element<'_>,
) -> Vec<&'a CssDeclaration> {
    let mut matching_rules: Vec<&CascadeRule> = rules
        .iter()
        .filter(|rule| selector_matches(&rule.selector, element))
        .collect();

    matching_rules.sort_by_key(|rule| (rule.specificity, rule.source_order));

    let mut declarations: Vec<_> = matching_rules
        .into_iter()
        .flat_map(|rule| rule.declarations.iter())
        .collect();

    declarations.sort_by_key(|declaration| declaration.important);

    declarations
}

fn apply_declarations(
    style: &mut TextStyle,
    declarations: &Declarations,
    parent_font_size: f32,
    parent_color: u32,
) {
    if let Some(color) = declarations.color {
        style.color = match color {
            TextColor::Value(value) => value,
            TextColor::Inherit => parent_color,
            TextColor::Initial => TextStyle::initial().color,
        };
    }

    if let Some(font_size) = declarations.font_size {
        style.font_size = match font_size {
            FontSize::Pixels(pixels) => pixels,
            FontSize::Em(em) => parent_font_size * em,
            FontSize::Percentage(percent) => parent_font_size * (percent / 100.0),
            FontSize::Inherit => parent_font_size,
            FontSize::Initial => TextStyle::initial().font_size,
        };
    }

    if let Some(display) = declarations.display {
        style.display = display;
    }

    if let Some(margin_before) = declarations.margin_before {
        style.margin_before = margin_before;
    }

    if let Some(margin_after) = declarations.margin_after {
        style.margin_after = margin_after;
    }

    if let Some(width) = declarations.width {
        style.width = width;
    }

    if let Some(height) = declarations.height {
        style.height = height;
    }

    if let Some(background_color) = declarations.background_color {
        style.background_color = background_color;
    }

    if let Some(border) = declarations.border_top {
        style.border_top = border;
    }

    if let Some(border) = declarations.border_bottom {
        style.border_bottom = border;
    }

    if let Some(font_weight) = declarations.font_weight {
        style.font_weight = font_weight;
    }

    if let Some(font_style) = declarations.font_style {
        style.font_style = font_style;
    }

    if let Some(font_family) = &declarations.font_family {
        style.font_family = Some(font_family.clone())
    }
}

pub(crate) struct Element<'a> {
    pub(crate) tag: &'a str,
    pub(crate) id: Option<&'a str>,
    pub(crate) classes: &'a [String],
    pub(crate) inline_style: Option<&'a str>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_css_font_weights() {
        assert_eq!(parse_font_weight("normal"), Some(FontWeight::Normal));
        assert_eq!(parse_font_weight("400"), Some(FontWeight::Normal));
        assert_eq!(parse_font_weight("bold"), Some(FontWeight::Bold));
        assert_eq!(parse_font_weight("700"), Some(FontWeight::Bold));
        assert_eq!(parse_font_weight("1200"), None);
    }

    #[test]
    fn parses_css_font_styles() {
        assert_eq!(parse_font_style("normal"), Some(FontStyle::Normal));
        assert_eq!(parse_font_style("italic"), Some(FontStyle::Italic));
        assert_eq!(parse_font_style("oblique"), Some(FontStyle::Italic));
        assert_eq!(parse_font_style("oblique 12deg"), None);
    }

    #[test]
    fn parses_compound_css_selectors_with_correct_specificity() {
        let (selector, specificity) = parse_selector("p#lead.notice.wide").unwrap();

        assert_eq!(selector.tag.as_deref(), Some("p"));
        assert_eq!(selector.id.as_deref(), Some("lead"));
        assert_eq!(selector.classes, ["notice", "wide"]);
        assert_eq!(specificity, Specificity::new(1, 2, 1));

        let classes = vec!["notice".to_owned(), "wide".to_owned()];

        let element = Element {
            tag: "p",
            id: Some("lead"),
            classes: &classes,
            inline_style: None,
        };

        assert!(compound_matches(&selector, &element));
    }

    #[test]
    fn exposes_cascaded_declarations_for_other_renderers() {
        let stylesheets =
            vec!["line { stroke: red; } #target { stroke: blue; stroke-width: 4; }".to_owned()];
        let rules = build_cascade_rules(&stylesheets);
        let classes = Vec::new();

        let element = Element {
            tag: "line",
            id: Some("target"),
            classes: &classes,
            inline_style: None,
        };

        let declarations = matching_declarations(&rules, &element)
            .into_iter()
            .map(|declaration| (declaration.name.as_str(), declaration.value.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(
            declarations,
            vec![("stroke", "red"), ("stroke", "blue"), ("stroke-width", "4")]
        );
    }

    #[test]
    fn parses_font_face_woff_sources() {
        let stylesheets = [r#"
            @font-face {
                font-family: "FreeDOM Sans";
                src: local("FreeDOM Sans"),
                     url("fonts/freedom.woff") format("woff"),
                     url('fonts/fallback.ttf') format("truetype");
                font-weight: bold;
                font-style: italic;
            }
        "#
        .to_owned()];

        let faces = parse_font_faces(&stylesheets);

        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0].family, "FreeDOM Sans");
        assert_eq!(
            faces[0].sources,
            vec![
                "fonts/freedom.woff".to_owned(),
                "fonts/fallback.ttf".to_owned(),
            ]
        );
        assert_eq!(faces[0].font_weight, FontWeight::Bold);
        assert_eq!(faces[0].font_style, FontStyle::Italic);
    }

    #[test]
    fn cascades_the_first_font_family_name() {
        let stylesheets = vec![r#"p { font-family: "FreeDOM Sans", sans-serif; }"#.to_owned()];
        let rules = build_author_rules(&stylesheets);
        let classes = Vec::new();

        let element = Element {
            tag: "p",
            id: None,
            classes: &classes,
            inline_style: None,
        };

        let style = compute_style(&element, TextStyle::initial(), &rules);

        assert_eq!(style.font_family.as_deref(), Some("FreeDOM Sans"));
    }

    #[test]
    fn quoted_family_name_can_contain_a_comma() {
        assert_eq!(
            parse_text_font_family(r#""Nko, Test", sans-serif"#),
            Some("Nko, Test".to_owned())
        );

        assert_eq!(
            parse_font_face_family(r#""Nko, Test""#),
            Some("Nko, Test".to_owned())
        );

        assert_eq!(parse_text_font_family(r#""Nko, Test"#), None);
    }

    #[test]
    fn invalid_declarations_do_not_erase_earlier_valid_values() {
        let declarations = parse_declaration_list(
            "color: green; color: not-a-color; \
            font-weight: bold; font-weight: nope;\
            display: block; display: nope",
        );

        let parsed = parse_text_declarations(&declarations);

        assert_eq!(parsed.color, Some(TextColor::Value(0x00_00_80_00)));
        assert_eq!(parsed.font_weight, Some(FontWeight::Bold));
        assert!(matches!(parsed.display, Some(Display::Block)));
    }

    #[test]
    fn stylesheet_preserves_comments_in_quoted_font_family() {
        let stylesheets = ["/* note */ p { font-family: 'A/*B*/C'; color: green; }".to_owned()];

        let rules = build_author_rules(&stylesheets);

        let element = Element {
            tag: "p",
            id: None,
            classes: &[],
            inline_style: None,
        };

        let style = compute_style(&element, TextStyle::initial(), &rules);

        assert_eq!(style.font_family.as_deref(), Some("A/*B*/C"));
        assert_eq!(style.color, 0x00_00_80_00);
    }

    #[test]
    fn selector_matches_non_ascii_class_and_rejects_digit_starts() {
        let stylesheets = [".💡 { color: green } .123 { color: red }".to_owned()];

        let rules = build_author_rules(&stylesheets);

        assert_eq!(rules.len(), 1);

        let classes = vec!["💡".to_owned()];

        let element = Element {
            tag: "p",
            id: None,
            classes: &classes,
            inline_style: None,
        };

        let style = compute_style(&element, TextStyle::initial(), &rules);

        assert_eq!(style.color, 0x00_00_80_00);
    }

    #[test]
    fn selector_decodes_escaped_class_name() {
        let stylesheets = [r".\31 23 { color: green }".to_owned()];
        let rules = build_author_rules(&stylesheets);

        assert_eq!(rules.len(), 1);

        let classes = vec!["123".to_owned()];

        let element = Element {
            tag: "p",
            id: None,
            classes: &classes,
            inline_style: None,
        };

        let style = compute_style(&element, TextStyle::initial(), &rules);

        assert_eq!(style.color, 0x00_00_80_00);
    }

    #[test]
    fn selector_list_with_escaped_comma_matches_both_classes() {
        let stylesheets = [r".hello\,world, .other { color: green }".to_owned()];

        let rules = build_author_rules(&stylesheets);

        assert_eq!(rules.len(), 2);

        for class in ["hello,world", "other"] {
            let classes = vec![class.to_owned()];

            let element = Element {
                tag: "p",
                id: None,
                classes: &classes,
                inline_style: None,
            };

            let style = compute_style(&element, TextStyle::initial(), &rules);

            assert_eq!(style.color, 0x00_00_80_00);
        }
    }

    #[test]
    fn ignores_entire_rule_with_invalid_selector_list() {
        for source in [
            "p,,div { color: red }",
            "p, .123 { color: red }",
            "p, { color: red }",
        ] {
            let rules = build_author_rules(&[source.to_owned()]);

            assert!(rules.is_empty(), "source: {source}");
        }

        let rules = build_author_rules(&["p, div { color: green }".to_owned()]);

        assert_eq!(rules.len(), 2);
    }

    #[test]
    fn id_specificity_exceeds_many_classes() {
        let stylesheets = ["#target { color: green } \
             .x.x.x.x.x.x.x.x.x.x.x { color: red }"
            .to_owned()];

        let rules = build_author_rules(&stylesheets);
        let classes = vec!["x".to_owned()];

        let element = Element {
            tag: "p",
            id: Some("target"),
            classes: &classes,
            inline_style: None,
        };

        let style = compute_style(&element, TextStyle::initial(), &rules);

        assert_eq!(style.color, 0x00_00_80_00);
    }

    #[test]
    fn equal_specificity_respects_source_order() {
        let stylesheets = [".a { color: red } .b { color: green }".to_owned()];

        let rules = build_author_rules(&stylesheets);
        let classes = vec!["a".to_owned(), "b".to_owned()];

        let element = Element {
            tag: "p",
            id: None,
            classes: &classes,
            inline_style: None,
        };

        let style = compute_style(&element, TextStyle::initial(), &rules);

        assert_eq!(style.color, 0x00_00_80_00);
    }

    #[test]
    fn relative_font_sizes_use_parent_not_previous_cascade_values() {
        let stylesheets = ["h1 { font-size: 2em } .lead { font-size: 3em }".to_owned()];

        let rules = build_author_rules(&stylesheets);
        let classes = vec!["lead".to_owned()];

        let element = Element {
            tag: "h1",
            id: None,
            classes: &classes,
            inline_style: Some("font-size: 1.5em"),
        };

        let mut parent_style = TextStyle::initial();
        parent_style.font_size = 20.0;

        let style = compute_style(&element, parent_style.clone(), &rules);

        assert_eq!(style.font_size, 30.0);

        let element_without_inline = Element {
            tag: "h1",
            id: None,
            classes: &classes,
            inline_style: None,
        };

        let style = compute_style(&element_without_inline, parent_style, &rules);

        assert_eq!(style.font_size, 60.0);
    }

    #[test]
    fn important_overrides_normal_rules_and_inline_style() {
        let stylesheets = [
            "p { color: green !important; font-size: 2em !IMPORTANT; color: red; } \
             #target { color: red; font-size: 30px }"
                .to_owned(),
        ];

        let rules = build_author_rules(&stylesheets);

        let element = Element {
            tag: "p",
            id: Some("target"),
            classes: &[],
            inline_style: Some("color: blue; font-size: 40px"),
        };

        let mut parent = TextStyle::initial();
        parent.font_size = 12.0;

        let style = compute_style(&element, parent.clone(), &rules);

        assert_eq!(style.color, 0x00_00_80_00);
        assert_eq!(style.font_size, 24.0);

        let important_inline = Element {
            inline_style: Some("color: blue !important"),
            ..element
        };

        let style = compute_style(&important_inline, parent.clone(), &rules);

        assert_eq!(style.color, 0x00_00_00_FF);
    }

    #[test]
    fn invalid_unicode_hex_color_preserves_previous_value() {
        let element = Element {
            tag: "p",
            id: None,
            classes: &[],
            inline_style: Some("color: green; color: #éa"),
        };

        let style = compute_style(&element, TextStyle::initial(), &[]);

        assert_eq!(style.color, 0x00_00_80_00);
    }

    #[test]
    fn cascades_clamped_rgb_colors() {
        let stylesheets = ["p { color: rgb(-500, 64, 128) }".to_owned()];

        let rules = build_author_rules(&stylesheets);

        let element = Element {
            tag: "p",
            id: None,
            classes: &[],
            inline_style: None,
        };

        let style = compute_style(&element, TextStyle::initial(), &rules);

        assert_eq!(style.color, 0x00_00_40_80);
    }

    #[test]
    fn zero_font_size_is_accepted_and_negative_is_rejected() {
        for value in ["0", "0px", "0em", "-0px"] {
            let inline_style = format!("font-size: 18px; font-size: {value}");

            let element = Element {
                tag: "p",
                id: None,
                classes: &[],
                inline_style: Some(&inline_style),
            };

            let style = compute_style(&element, TextStyle::initial(), &[]);

            assert_eq!(style.font_size, 0.0, "value: {value}");
        }

        let element = Element {
            tag: "p",
            id: None,
            classes: &[],
            inline_style: Some("font-size: 18px; font-size: -1px"),
        };

        let style = compute_style(&element, TextStyle::initial(), &[]);

        assert_eq!(style.font_size, 18.0);
    }

    #[test]
    fn percentage_font_size_uses_parent_and_respects_cascade() {
        let mut parent = TextStyle::initial();
        parent.font_size = 20.0;

        for (value, expected) in [("150%", 30.0), ("25%", 5.0), ("0%", 0.0), ("-0%", 0.0)] {
            let inline_style = format!("font-size: 7px; font-size: {value}");

            let element = Element {
                tag: "p",
                id: None,
                classes: &[],
                inline_style: Some(&inline_style),
            };

            let style = compute_style(&element, parent.clone(), &[]);

            assert_eq!(style.font_size, expected, "value: {value}");
        }

        for value in ["-1%", "nan%", "inf%", "20 %"] {
            let inline_style = format!("font-size: 18px; font-size: {value}");

            let element = Element {
                tag: "p",
                id: None,
                classes: &[],
                inline_style: Some(&inline_style),
            };

            let style = compute_style(&element, parent.clone(), &[]);

            assert_eq!(style.font_size, 18.0, "value: {value}");
        }
    }

    #[test]
    fn font_size_defaulting_keywords_follow_cascade() {
        let mut parent = TextStyle::initial();
        parent.font_size = 24.0;

        for (value, expected) in [("inherit", 24.0), ("unset", 24.0), ("INITIAL", 16.0)] {
            let inline_style = format!("font-size: 30px; font-size: {value}");

            let element = Element {
                tag: "h1",
                id: None,
                classes: &[],
                inline_style: Some(&inline_style),
            };

            let style = compute_style(&element, parent.clone(), &[]);

            assert_eq!(style.font_size, expected, "value: {value}");
        }

        let important = Element {
            tag: "p",
            id: None,
            classes: &[],
            inline_style: Some("font-size: inherit !important; font-size: 30px"),
        };

        let style = compute_style(&important, parent, &[]);

        assert_eq!(style.font_size, 24.0);
    }

    #[test]
    fn absolute_font_size_keywords_do_not_depend_on_parent() {
        let mut parent = TextStyle::initial();
        parent.font_size = 25.0;

        for (keyword, expected) in [
            ("xx-small", 9.0),
            ("x-small", 10.0),
            ("small", 13.0),
            ("medium", 16.0),
            ("large", 18.0),
            ("x-large", 24.0),
            ("xx-large", 32.0),
            ("XXX-LARGE", 48.0),
        ] {
            let inline_style = format!("font-size: 7px; font-size: {keyword}");

            let element = Element {
                tag: "h1",
                id: None,
                classes: &[],
                inline_style: Some(&inline_style),
            };

            let style = compute_style(&element, parent.clone(), &[]);

            assert_eq!(style.font_size, expected, "keyword: {keyword}");
        }
    }

    #[test]
    fn relative_font_size_keywords_use_parent_size() {
        for (parent_size, keyword, expected) in [
            (20.0, "larger", 24.0),
            (24.0, "smaller", 20.0),
            (0.0, "larger", 0.0),
            (20.0, "LARGER", 24.0),
        ] {
            let mut parent = TextStyle::initial();
            parent.font_size = parent_size;

            let inline_style = format!("font-size: 100px; font-size: {keyword}");

            let element = Element {
                tag: "h1",
                id: None,
                classes: &[],
                inline_style: Some(&inline_style),
            };

            let style = compute_style(&element, parent, &[]);

            assert!(
                (style.font_size - expected).abs() < 0.001,
                "keyword: {keyword}, actual: {}",
                style.font_size
            );
        }
    }

    #[test]
    fn color_defaulting_keywords_use_parent_or_initial() {
        let mut parent = TextStyle::initial();
        parent.color = 0x00_FF_00_00;

        for (keyword, expected) in [
            ("currentColor", 0x00_FF_00_00),
            ("inherit", 0x00_FF_00_00),
            ("unset", 0x00_FF_00_00),
            ("initial", TextStyle::initial().color),
        ] {
            let inline_style = format!("color: green; color: {keyword}");

            let element = Element {
                tag: "a",
                id: None,
                classes: &[],
                inline_style: Some(&inline_style),
            };

            let style = compute_style(&element, parent.clone(), &[]);

            assert_eq!(style.color, expected, "keyword: {keyword}");
        }

        let element = Element {
            tag: "p",
            id: None,
            classes: &[],
            inline_style: Some("color: currentcolor !important; color: green"),
        };

        let style = compute_style(&element, parent, &[]);

        assert_eq!(style.color, 0x00_FF_00_00);
    }

    #[test]
    fn parses_vertical_margin_shorthand() {
        assert_eq!(parse_vertical_margin_shorthand("1px"), Some((1, 1)));
        assert_eq!(parse_vertical_margin_shorthand("1px 2px"), Some((1, 1)));
        assert_eq!(parse_vertical_margin_shorthand("1px 2px 3px"), Some((1, 3)));

        assert_eq!(
            parse_vertical_margin_shorthand("-1px 2px 3px 4px"),
            Some((-1, 3))
        );

        assert_eq!(parse_vertical_margin_shorthand("0 2PX"), Some((0, 0)));

        for input in [
            "",
            "1px 2em",
            "1px 2px 3px 4px 5px",
            "3",
            "NaNpx",
            "1px 2px junk",
        ] {
            assert_eq!(parse_vertical_margin_shorthand(input), None, "{input}");
        }
    }

    #[test]
    fn cascades_vertical_margins_and_does_not_inherit_them() {
        let stylesheets = vec![
            "div { margin: 1px 2px 3px 4px; \
             margin-top: 5px !important }"
                .to_owned(),
        ];

        let rules = build_author_rules(&stylesheets);

        let element = Element {
            tag: "div",
            id: None,
            classes: &[],
            inline_style: Some("margin: 6px 7px 8px; margin-bottom: 9px"),
        };

        let style = compute_style(&element, TextStyle::initial(), &rules);

        assert_eq!((style.margin_before, style.margin_after), (5, 9));

        let child = Element {
            tag: "span",
            id: None,
            classes: &[],
            inline_style: None,
        };

        let child_style = compute_style(&child, style, &[]);

        assert_eq!(
            (child_style.margin_before, child_style.margin_after),
            (0, 0)
        );
    }

    #[test]
    fn invalid_margin_shorthand_does_not_override_previous_values() {
        let element = Element {
            tag: "div",
            id: None,
            classes: &[],
            inline_style: Some(
                "margin: 2px 3px 4px; \
                 margin: 8px nope; \
                 margin-top: 1px 2px",
            ),
        };

        let style = compute_style(&element, TextStyle::initial(), &[]);

        assert_eq!((style.margin_before, style.margin_after), (2, 4));
    }

    #[test]
    fn parses_absolute_inches_in_vertical_margins() {
        for (input, expected) in [
            ("2in", Some(192)),
            ("-2in", Some(-192)),
            ("0.5IN", Some(48)),
            ("0", Some(0)),
            ("2px", Some(2)),
            ("2em", None),
            ("2 in", None),
            ("1e308in", None),
        ] {
            assert_eq!(parse_margin_length(input), expected, "{input}");
        }

        assert_eq!(
            parse_vertical_margin_shorthand("1in 0 -0.5in"),
            Some((96, -48))
        );
    }

    #[test]
    fn cascades_inches_in_vertical_margin_declarations() {
        let rules = build_author_rules(&[concat!(
            "#div1 { margin-bottom: 2in } ",
            "#div2 { margin-top: -2in }"
        )
        .to_owned()]);

        let first = Element {
            tag: "div",
            id: Some("div1"),
            classes: &[],
            inline_style: None,
        };

        let second = Element {
            tag: "div",
            id: Some("div2"),
            classes: &[],
            inline_style: None,
        };

        let first_style = compute_style(&first, TextStyle::initial(), &rules);
        let second_style = compute_style(&second, TextStyle::initial(), &rules);

        assert_eq!(first_style.margin_after, 192);
        assert_eq!(second_style.margin_before, -192);
    }

    #[test]
    fn parses_fixed_height_subset() {
        for (value, expected) in [
            ("20px", Some(CssSize::Pixels(20))),
            (" 0PX ", Some(CssSize::Pixels(0))),
            ("0", Some(CssSize::Pixels(0))),
            ("auto", Some(CssSize::Auto)),
            ("-1px", None),
            ("-0.5px", None),
            ("20", None),
            ("20%", None),
            ("0.5px", None),
            ("2147483648px", None),
        ] {
            assert_eq!(parse_size(value), expected, "{value}");
        }
    }

    #[test]
    fn cascades_height_and_resets_it_for_children() {
        let rules = build_author_rules(&["div { height: 20px !important }".to_owned()]);

        let element = Element {
            tag: "div",
            id: None,
            classes: &[],
            inline_style: Some("height: 10px"),
        };

        let style = compute_style(&element, TextStyle::initial(), &rules);

        assert_eq!(style.height, CssSize::Pixels(20));

        let child = Element {
            tag: "span",
            id: None,
            classes: &[],
            inline_style: None,
        };

        let child_style = compute_style(&child, style, &[]);

        assert_eq!(child_style.height, CssSize::Auto);

        let reset = Element {
            tag: "div",
            id: None,
            classes: &[],
            inline_style: Some("height: 20px; height: auto"),
        };

        let reset_style = compute_style(&reset, TextStyle::initial(), &[]);

        assert_eq!(reset_style.height, CssSize::Auto);
    }

    #[test]
    fn cascades_width_and_resets_it_for_children() {
        let rules = build_author_rules(&["div { width: 50px !important }".to_owned()]);

        let element = Element {
            tag: "div",
            id: None,
            classes: &[],
            inline_style: Some("width: 10px"),
        };

        let style = compute_style(&element, TextStyle::initial(), &rules);

        assert_eq!(style.width, CssSize::Pixels(50));

        let child = Element {
            tag: "span",
            id: None,
            classes: &[],
            inline_style: None,
        };

        let child_style = compute_style(&child, style, &[]);

        assert_eq!(child_style.width, CssSize::Auto);

        let reset = Element {
            tag: "div",
            id: None,
            classes: &[],
            inline_style: Some("width: 20px; width: auto"),
        };

        let reset_style = compute_style(&reset, TextStyle::initial(), &[]);

        assert_eq!(reset_style.width, CssSize::Auto);

        for (input, expected) in [
            ("0", Some(CssSize::Pixels(0))),
            ("20PX", Some(CssSize::Pixels(20))),
            ("-1px", None),
            ("20%", None),
        ] {
            assert_eq!(parse_size(input), expected, "{input}");
        }
    }

    #[test]
    fn cascades_background_color_without_inheriting_it() {
        let rules = build_author_rules(&["div { background: orange !important }".to_owned()]);

        let element = Element {
            tag: "div",
            id: None,
            classes: &[],
            inline_style: Some("background-color: blue"),
        };

        let style = compute_style(&element, TextStyle::initial(), &rules);

        assert_eq!(
            style.background_color,
            BackgroundColor::Opaque(0x00_FF_A5_00)
        );

        let child = Element {
            tag: "span",
            id: None,
            classes: &[],
            inline_style: None,
        };

        let child_style = compute_style(&child, style, &[]);

        assert_eq!(child_style.background_color, BackgroundColor::Transparent);

        let clear = Element {
            tag: "div",
            id: None,
            classes: &[],
            inline_style: Some("background: blue; background-color: transparent"),
        };

        let clear_style = compute_style(&clear, TextStyle::initial(), &[]);

        assert_eq!(clear_style.background_color, BackgroundColor::Transparent);
    }

    #[test]
    fn background_preview_supports_only_solid_colors_and_transparent() {
        assert_eq!(
            parse_background_color("#aBc"),
            Some(BackgroundColor::Opaque(0x00_AA_BB_CC))
        );

        assert_eq!(
            parse_background_color("rgb(0, 128, 0)"),
            Some(BackgroundColor::Opaque(0x00_00_80_00))
        );

        assert_eq!(
            parse_background_color("Transparent"),
            Some(BackgroundColor::Transparent)
        );

        assert_eq!(parse_background_color("rgba(0, 0, 0, 0.5)"), None);
        assert_eq!(parse_background_color("url(example.png)"), None);
    }

    #[test]
    fn descendant_selector_matches_ancestors_in_order() {
        let (selector, specificity) = parse_selector("#root .box span").unwrap();

        assert_eq!(specificity, Specificity::new(1, 1, 1));

        let classes = vec!["box".to_owned()];

        let root = Element {
            tag: "main",
            id: Some("root"),
            classes: &[],
            inline_style: None,
        };

        let middle = Element {
            tag: "div",
            id: None,
            classes: &classes,
            inline_style: None,
        };

        let target = Element {
            tag: "span",
            id: None,
            classes: &[],
            inline_style: None,
        };

        assert!(selector_matches_with_ancestors(
            &selector,
            &target,
            &[root, middle]
        ));

        let root = Element {
            tag: "main",
            id: Some("root"),
            classes: &[],
            inline_style: None,
        };

        let middle = Element {
            tag: "div",
            id: None,
            classes: &classes,
            inline_style: None,
        };

        assert!(!selector_matches_with_ancestors(
            &selector,
            &target,
            &[middle, root]
        ));

        let (repeat, _) = parse_selector(".box .box span").unwrap();

        let middle = Element {
            tag: "div",
            id: None,
            classes: &classes,
            inline_style: None,
        };

        assert!(!selector_matches_with_ancestors(
            &repeat,
            &target,
            &[middle]
        ));
    }

    #[test]
    fn descendant_selector_preserves_escapes_and_rejects_other_combinators() {
        let (selector, specificity) = parse_selector(r".\31 23 span").unwrap();

        assert_eq!(specificity, Specificity::new(0, 1, 1));
        assert_eq!(selector.ancestors[0].classes, ["123"]);
        assert_eq!(selector.tag.as_deref(), Some("span"));

        let (selector, _) = parse_selector("#a\u{00a0}").unwrap();

        assert_eq!(selector.id.as_deref(), Some("a\u{00a0}"));

        for invalid in [
            "div > span",
            "div + span",
            "div ~ span",
            "div >",
            "div span::before",
        ] {
            assert!(parse_selector(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn descendant_specificity_participates_in_the_cascade() {
        let rules = build_author_rules(&["#root span { color: blue } \
             .box span { color: red }"
            .to_owned()]);

        let classes = vec!["box".to_owned()];

        let ancestors = [Element {
            tag: "div",
            id: Some("root"),
            classes: &classes,
            inline_style: None,
        }];

        let target = Element {
            tag: "span",
            id: None,
            classes: &[],
            inline_style: None,
        };

        let style = compute_style_with_ancestors(&target, TextStyle::initial(), &rules, &ancestors);

        assert_eq!(style.color, 0x00_00_00_FF);
    }

    #[test]
    fn parses_solid_browser_subset() {
        let expected = Some(BorderEdge {
            width: 20,
            color: 0x00_00_00_FF,
        });

        assert_eq!(parse_solid_border("blue solid 20px"), expected);
        assert_eq!(parse_solid_border("20px SOLID blue"), expected);
        assert_eq!(parse_solid_border("none"), Some(BorderEdge::default()));

        for value in [
            "solid blue",
            "20px blue",
            "20px dashed blue",
            "20px solid blue orange",
            "-1px solid red",
            "20px solid rgb(0, 0, 255)",
        ] {
            assert_eq!(parse_solid_border(value), None, "{value}");
        }
    }

    #[test]
    fn cascades_borders_without_inheriting_them() {
        let rules =
            build_author_rules(&["div { border-top: blue solid 20px !important }".to_owned()]);

        let parent = Element {
            tag: "div",
            id: None,
            classes: &[],
            inline_style: Some(
                "border-top: orange solid 10px; \
             border-bottom: orange solid 20px",
            ),
        };

        let style = compute_style(&parent, TextStyle::initial(), &rules);

        assert_eq!(
            style.border_top,
            BorderEdge {
                width: 20,
                color: 0x00_00_00_FF
            }
        );

        assert_eq!(
            style.border_bottom,
            BorderEdge {
                width: 20,
                color: 0x00_FF_A5_00
            }
        );

        let child = Element {
            tag: "span",
            id: None,
            classes: &[],
            inline_style: None,
        };

        let child_style = compute_style(&child, style, &[]);

        assert_eq!(child_style.border_top, BorderEdge::default());
        assert_eq!(child_style.border_bottom, BorderEdge::default());
    }

    #[test]
    fn cascade_without_ancestors_does_not_match_descendant_selectors() {
        let rules = build_cascade_rules(&["g rect { fill: red } rect { fill: blue }".to_owned()]);

        let element = Element {
            tag: "rect",
            id: None,
            classes: &[],
            inline_style: None,
        };

        let values: Vec<_> = matching_declarations(&rules, &element)
            .iter()
            .map(|declaration| declaration.value.as_str())
            .collect();

        assert_eq!(values, ["blue"]);
    }

    #[test]
    fn light_theme_default_text_color_is_black() {
        assert_eq!(TextStyle::initial().color, 0x00_00_00_00);
    }

    #[test]
    fn type_selector_beats_later_universal_selector() {
        let rules = build_author_rules(&[
            "div { color: green } * { color: red } p { color: black }".to_owned(),
        ]);

        for (tag, expected) in [
            ("div", 0x00_00_80_00),
            ("p", 0x00_00_00_00),
            ("span", 0x00_FF_00_00),
        ] {
            let element = Element {
                tag,
                id: None,
                classes: &[],
                inline_style: None,
            };

            let style = compute_style(&element, TextStyle::initial(), &rules);

            assert_eq!(style.color, expected, "{tag}");
        }
    }
}
