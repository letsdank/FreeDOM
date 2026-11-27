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

pub use self::color::parse_color;
use self::comments::remove_css_comments;
use self::ident::consume_identifier;
pub use self::input::preprocess_input;
use self::specificity::Specificity;
pub use self::syntax::parse_declaration_list;

// Temporary light-theme CanvasText until system colors are implemented.
const DEFAULT_TEXT_COLOR: u32 = 0x00_00_00_00;
const DEFAULT_LINK_COLOR: u32 = 0x00_00_00_EE;

// https://www.w3.org/TR/css-fonts-4/#font-size-prop
const RELATIVE_FONT_SIZE_SCALE: f32 = 1.2;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Display {
    Inline,
    Block,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontWeight {
    Normal,
    Bold,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontStyle {
    Normal,
    Italic,
}

/// Computed line-height; unitless factors remain relative when inherited.
/// <https://www.w3.org/TR/CSS22/visudet.html#propdef-line-height>
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineHeight {
    Normal,
    Number(f32),
    Pixels(f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CssSize {
    Auto,
    Pixels(i32),
}

// Partial CSS Text white-space support.
// https://www.w3.org/TR/css-text-3/#white-space-property
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhiteSpace {
    Normal,
    Nowrap,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BorderEdge {
    pub width: i32,
    pub color: u32,
}

#[derive(Clone)]
pub struct TextStyle {
    pub color: u32,
    pub font_size: f32,
    pub line_height: LineHeight,
    pub font_weight: FontWeight,
    pub font_style: FontStyle,
    pub font_families: Vec<String>,
    pub margin_before: i32,
    pub margin_after: i32,
    pub margin_left: i32,
    pub margin_right: i32,
    pub padding_top: i32,
    pub padding_bottom: i32,
    pub display: Display,
    pub width: CssSize,
    pub height: CssSize,
    pub background_color: BackgroundColor,
    pub border_top: BorderEdge,
    pub border_bottom: BorderEdge,
    pub border_left: BorderEdge,
    pub border_right: BorderEdge,
    pub white_space: WhiteSpace,
}

impl TextStyle {
    pub fn initial() -> Self {
        Self {
            color: DEFAULT_TEXT_COLOR,
            font_size: 16.0,
            line_height: LineHeight::Normal,
            font_weight: FontWeight::Normal,
            font_families: Vec::new(),
            font_style: FontStyle::Normal,
            margin_before: 0,
            margin_after: 0,
            margin_left: 0,
            margin_right: 0,
            padding_top: 0,
            padding_bottom: 0,
            display: Display::Inline,
            width: CssSize::Auto,
            height: CssSize::Auto,
            background_color: BackgroundColor::Transparent,
            border_top: BorderEdge::default(),
            border_bottom: BorderEdge::default(),
            border_left: BorderEdge::default(),
            border_right: BorderEdge::default(),
            white_space: WhiteSpace::Normal,
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

#[derive(Clone, Copy)]
enum LineHeightDeclaration {
    Normal,
    Number(f32),
    Pixels(f32),
    Em(f32),
    Percentage(f32),
    Inherit,
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
    line_height: Option<LineHeightDeclaration>,
    font_weight: Option<FontWeight>,
    font_family: Option<Vec<String>>,
    font_style: Option<FontStyle>,
    display: Option<Display>,
    margin_before: Option<i32>,
    margin_after: Option<i32>,
    margin_left: Option<i32>,
    margin_right: Option<i32>,
    padding_top: Option<i32>,
    padding_bottom: Option<i32>,
    width: Option<CssSize>,
    height: Option<CssSize>,
    background_color: Option<BackgroundColor>,
    border_top: Option<BorderEdge>,
    border_bottom: Option<BorderEdge>,
    border_left: Option<BorderEdge>,
    border_right: Option<BorderEdge>,
    white_space: Option<WhiteSpaceDeclaration>,
}

#[derive(Clone, Copy)]
enum WhiteSpaceDeclaration {
    Value(WhiteSpace),
    Inherit,
    Initial,
}

pub struct CssRule {
    selector: Selector,
    specificity: Specificity,
    source_order: usize,
    declarations: Vec<CssDeclaration>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackgroundColor {
    Transparent,
    Opaque(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CssDeclaration {
    pub name: String,
    pub value: String,
    pub important: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontFaceRule {
    pub family: String,
    pub sources: Vec<String>,
    pub font_weight: FontWeight,
    pub font_style: FontStyle,
}

pub struct CascadeRule {
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

pub fn build_author_rules(stylesheets: &[String]) -> Vec<CssRule> {
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

pub fn build_cascade_rules(stylesheets: &[String]) -> Vec<CascadeRule> {
    let mut rules = Vec::new();
    let mut source_order = 0;

    for stylesheet in stylesheets {
        parse_stylesheet(stylesheet, &mut rules, &mut source_order);
    }

    rules
}

pub fn parse_font_faces(stylesheets: &[String]) -> Vec<FontFaceRule> {
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

// Partial font-family list parsing. Generic and escaped names are not resolved here.
// https://www.w3.org/TR/css-fonts-4/#font-family-prop
fn parse_text_font_families(value: &str) -> Option<Vec<String>> {
    syntax::split_top_level_commas(value)
        .into_iter()
        .map(parse_text_font_family)
        .collect()
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

/// A conservative check of CSS parser support, not a conformance guarantee.
#[cfg(any(test, feature = "test-support"))]
pub fn audit_preview_declarations(source: &str) -> Result<(), String> {
    for declaration in parse_declaration_list(source) {
        let parsed = parse_text_declarations(std::slice::from_ref(&declaration));

        let supported = match declaration.name.as_str() {
            "color" => parsed.color.is_some(),

            "font-size" => parsed.font_size.is_some(),
            "font-weight" => parsed.font_weight.is_some(),
            "font-family" => parsed.font_family.is_some(),
            "font-style" => parsed.font_style.is_some(),

            "display" => parsed.display.is_some(),

            "margin-top" => parsed.margin_before.is_some(),
            "margin-bottom" => parsed.margin_after.is_some(),

            "padding-top" => {
                parsed.padding_top.is_some()
                    && preview_horizontal_margin_supported(&declaration.value)
            }

            "padding-bottom" => {
                parsed.padding_bottom.is_some()
                    && preview_horizontal_margin_supported(&declaration.value)
            }

            "margin-left" => {
                parsed.margin_left.is_some()
                    && preview_horizontal_margin_supported(&declaration.value)
            }

            "margin-right" => {
                parsed.margin_right.is_some()
                    && preview_horizontal_margin_supported(&declaration.value)
            }

            "margin" => {
                parsed.margin_before.is_some()
                    && parsed.margin_after.is_some()
                    && preview_margin_has_supported_horizontal(&declaration.value)
            }

            "width" => parsed.width.is_some(),
            "height" => parsed.height.is_some(),

            "background" | "background-color" => parsed.background_color.is_some(),

            "border-top" => parsed.border_top.is_some(),
            "border-bottom" => parsed.border_bottom.is_some(),
            "border-left" => parsed.border_left.is_some(),
            "border-right" => parsed.border_right.is_some(),

            _ => false,
        };

        if !supported {
            return Err(format!(
                "unsupported CSS declaration: {}: {}",
                declaration.name, declaration.value
            ));
        }
    }

    Ok(())
}

/// The preview stores horizontal margins as whole CSS pixels.
#[cfg(any(test, feature = "test-support"))]
fn preview_horizontal_margin_supported(value: &str) -> bool {
    if parse_margin_length(value).is_none() {
        return false;
    }

    let lower = value.trim().to_ascii_lowercase();

    let number = lower
        .strip_suffix("px")
        .or_else(|| lower.strip_suffix("in"))
        .unwrap_or(&lower);

    let unsigned = number.trim_start_matches(['+', '-']);
    let mantissa = unsigned.split('e').next().unwrap_or("");

    let exact_zero =
        mantissa.contains('0') && mantissa.bytes().all(|byte| matches!(byte, b'0' | b'.'));

    let integer = !unsigned.is_empty() && unsigned.bytes().all(|byte| byte.is_ascii_digit());

    exact_zero || integer
}

#[cfg(any(test, feature = "test-support"))]
fn preview_margin_has_supported_horizontal(value: &str) -> bool {
    let components: Vec<_> = value.split_ascii_whitespace().collect();

    match components.as_slice() {
        [all] => preview_horizontal_margin_supported(all),

        [_, horizontal] | [_, horizontal, _] => preview_horizontal_margin_supported(horizontal),

        [_, right, _, left] => {
            preview_horizontal_margin_supported(right) && preview_horizontal_margin_supported(left)
        }

        _ => false,
    }
}

/// Checks only a restricted subset of stylesheet syntax.
#[cfg(any(test, feature = "test-support"))]
pub fn audit_preview_stylesheet(source: &str) -> Result<(), String> {
    let source = remove_css_comments(&preprocess_input(source));

    if source
        .chars()
        .any(|character| matches!(character, '@' | '\'' | '"' | '\\' | '(' | ')' | '[' | ']'))
    {
        return Err("unsupported CSS stylesheet syntax".into());
    }

    let mut parts = source.split('}').peekable();

    while let Some(rule) = parts.next() {
        if parts.peek().is_none() {
            return if rule.trim().is_empty() {
                Ok(())
            } else {
                Err("unclosed CSS rule".into())
            };
        }

        let (selectors, declarations) = rule.split_once('{').ok_or("invalid CSS rule")?;

        if declarations.contains('{') {
            return Err("nested CSS rules are unsupported".into());
        }

        let selector_list = syntax::split_top_level_commas(selectors);

        // An empty item invalidates an ordinary selector list; the rule is ignored.
        // https://www.w3.org/TR/selectors-4/#invalid
        if selector_list.len() > 1
            && selector_list
                .iter()
                .any(|item| item.trim_matches(is_css_whitespace).is_empty())
        {
            continue;
        }

        let unsupported_selector = selector_list
            .into_iter()
            .any(|selector| parse_selector(selector).is_none());

        if unsupported_selector {
            return Err(format!(
                "unsupported or invalid CSS selector: {}",
                selectors.trim()
            ));
        }

        audit_preview_declarations(declarations)?;
    }

    Ok(())
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

            "line-height" => {
                if let Some(value) = parse_line_height(&declaration.value) {
                    parsed.line_height = Some(value);
                }
            }

            "font-weight" => {
                if let Some(value) = parse_font_weight(&declaration.value) {
                    parsed.font_weight = Some(value);
                }
            }

            "font-family" => {
                if let Some(value) = parse_text_font_families(&declaration.value) {
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

            "margin-left" => {
                if let Some(value) = parse_margin_length(&declaration.value) {
                    parsed.margin_left = Some(value);
                }
            }

            "margin-right" => {
                if let Some(value) = parse_margin_length(&declaration.value) {
                    parsed.margin_right = Some(value);
                }
            }

            "padding-top" => {
                if let Some(value) = parse_padding_length(&declaration.value) {
                    parsed.padding_top = Some(value);
                }
            }

            "padding-bottom" => {
                if let Some(value) = parse_padding_length(&declaration.value) {
                    parsed.padding_bottom = Some(value);
                }
            }

            "margin" => {
                if let Some((top, right, bottom, left)) = parse_margin_shorthand(&declaration.value)
                {
                    parsed.margin_before = Some(top);
                    parsed.margin_right = Some(right);
                    parsed.margin_after = Some(bottom);
                    parsed.margin_left = Some(left);
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

            "border-left" => {
                if let Some(value) = parse_solid_border(&declaration.value) {
                    parsed.border_left = Some(value);
                }
            }

            "border-right" => {
                if let Some(value) = parse_solid_border(&declaration.value) {
                    parsed.border_right = Some(value);
                }
            }

            "white-space" => {
                let value = match declaration.value.trim().to_ascii_lowercase().as_str() {
                    "normal" => Some(WhiteSpaceDeclaration::Value(WhiteSpace::Normal)),
                    "nowrap" => Some(WhiteSpaceDeclaration::Value(WhiteSpace::Nowrap)),
                    "inherit" | "unset" => Some(WhiteSpaceDeclaration::Inherit),
                    "initial" => Some(WhiteSpaceDeclaration::Initial),
                    _ => None,
                };

                if let Some(value) = value {
                    parsed.white_space = Some(value);
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

// Partial border shorthand: solid, non-negative integer px/in, opaque color.
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
        } else {
            let value = parse_color(component)?;
            if color.replace(value).is_some() {
                return None;
            }
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

fn parse_line_height(value: &str) -> Option<LineHeightDeclaration> {
    let value = value.trim().to_ascii_lowercase();
    match value.as_str() {
        "normal" | "initial" => return Some(LineHeightDeclaration::Normal),
        "inherit" | "unset" => return Some(LineHeightDeclaration::Inherit),
        _ => {}
    }
    let nonnegative = |value: &str| {
        let value = value.parse::<f32>().ok()?;
        (value.is_finite() && value >= 0.0).then_some(value)
    };
    if let Some(number) = value.strip_suffix("px") {
        return nonnegative(number).map(LineHeightDeclaration::Pixels);
    }
    if let Some(number) = value.strip_suffix("em") {
        return nonnegative(number).map(LineHeightDeclaration::Em);
    }
    if let Some(number) = value.strip_suffix('%') {
        return nonnegative(number).map(LineHeightDeclaration::Percentage);
    }
    nonnegative(&value).map(LineHeightDeclaration::Number)
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

// Partial padding support: px, in and unitless zero; negative lengths are invalid.
// https://www.w3.org/TR/CSS22/box.html#padding-properties
fn parse_padding_length(value: &str) -> Option<i32> {
    let input = value.trim().to_ascii_lowercase();
    let number = input
        .strip_suffix("px")
        .or_else(|| input.strip_suffix("in"))
        .unwrap_or(&input)
        .parse::<f64>()
        .ok()?;
    if number < 0.0 {
        return None;
    }
    parse_margin_length(&input)
}

// CSS 2.2 sections 10.2 and 10.5:
// https://www.w3.org/TR/CSS22/visudet.html#the-width-property
// https://www.w3.org/TR/CSS22/visudet.html#the-height-property
//
// One CSS inch equals 96 CSS pixels:
// https://www.w3.org/TR/css-values-4/#absolute-lengths
//
// This subset accepts auto, non-negative integer px/in, and unitless zero.
fn parse_size(value: &str) -> Option<CssSize> {
    let value = value.trim().to_ascii_lowercase();

    if value == "auto" {
        return Some(CssSize::Auto);
    }

    let (number, scale, has_unit) = if let Some(number) = value.strip_suffix("px") {
        (number, 1, true)
    } else if let Some(number) = value.strip_suffix("in") {
        (number, 96, true)
    } else {
        (value.as_str(), 1, false)
    };

    let number = number.parse::<i32>().ok()?;

    if number < 0 || (!has_unit && number != 0) {
        return None;
    }

    number.checked_mul(scale).map(CssSize::Pixels)
}

// CSS 2.2 section 8.3:
// https://www.w3.org/TR/CSS22/box.html#margin-properties
//
// Only px, in and unitless zero are supported. Every component is validated.
// The returned order is top, right, bottom, left.
fn parse_margin_shorthand(value: &str) -> Option<(i32, i32, i32, i32)> {
    let components = value
        .split_ascii_whitespace()
        .map(parse_margin_length)
        .collect::<Option<Vec<_>>>()?;

    match components.as_slice() {
        [all] => Some((*all, *all, *all, *all)),

        [top, horizontal] => Some((*top, *horizontal, *top, *horizontal)),

        [top, horizontal, bottom] => Some((*top, *horizontal, *bottom, *horizontal)),

        [top, right, bottom, left] => Some((*top, *right, *bottom, *left)),

        _ => None,
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn compute_style(
    element: &Element<'_>,
    inherited_style: TextStyle,
    rules: &[CssRule],
) -> TextStyle {
    compute_style_with_ancestors(element, inherited_style, rules, &[])
}

/// Ancestors are ordered from the document root to the element's parent.
pub fn compute_style_with_ancestors(
    element: &Element<'_>,
    inherited_style: TextStyle,
    rules: &[CssRule],
    ancestors: &[Element<'_>],
) -> TextStyle {
    let parent_font_size = inherited_style.font_size;
    let parent_line_height = inherited_style.line_height;
    let parent_color = inherited_style.color;
    let parent_white_space = inherited_style.white_space;
    let mut style = default_style(element.tag, inherited_style);

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

    let mut line_height = None;
    for declaration in declarations {
        let parsed = parse_text_declarations(std::slice::from_ref(declaration));
        if parsed.line_height.is_some() {
            line_height = parsed.line_height;
        }

        apply_declarations(
            &mut style,
            &parsed,
            parent_font_size,
            parent_color,
            parent_white_space,
        );
    }

    // Relative lengths and percentages resolve against the final computed
    // font-size, independent of declaration order. Unitless factors inherit.
    if let Some(line_height) = line_height {
        let scaled = |factor: f32| {
            ((f64::from(style.font_size) * f64::from(factor)).min(f64::from(f32::MAX))) as f32
        };
        style.line_height = match line_height {
            LineHeightDeclaration::Normal => LineHeight::Normal,
            LineHeightDeclaration::Number(value) => LineHeight::Number(value),
            LineHeightDeclaration::Pixels(value) => LineHeight::Pixels(value),
            LineHeightDeclaration::Em(value) => LineHeight::Pixels(scaled(value)),
            LineHeightDeclaration::Percentage(value) => LineHeight::Pixels(scaled(value / 100.0)),
            LineHeightDeclaration::Inherit => parent_line_height,
        };
    }

    style
}

fn default_style(tag: &str, inherited_style: TextStyle) -> TextStyle {
    let mut style = inherited_style;
    style.display = Display::Inline;
    style.margin_before = 0;
    style.margin_after = 0;
    style.margin_left = 0;
    style.margin_right = 0;
    style.padding_top = 0;
    style.padding_bottom = 0;
    style.width = CssSize::Auto;
    style.height = CssSize::Auto;
    style.background_color = BackgroundColor::Transparent;
    style.border_top = BorderEdge::default();
    style.border_bottom = BorderEdge::default();
    style.border_left = BorderEdge::default();
    style.border_right = BorderEdge::default();

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

        // Approximation of the user-agent bolder style with two weight levels.
        // https://html.spec.whatwg.org/multipage/rendering.html#phrasing-content-3
        "strong" | "b" => {
            style.font_weight = FontWeight::Bold;
        }

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
    let tag_matches = selector.tag.as_ref().is_none_or(|tag| element.tag == *tag);

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

pub fn matching_declarations<'a>(
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
    parent_white_space: WhiteSpace,
) {
    if let Some(white_space) = declarations.white_space {
        style.white_space = match white_space {
            WhiteSpaceDeclaration::Value(value) => value,
            WhiteSpaceDeclaration::Inherit => parent_white_space,
            WhiteSpaceDeclaration::Initial => WhiteSpace::Normal,
        };
    }

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

    if let Some(margin_left) = declarations.margin_left {
        style.margin_left = margin_left;
    }

    if let Some(margin_right) = declarations.margin_right {
        style.margin_right = margin_right;
    }

    if let Some(padding_top) = declarations.padding_top {
        style.padding_top = padding_top;
    }

    if let Some(padding_bottom) = declarations.padding_bottom {
        style.padding_bottom = padding_bottom;
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

    if let Some(border) = declarations.border_left {
        style.border_left = border;
    }

    if let Some(border) = declarations.border_right {
        style.border_right = border;
    }

    if let Some(font_weight) = declarations.font_weight {
        style.font_weight = font_weight;
    }

    if let Some(font_style) = declarations.font_style {
        style.font_style = font_style;
    }

    if let Some(font_family) = &declarations.font_family {
        style.font_families = font_family.clone();
    }
}

pub struct Element<'a> {
    pub tag: &'a str,
    pub id: Option<&'a str>,
    pub classes: &'a [String],
    pub inline_style: Option<&'a str>,
}

#[cfg(test)]
mod tests;
