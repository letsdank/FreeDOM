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

#[derive(Clone, Copy)]
pub(crate) struct TextStyle {
    pub(crate) color: u32,
    pub(crate) font_size: f32,
    pub(crate) font_weight: FontWeight,
    pub(crate) font_style: FontStyle,
    pub(crate) margin_after: i32,
    pub(crate) display: Display,
}

impl TextStyle {
    pub(crate) fn initial() -> Self {
        Self {
            color: DEFAULT_TEXT_COLOR,
            font_size: 16.0,
            font_weight: FontWeight::Normal,
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
}

#[derive(Clone, Default)]
struct Declarations {
    color: Option<u32>,
    font_size: Option<FontSize>,
    font_weight: Option<FontWeight>,
    font_style: Option<FontStyle>,
    display: Option<Display>,
    margin_after: Option<i32>,
}

pub(crate) struct CssRule {
    selector: Selector,
    specificity: u16,
    source_order: usize,
    declarations: Declarations,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CssDeclaration {
    pub(crate) name: String,
    pub(crate) value: String,
}

pub(crate) struct CascadeRule {
    selector: Selector,
    specificity: u16,
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
            declarations: parse_text_declarations(&rule.declarations),
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

fn parse_stylesheet(source: &str, rules: &mut Vec<CascadeRule>, source_order: &mut usize) {
    let source = remove_css_comments(source);

    for rule_text in source.split('}') {
        let Some((selector_text, declaration_text)) = rule_text.split_once('{') else {
            continue;
        };

        let declarations = parse_declaration_list(declaration_text);

        for selector_text in selector_text.split(',') {
            let Some((selector, specificity)) = parse_selector(selector_text.trim()) else {
                continue;
            };

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

fn remove_css_comments(source: &str) -> String {
    let mut output = String::new();
    let mut remaining = source;

    loop {
        let Some(comment_start) = remaining.find("/*") else {
            output.push_str(remaining);
            break;
        };

        output.push_str(&remaining[..comment_start]);

        let after_start = &remaining[comment_start + 2..];

        let Some(comment_end) = after_start.find("*/") else {
            break;
        };

        remaining = &after_start[comment_end + 2..];
    }

    output
}

fn parse_selector(input: &str) -> Option<(Selector, u16)> {
    let input = input.trim();

    if input.is_empty()
        || input
            .chars()
            .any(|character| character.is_ascii_whitespace())
    {
        return None;
    }

    let mut selector = Selector::default();
    let mut specificity: u16 = 0;
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

                selector.id = Some(id.to_owned());
                specificity = specificity.saturating_add(100);
                remaining = tail;
            }

            b'.' => {
                let (class, tail) = take_css_identifier(&remaining[1..])?;

                selector.classes.push(class.to_owned());
                specificity = specificity.saturating_add(10);
                remaining = tail;
            }

            _ => {
                if selector.tag.is_some() || saw_universal {
                    return None;
                }

                let (tag, tail) = take_css_identifier(remaining)?;

                selector.tag = Some(tag.to_ascii_lowercase());
                specificity = specificity.saturating_add(1);
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

fn take_css_identifier(input: &str) -> Option<(&str, &str)> {
    let boundary = input
        .find(|character| matches!(character, '.' | '#'))
        .unwrap_or(input.len());

    let identifier = &input[..boundary];

    is_css_identifier(identifier).then_some((identifier, &input[boundary..]))
}

fn is_css_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_alphanumeric() || character == '-' || character == '_')
}

pub(crate) fn parse_declaration_list(source: &str) -> Vec<CssDeclaration> {
    source
        .split(';')
        .filter_map(|declaration| {
            let (name, value) = declaration.split_once(':')?;
            let name = name.trim().to_ascii_lowercase();

            (!name.is_empty()).then_some(CssDeclaration {
                name,
                value: value.trim().to_owned(),
            })
        })
        .collect()
}

fn parse_text_declarations(declarations: &[CssDeclaration]) -> Declarations {
    let mut parsed = Declarations::default();

    for declaration in declarations {
        match declaration.name.as_str() {
            "color" => parsed.color = parse_color(&declaration.value),
            "font-size" => parsed.font_size = parse_font_size(&declaration.value),
            "font-weight" => parsed.font_weight = parse_font_weight(&declaration.value),
            "font-style" => parsed.font_style = parse_font_style(&declaration.value),

            "display" => {
                parsed.display = match declaration.value.to_ascii_lowercase().as_str() {
                    "none" => Some(Display::None),
                    "block" => Some(Display::Block),
                    "inline" => Some(Display::Inline),
                    _ => None,
                };
            }

            "margin-bottom" | "margin" => {
                parsed.margin_after = parse_length(&declaration.value);
            }

            _ => {}
        }
    }

    parsed
}

fn parse_color(value: &str) -> Option<u32> {
    let value = value.trim().to_ascii_lowercase();

    match value.as_str() {
        "black" => return Some(0x00_00_00_00),
        "white" => return Some(0x00_FF_FF_FF),
        "red" => return Some(0x00_FF_00_00),
        "green" => return Some(0x00_00_80_00),
        "blue" => return Some(0x00_00_00_FF),
        "yellow" => return Some(0x00_FF_FF_00),
        "gray" | "grey" => return Some(0x00_80_80_80),
        "purple" => return Some(0x00_80_00_80),
        _ => {}
    }

    if let Some(hex) = value.strip_prefix('#') {
        return match hex.len() {
            3 => {
                let red = u8::from_str_radix(&hex[0..1], 16).ok()?;
                let green = u8::from_str_radix(&hex[1..2], 16).ok()?;
                let blue = u8::from_str_radix(&hex[2..3], 16).ok()?;

                Some(((red as u32 * 17) << 16) | ((green as u32 * 17) << 8) | (blue as u32 * 17))
            }

            6 => Some(u32::from_str_radix(hex, 16).ok()?),

            _ => None,
        };
    }

    let rgb = value.strip_prefix("rgb(")?.strip_suffix(')')?;
    let channels: Vec<u8> = rgb
        .split(',')
        .map(|channel| channel.trim().parse::<u8>())
        .collect::<std::result::Result<_, _>>()
        .ok()?;

    if channels.len() == 3 {
        return Some(
            ((channels[0] as u32) << 16) | ((channels[1] as u32) << 8) | channels[2] as u32,
        );
    }

    None
}

fn parse_font_size(value: &str) -> Option<FontSize> {
    let value = value.trim().to_ascii_lowercase();

    if let Some(value) = value.strip_suffix("px") {
        let pixels = value.trim().parse::<f32>().ok()?;
        return (pixels.is_finite() && pixels > 0.0).then_some(FontSize::Pixels(pixels));
    }

    if let Some(value) = value.strip_suffix("em") {
        let em = value.trim().parse::<f32>().ok()?;
        return (em.is_finite() && em > 0.0).then_some(FontSize::Em(em));
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
    let mut style = default_style(&element.tag, inherited_style);

    let mut matching_rules: Vec<&CssRule> = rules
        .iter()
        .filter(|rule| selector_matches(&rule.selector, element))
        .collect();

    matching_rules.sort_by_key(|rule| (rule.specificity, rule.source_order));

    for rule in matching_rules {
        apply_declarations(&mut style, &rule.declarations);
    }

    if let Some(inline_style) = &element.inline_style {
        let declarations = parse_declaration_list(inline_style);
        apply_declarations(&mut style, &parse_text_declarations(&declarations));
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

    matching_rules
        .into_iter()
        .flat_map(|rule| rule.declarations.iter())
        .collect()
}

fn apply_declarations(style: &mut TextStyle, declarations: &Declarations) {
    if let Some(color) = declarations.color {
        style.color = color;
    }

    if let Some(font_size) = declarations.font_size {
        style.font_size = match font_size {
            FontSize::Pixels(pixels) => pixels,
            FontSize::Em(em) => style.font_size * em,
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
        assert_eq!(specificity, 121);

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
}
