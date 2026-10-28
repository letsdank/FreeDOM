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

const DEFAULT_TEXT_COLOR: u32 = 0x00_18_1B_20;
const DEFAULT_LINK_COLOR: u32 = 0x00_00_00_EE;

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

#[derive(Clone)]
pub(crate) struct TextStyle {
    pub(crate) color: u32,
    pub(crate) font_size: f32,
    pub(crate) font_weight: FontWeight,
    pub(crate) font_style: FontStyle,
    pub(crate) font_family: Option<String>,
    pub(crate) margin_after: i32,
    pub(crate) display: Display,
}

impl TextStyle {
    pub(crate) fn initial() -> Self {
        Self {
            color: DEFAULT_TEXT_COLOR,
            font_size: 16.0,
            font_weight: FontWeight::Normal,
            font_family: None,
            font_style: FontStyle::Normal,
            margin_after: 0,
            display: Display::Inline,
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

#[derive(Clone, Default)]
struct Declarations {
    color: Option<u32>,
    font_size: Option<FontSize>,
    font_weight: Option<FontWeight>,
    font_family: Option<String>,
    font_style: Option<FontStyle>,
    display: Option<Display>,
    margin_after: Option<i32>,
}

pub(crate) struct CssRule {
    selector: Selector,
    specificity: Specificity,
    source_order: usize,
    declarations: Vec<CssDeclaration>,
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
            .map(|selector| parse_selector(selector.trim()))
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
    let input = input.trim();

    if input.is_empty() {
        return None;
    }

    let mut selector = Selector::default();
    let mut specificity = Specificity::default();
    let mut remaining = input;
    let mut saw_universal = false;

    while !remaining.is_empty() {
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
    .then_some((selector, specificity))
}

fn take_css_identifier(input: &str) -> Option<(String, &str)> {
    consume_identifier(input)
}

fn parse_text_declarations(declarations: &[CssDeclaration]) -> Declarations {
    let mut parsed = Declarations::default();

    for declaration in declarations {
        match declaration.name.as_str() {
            "color" => {
                if let Some(value) = parse_color(&declaration.value) {
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

            "margin-bottom" | "margin" => {
                if let Some(value) = parse_length(&declaration.value) {
                    parsed.margin_after = Some(value);
                }
            }

            _ => {}
        }
    }

    parsed
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

fn parse_length(value: &str) -> Option<i32> {
    let value = value.split_whitespace().next()?.trim();
    let value = value.trim_end_matches("px");

    value.parse::<f32>().ok().map(|value| value.round() as i32)
}

pub(crate) fn compute_style(
    element: &Element<'_>,
    inherited_style: TextStyle,
    rules: &[CssRule],
) -> TextStyle {
    let parent_font_size = inherited_style.font_size;
    let mut style = default_style(&element.tag, inherited_style);

    let mut matching_rules: Vec<&CssRule> = rules
        .iter()
        .filter(|rule| selector_matches(&rule.selector, element))
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

        apply_declarations(&mut style, &parsed, parent_font_size);
    }

    style
}

fn default_style(tag: &str, inherited_style: TextStyle) -> TextStyle {
    let mut style = inherited_style;
    style.display = Display::Inline;
    style.margin_after = 0;

    if tag == "a" {
        style.color = DEFAULT_LINK_COLOR;
    }

    if matches!(
        tag,
        "body"
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

fn apply_declarations(style: &mut TextStyle, declarations: &Declarations, parent_font_size: f32) {
    if let Some(color) = declarations.color {
        style.color = color;
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

    if let Some(margin_after) = declarations.margin_after {
        style.margin_after = margin_after;
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

        assert!(selector_matches(&selector, &element));
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

        assert_eq!(parsed.color, Some(0x00_00_80_00));
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
}
