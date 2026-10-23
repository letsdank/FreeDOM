mod fetch;
mod url;

#[allow(dead_code)]
mod html;

#[allow(dead_code)]
mod javascript;

pub mod graphics;

#[cfg(test)]
mod wpt;

use crate::{
    fetch::Fetcher,
    graphics::{
        font::{layout::TextOutline, render::rasterize_text_outline, truetype::TrueTypeFont},
        geometry::Transform,
        raster::{Color as RasterColor, Surface as RasterSurface},
        svg::{SvgDocument, parse_svg_element, parse_svg_element_at},
    },
    html::{Document as HtmlDocument, Element as HtmlElement, Node as HtmlNode},
    javascript::{DocumentState, evaluate_with_document},
    url::Url,
};
use anyhow::{Context as _, Result};
use font8x8::{BASIC_FONTS, UnicodeFonts};
use softbuffer::{Context as SoftbufferContext, Surface};
use std::{num::NonZeroU32, rc::Rc};
use winit::keyboard::ModifiersState;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    keyboard::{Key, NamedKey},
    window::{Window, WindowId},
};

const BACKGROUND: u32 = 0x00_F7_F7_F7;
const TOOLBAR: u32 = 0x00_20_25_2B;
const ADDRESS_BAR: u32 = 0x00_38_40_4A;
const TEXT: u32 = 0x00_18_1B_20;
const LIGHT_TEXT: u32 = 0x00_F4_F6_F8;
const LINK_TEXT: u32 = 0x00_00_00_EE;

const CONTENT_TOP: i32 = 90;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Display {
    Inline,
    Block,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FontWeight {
    Normal,
    Bold,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FontStyle {
    Normal,
    Italic,
}

#[derive(Clone, Copy)]
struct TextStyle {
    color: u32,
    font_size: f32,
    font_weight: FontWeight,
    font_style: FontStyle,
    margin_after: i32,
    display: Display,
}

struct Block {
    text: String,
    style: TextStyle,
    link: Option<String>,
    svg: Option<SvgDocument>,
}

struct LinkRegion {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    target: String,
}

struct PaintResult {
    link_regions: Vec<LinkRegion>,
    max_scroll_y: i32,
}

impl LinkRegion {
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

struct Page {
    url: String,
    title: String,
    blocks: Vec<Block>,
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

struct CssRule {
    selector: Selector,
    specificity: u16,
    source_order: usize,
    declarations: Declarations,
}

enum Selector {
    Universal,
    Tag(String),
    Class(String),
    Id(String),
}

struct ElementInfo {
    tag: String,
    id: Option<String>,
    classes: Vec<String>,
    inline_style: Option<String>,
    href: Option<String>,
}

enum StylesheetSource {
    Inline(String),
    External(Url),
}

#[derive(Clone, Copy)]
enum NavigationKind {
    New,
    History { entry_index: usize },
}

struct SessionHistory {
    entries: Vec<String>,
    current: usize,
}

impl SessionHistory {
    fn new(initial_url: String) -> Self {
        Self {
            entries: vec![initial_url],
            current: 0,
        }
    }

    fn back_target(&self) -> Option<(usize, String)> {
        let entry_index = self.current.checked_sub(1)?;
        Some((entry_index, self.entries[entry_index].clone()))
    }

    fn forward_target(&self) -> Option<(usize, String)> {
        let entry_index = self.current + 1;

        (entry_index < self.entries.len()).then(|| (entry_index, self.entries[entry_index].clone()))
    }

    fn commit_new(&mut self, url: String) {
        self.entries.truncate(self.current + 1);
        self.entries.push(url);
        self.current = self.entries.len() - 1;
    }

    fn commit_existing(&mut self, entry_index: usize, url: String) {
        if entry_index < self.entries.len() {
            self.entries[entry_index] = url;
            self.current = entry_index;
        }
    }
}

enum AppEvent {
    NavigationFinished {
        request_id: u64,
        kind: NavigationKind,
        result: std::result::Result<Page, String>,
    },
}

fn main() -> Result<()> {
    let requested_url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://example.org".to_owned());

    let page =
        load_page(&requested_url).with_context(|| format!("не удалось открыть {requested_url}"))?;

    let event_loop = EventLoop::<AppEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let mut app = App::new(page, proxy);

    event_loop.run_app(&mut app)?;

    Ok(())
}

fn load_page(input: &str) -> Result<Page> {
    let input = if input.contains("://") {
        input.to_owned()
    } else {
        format!("https://{input}")
    };

    let url = Url::parse(&input)?;
    let fetcher = Fetcher::new()?;
    let response = fetcher.get(&url)?;

    let html = response.text_lossy();
    let final_url = response.url;

    let document = HtmlDocument::parse(&html);
    let stylesheets = load_author_stylesheets(&fetcher, &final_url, &document);
    let final_url_text = final_url.to_string();

    Ok(parse_dom(&final_url_text, &document, &stylesheets))
}

#[cfg(test)]
fn parse_html(url: &str, html: &str) -> Page {
    let document = HtmlDocument::parse(html);
    let stylesheets = collect_inline_stylesheets(&document);

    parse_dom(url, &document, &stylesheets)
}

fn parse_dom(url: &str, document: &HtmlDocument, stylesheets: &[String]) -> Page {
    let mut document_state = DocumentState::with_title(find_title(document).unwrap_or_default());

    register_document_elements(document, &mut document_state);
    run_inline_scripts(document, &mut document_state);

    let title = if document_state.title.is_empty() {
        url.to_owned()
    } else {
        document_state.title.clone()
    };

    let rules = build_author_rules(stylesheets);
    let base_style = TextStyle {
        color: TEXT,
        font_size: 16.0,
        font_weight: FontWeight::Normal,
        font_style: FontStyle::Normal,
        margin_after: 0,
        display: Display::Inline,
    };

    let mut blocks = Vec::new();
    let mut current_text = String::new();
    let page_url = Url::parse(url).ok();

    for child in &document.children {
        extract_blocks(
            child,
            &rules,
            &mut blocks,
            &mut current_text,
            base_style,
            page_url.as_ref(),
            &document_state,
        );
    }

    flush(&mut blocks, &mut current_text, base_style);

    Page {
        url: url.to_owned(),
        title,
        blocks,
    }
}

fn find_title(document: &HtmlDocument) -> Option<String> {
    document.children.iter().find_map(find_title_in_node)
}
fn find_title_in_node(node: &HtmlNode) -> Option<String> {
    let HtmlNode::Element(element) = node else {
        return None;
    };

    if element.name == "title" {
        return Some(text_content(node).trim().to_owned());
    }

    element.children.iter().find_map(find_title_in_node)
}

fn text_content(node: &HtmlNode) -> String {
    match node {
        HtmlNode::Text(text) => text.clone(),

        HtmlNode::Element(element) => element.children.iter().map(text_content).collect(),

        HtmlNode::Doctype(_) | HtmlNode::Comment(_) => String::new(),
    }
}

fn register_document_elements(root: &HtmlDocument, document: &mut DocumentState) {
    for child in &root.children {
        register_document_elements_in_node(child, None, document)
    }
}

fn register_document_elements_in_node(
    node: &HtmlNode,
    parent_node_id: Option<u64>,
    document: &mut DocumentState,
) {
    let HtmlNode::Element(element) = node else {
        return;
    };

    let node_id = document.register_parsed_element(
        &element.name,
        html_attribute(element, "id").unwrap_or(""),
        text_content(node),
        parent_node_id,
    );

    for attribute in &element.attributes {
        document.register_element_attribute(node_id, &attribute.name, attribute.value.clone());
    }

    for child in &element.children {
        register_document_elements_in_node(child, Some(node_id), document);
    }
}

fn run_inline_scripts(root: &HtmlDocument, document: &mut DocumentState) {
    for child in &root.children {
        run_inline_scripts_in_node(child, document);
    }
}

fn run_inline_scripts_in_node(node: &HtmlNode, document: &mut DocumentState) {
    let HtmlNode::Element(element) = node else {
        return;
    };

    if element.name == "script" {
        let is_inline = html_attribute(element, "src").is_none();

        if is_inline && is_classic_javascript(element) {
            let source = text_content(node);

            if !source.trim().is_empty() {
                if let Err(error) = evaluate_with_document(&source, document) {
                    eprintln!("FreeDOM: inline script error: {error}");
                }
            }
        }

        return;
    }

    for child in &element.children {
        run_inline_scripts_in_node(child, document);
    }
}

fn is_classic_javascript(element: &HtmlElement) -> bool {
    let Some(kind) = html_attribute(element, "type") else {
        return true;
    };

    matches!(
        kind.trim().to_ascii_lowercase().as_str(),
        "" | "text/javascript"
            | "application/javascript"
            | "text/ecmascript"
            | "application/ecmascript"
    )
}

fn build_author_rules(stylesheets: &[String]) -> Vec<CssRule> {
    let mut rules = Vec::new();
    let mut source_order = 0;

    for stylesheet in stylesheets {
        parse_stylesheet(stylesheet, &mut rules, &mut source_order);
    }

    rules
}

#[cfg(test)]
fn collect_inline_stylesheets(root: &HtmlDocument) -> Vec<String> {
    let mut stylesheets = Vec::new();

    for child in &root.children {
        collect_inline_stylesheets_from_node(child, &mut stylesheets);
    }

    stylesheets
}

#[cfg(test)]
fn collect_inline_stylesheets_from_node(node: &HtmlNode, stylesheets: &mut Vec<String>) {
    let HtmlNode::Element(element) = node else {
        return;
    };

    if element.name == "style" {
        stylesheets.push(text_content(node));
    }

    for child in &element.children {
        collect_inline_stylesheets_from_node(child, stylesheets);
    }
}

fn load_author_stylesheets(fetcher: &Fetcher, page_url: &Url, root: &HtmlDocument) -> Vec<String> {
    let mut sources = Vec::new();
    collect_stylesheet_sources(root, page_url, &mut sources);

    sources
        .into_iter()
        .filter_map(|source| match source {
            StylesheetSource::Inline(css) => Some(css),

            StylesheetSource::External(url) => match fetcher.get(&url) {
                Ok(response) => Some(response.text_lossy()),

                Err(error) => {
                    eprintln!("не удалось загрузить stylesheet {url}: {error}");
                    None
                }
            },
        })
        .collect()
}

fn collect_stylesheet_sources(
    root: &HtmlDocument,
    page_url: &Url,
    sources: &mut Vec<StylesheetSource>,
) {
    for child in &root.children {
        collect_stylesheet_sources_from_node(child, page_url, sources);
    }
}

fn collect_stylesheet_sources_from_node(
    node: &HtmlNode,
    page_url: &Url,
    sources: &mut Vec<StylesheetSource>,
) {
    let HtmlNode::Element(element) = node else {
        return;
    };

    if element.name == "style" {
        sources.push(StylesheetSource::Inline(text_content(node)));
    }

    if element.name == "link" {
        let rel = html_attribute(element, "rel");
        let href = html_attribute(element, "href");

        let is_stylesheet = rel
            .map(|rel| {
                rel.split_ascii_whitespace()
                    .any(|value| value.eq_ignore_ascii_case("stylesheet"))
            })
            .unwrap_or(false);

        if is_stylesheet {
            if let Some(href) = href {
                if let Ok(url) = page_url.join(href.trim()) {
                    if matches!(url.scheme(), "http" | "https") {
                        sources.push(StylesheetSource::External(url));
                    }
                }
            }
        }
    }

    for child in &element.children {
        collect_stylesheet_sources_from_node(child, page_url, sources);
    }
}

fn html_attribute<'a>(element: &'a HtmlElement, name: &str) -> Option<&'a str> {
    element
        .attributes
        .iter()
        .find(|attribute| attribute.name == name)
        .map(|attribute| attribute.value.as_str())
}

fn parse_stylesheet(source: &str, rules: &mut Vec<CssRule>, source_order: &mut usize) {
    let source = remove_css_comments(source);

    for rule_text in source.split('}') {
        let Some((selector_text, declaration_text)) = rule_text.split_once('{') else {
            continue;
        };

        let declarations = parse_declarations(declaration_text);

        for selector_text in selector_text.split(',') {
            let Some((selector, specificity)) = parse_selector(selector_text.trim()) else {
                continue;
            };

            rules.push(CssRule {
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

fn parse_selector(selector: &str) -> Option<(Selector, u16)> {
    if selector == "*" {
        return Some((Selector::Universal, 0));
    }

    if let Some(id) = selector.strip_prefix('#') {
        return is_css_identifier(id).then(|| (Selector::Id(id.to_owned()), 100));
    }

    if let Some(class) = selector.strip_prefix('.') {
        return is_css_identifier(class).then(|| (Selector::Class(class.to_owned()), 10));
    }

    let tag = selector.to_ascii_lowercase();
    is_css_identifier(&tag).then(|| (Selector::Tag(tag), 1))
}

fn is_css_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_alphanumeric() || character == '-' || character == '_')
}

fn parse_declarations(source: &str) -> Declarations {
    let mut declarations = Declarations::default();

    for declaration in source.split(';') {
        let Some((property, value)) = declaration.split_once(':') else {
            continue;
        };

        let property = property.trim().to_ascii_lowercase();
        let value = value.trim();

        match property.as_str() {
            "color" => declarations.color = parse_color(value),

            "font-size" => declarations.font_size = parse_font_size(value),

            "font-weight" => declarations.font_weight = parse_font_weight(value),

            "font-style" => declarations.font_style = parse_font_style(value),

            "display" => {
                declarations.display = match value.trim().to_ascii_lowercase().as_str() {
                    "none" => Some(Display::None),
                    "block" => Some(Display::Block),
                    "inline" => Some(Display::Inline),
                    _ => None,
                };
            }

            "margin-bottom" => declarations.margin_after = parse_length(value),

            "margin" => declarations.margin_after = parse_length(value),

            _ => {}
        }
    }

    declarations
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

fn element_info(node: &HtmlNode) -> Option<ElementInfo> {
    let HtmlNode::Element(element) = node else {
        return None;
    };

    let mut id = None;
    let mut classes = Vec::new();
    let mut inline_style = None;
    let mut href = None;

    for attribute in &element.attributes {
        match attribute.name.as_str() {
            "id" => id = Some(attribute.value.clone()),

            "class" => {
                classes = attribute
                    .value
                    .split_whitespace()
                    .map(ToOwned::to_owned)
                    .collect();
            }

            "style" => inline_style = Some(attribute.value.clone()),

            "href" => href = Some(attribute.value.clone()),

            _ => {}
        }
    }

    Some(ElementInfo {
        tag: element.name.clone(),
        id,
        classes,
        inline_style,
        href,
    })
}

fn extract_blocks(
    node: &HtmlNode,
    rules: &[CssRule],
    blocks: &mut Vec<Block>,
    current: &mut String,
    inherited_style: TextStyle,
    page_url: Option<&Url>,
    document: &DocumentState,
) {
    if let HtmlNode::Text(contents) = node {
        append_collapsed(current, contents);
        return;
    }

    let Some(element) = element_info(node) else {
        return;
    };

    let HtmlNode::Element(dom_element) = node else {
        unreachable!("element_info возвращает данные только для Element");
    };

    if matches!(
        element.tag.as_str(),
        "script" | "style" | "template" | "head" | "title" | "meta" | "link"
    ) {
        return;
    }

    if element.tag == "br" {
        flush(blocks, current, inherited_style);
        return;
    }

    let style = compute_style(&element, inherited_style, rules);

    if style.display == Display::None {
        return;
    }

    if element.tag == "svg" {
        flush(blocks, current, inherited_style);

        let parsed_svg = match page_url {
            Some(page_url) => parse_svg_element_at(dom_element, page_url),
            None => parse_svg_element(dom_element),
        };

        if let Ok(svg) = parsed_svg {
            blocks.push(Block {
                text: String::new(),
                svg: Some(svg),
                style,
                link: None,
            });
        }

        return;
    }

    if element.tag == "a" {
        flush(blocks, current, inherited_style);

        let mut text = String::new();

        let contents = element
            .id
            .as_deref()
            .and_then(|id| document.text_content_override(id))
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| text_content(node));

        append_collapsed(&mut text, &contents);

        let target = element
            .href
            .as_deref()
            .and_then(|href| page_url.and_then(|url| url.join(href.trim()).ok()))
            .filter(|url| matches!(url.scheme(), "http" | "https"))
            .map(|url| url.to_string());

        if !text.is_empty() {
            blocks.push(Block {
                text,
                svg: None,
                style,
                link: target,
            });
        }

        return;
    }

    let is_block = style.display == Display::Block;

    if is_block {
        flush(blocks, current, inherited_style);
    }

    if let Some(contents) = element
        .id
        .as_deref()
        .and_then(|id| document.text_content_override(id))
    {
        append_collapsed(current, contents);

        if is_block {
            flush(blocks, current, style);
        }

        return;
    }

    for child in &dom_element.children {
        extract_blocks(child, rules, blocks, current, style, page_url, document);
    }

    if is_block {
        flush(blocks, current, style);
    }
}

fn compute_style(
    element: &ElementInfo,
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
        apply_declarations(&mut style, &parse_declarations(inline_style));
    }

    style
}

fn default_style(tag: &str, inherited_style: TextStyle) -> TextStyle {
    let mut style = inherited_style;
    style.display = Display::Inline;
    style.margin_after = 0;

    if tag == "a" {
        style.color = LINK_TEXT;
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

fn selector_matches(selector: &Selector, element: &ElementInfo) -> bool {
    match selector {
        Selector::Universal => true,
        Selector::Tag(tag) => element.tag == *tag,
        Selector::Class(class) => element.classes.iter().any(|item| item == class),
        Selector::Id(id) => element.id.as_ref() == Some(id),
    }
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

fn append_collapsed(target: &mut String, source: &str) {
    for word in source.split_whitespace() {
        if !target.is_empty() {
            target.push(' ');
        }

        target.push_str(word);
    }
}

fn flush(blocks: &mut Vec<Block>, current: &mut String, style: TextStyle) {
    let text = current.trim().to_owned();
    current.clear();

    if !text.is_empty() {
        blocks.push(Block {
            text,
            svg: None,
            style,
            link: None,
        });
    }
}

struct App {
    page: Page,
    address: String,
    status: String,
    loading: bool,
    next_request_id: u64,
    active_request_id: u64,
    event_proxy: EventLoopProxy<AppEvent>,
    window: Option<Rc<Window>>,
    _context: Option<SoftbufferContext<Rc<Window>>>,
    surface: Option<Surface<Rc<Window>, Rc<Window>>>,
    link_regions: Vec<LinkRegion>,
    cursor_position: Option<(i32, i32)>,
    scroll_y: i32,
    max_scroll_y: i32,
    history: SessionHistory,
    modifiers: ModifiersState,
    vector_font_data: Option<Vec<u8>>,
}

impl App {
    fn new(page: Page, event_proxy: EventLoopProxy<AppEvent>) -> Self {
        let initial_url = page.url.clone();
        let vector_font_data = load_vector_font_from_environment();

        Self {
            address: initial_url.clone(),
            history: SessionHistory::new(initial_url),
            modifiers: ModifiersState::default(),
            page,
            status: "Ready".to_owned(),
            loading: false,
            next_request_id: 0,
            active_request_id: 0,
            event_proxy,
            window: None,
            _context: None,
            surface: None,
            link_regions: Vec::new(),
            cursor_position: None,
            scroll_y: 0,
            max_scroll_y: 0,
            vector_font_data,
        }
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn navigate(&mut self) {
        let address = self.address.trim().to_owned();

        if address.is_empty() {
            return;
        }

        self.start_navigation(address, NavigationKind::New);
    }

    fn navigate_back(&mut self) {
        if self.loading {
            return;
        }

        let Some((entry_index, address)) = self.history.back_target() else {
            return;
        };

        self.address = address.clone();
        self.start_navigation(address, NavigationKind::History { entry_index });
    }

    fn navigate_forward(&mut self) {
        if self.loading {
            return;
        }

        let Some((entry_index, address)) = self.history.forward_target() else {
            return;
        };

        self.address = address.clone();
        self.start_navigation(address, NavigationKind::History { entry_index });
    }

    fn start_navigation(&mut self, address: String, kind: NavigationKind) {
        self.next_request_id += 1;
        let request_id = self.next_request_id;

        self.active_request_id = request_id;
        self.loading = true;
        self.status = "Loading...".to_owned();
        self.request_redraw();

        let proxy = self.event_proxy.clone();

        std::thread::spawn(move || {
            let result = load_page(&address).map_err(|error| error.to_string());

            let _ = proxy.send_event(AppEvent::NavigationFinished {
                request_id,
                kind,
                result,
            });
        });
    }

    fn open_link_at_cursor(&mut self) {
        let Some((x, y)) = self.cursor_position else {
            return;
        };

        let Some(target) = self
            .link_regions
            .iter()
            .find(|region| region.contains(x, y))
            .map(|region| region.target.clone())
        else {
            return;
        };

        self.address = target;
        self.navigate();
    }

    fn handle_key(&mut self, event: KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }

        match &event.logical_key {
            Key::Named(NamedKey::ArrowLeft) if self.modifiers.alt_key() => {
                self.navigate_back();
            }

            Key::Named(NamedKey::ArrowRight) if self.modifiers.alt_key() => {
                self.navigate_forward();
            }

            Key::Named(NamedKey::Enter) => self.navigate(),

            Key::Named(NamedKey::Backspace) => {
                self.address.pop();
                self.request_redraw();
            }

            Key::Named(NamedKey::Escape) => {
                self.address = self.page.url.clone();
                self.status = "Ready".to_owned();
                self.request_redraw();
            }

            _ => {
                if let Some(text) = &event.text {
                    let text = text.to_string();

                    if !text.chars().any(char::is_control) {
                        self.address.push_str(&text);
                        self.request_redraw();
                    }
                }
            }
        }
    }

    fn scroll_by(&mut self, amount: i32) {
        self.scroll_y = next_scroll_offset(self.scroll_y, self.max_scroll_y, amount);
        self.request_redraw();
    }

    fn redraw(&mut self) -> Result<()> {
        let size = self
            .window
            .as_ref()
            .context("окно еще не создано")?
            .inner_size();

        let width = size.width.max(1);
        let height = size.height.max(1);

        let surface = self.surface.as_mut().context("surface еще не создан")?;

        surface
            .resize(
                NonZeroU32::new(width).unwrap(),
                NonZeroU32::new(height).unwrap(),
            )
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;

        let mut buffer = surface
            .buffer_mut()
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;

        let paint_result = paint(
            &mut buffer,
            width as usize,
            height as usize,
            &self.page,
            &self.address,
            &self.status,
            self.vector_font_data.as_deref(),
            self.scroll_y,
        );

        buffer
            .present()
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;

        self.link_regions = paint_result.link_regions;
        self.max_scroll_y = paint_result.max_scroll_y;

        if self.scroll_y > self.max_scroll_y {
            self.scroll_y = self.max_scroll_y;
            self.request_redraw();
        }

        Ok(())
    }
}

fn load_vector_font_from_environment() -> Option<Vec<u8>> {
    let path = std::env::var("FREEDOM_FONT").ok()?;

    let data = match std::fs::read(&path) {
        Ok(data) => data,
        Err(error) => {
            eprintln!("не удалось загрузить FREEDOM_FONT {path:?}: {error}");
            return None;
        }
    };

    match TrueTypeFont::parse(&data) {
        Ok(_) => Some(data),
        Err(error) => {
            eprintln!("FREEDOM_FONT {path:?} не поддерживается: {error}");
            None
        }
    }
}

fn next_scroll_offset(current: i32, maximum: i32, amount: i32) -> i32 {
    current.saturating_add(amount).clamp(0, maximum.max(0))
}

fn scroll_amount(delta: MouseScrollDelta) -> i32 {
    match delta {
        MouseScrollDelta::LineDelta(_, lines) => (-lines * 48.0).round() as i32,
        MouseScrollDelta::PixelDelta(position) => (-position.y).round() as i32,
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attributes = Window::default_attributes()
            .with_title(format!("FreeDOM - {}", self.page.title))
            .with_inner_size(LogicalSize::new(1024.0, 720.0));

        let window = match event_loop.create_window(attributes) {
            Ok(window) => Rc::new(window),
            Err(error) => {
                eprintln!("не удалось создать окно: {error}");
                event_loop.exit();
                return;
            }
        };

        let context = match SoftbufferContext::new(window.clone()) {
            Ok(context) => context,
            Err(error) => {
                eprintln!("не удалось создать графический контекст: {error}");
                event_loop.exit();
                return;
            }
        };

        let surface = match Surface::new(&context, window.clone()) {
            Ok(surface) => surface,
            Err(error) => {
                eprintln!("не удалось создать поверхность отрисовки: {error}");
                event_loop.exit();
                return;
            }
        };

        window.request_redraw();

        self.window = Some(window);
        self._context = Some(context);
        self.surface = Some(surface);
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::NavigationFinished {
                request_id,
                kind,
                result,
            } => {
                if request_id != self.active_request_id {
                    return;
                }

                self.loading = false;

                match result {
                    Ok(page) => {
                        match kind {
                            NavigationKind::New => self.history.commit_new(page.url.clone()),

                            NavigationKind::History { entry_index } => {
                                self.history.commit_existing(entry_index, page.url.clone());
                            }
                        }

                        self.address = page.url.clone();
                        self.status = "Ready".to_owned();

                        if let Some(window) = &self.window {
                            window.set_title(&format!("FreeDOM - {}", page.title));
                        }

                        self.scroll_y = 0;
                        self.max_scroll_y = 0;
                        self.link_regions.clear();
                        self.page = page;
                    }

                    Err(error) => {
                        self.status = format!("Load failed: {error}");

                        if matches!(kind, NavigationKind::History { .. }) {
                            self.address = self.page.url.clone();
                        }
                    }
                }

                self.request_redraw();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.window.as_ref().map(|window| window.id()) != Some(window_id) {
            return;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(_) => self.request_redraw(),

            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }

            WindowEvent::KeyboardInput { event, .. } => self.handle_key(event),

            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_position = Some((position.x as i32, position.y as i32));
            }

            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => self.open_link_at_cursor(),

            WindowEvent::MouseWheel { delta, .. } => {
                self.scroll_by(scroll_amount(delta));
            }

            WindowEvent::RedrawRequested => {
                if let Err(error) = self.redraw() {
                    eprintln!("ошибка отрисовки: {error}");
                    event_loop.exit();
                }
            }

            _ => {}
        }
    }
}

fn paint(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    page: &Page,
    address: &str,
    status: &str,
    vector_font_data: Option<&[u8]>,
    scroll_y: i32,
) -> PaintResult {
    pixels.fill(BACKGROUND);

    let vector_font = vector_font_data.and_then(|data| TrueTypeFont::parse(data).ok());

    fill_rect(pixels, width, height, 0, 0, width as i32, 70, TOOLBAR);
    fill_rect(
        pixels,
        width,
        height,
        12,
        9,
        width.saturating_sub(24) as i32,
        30,
        ADDRESS_BAR,
    );

    draw_text_line(pixels, width, height, 20, 18, address, 1, LIGHT_TEXT);
    draw_text_line(pixels, width, height, 16, 51, status, 1, LIGHT_TEXT);

    let mut y = CONTENT_TOP - scroll_y;
    let mut link_regions = Vec::new();

    for block in &page.blocks {
        if let Some(svg) = &block.svg {
            let available_width = width.saturating_sub(32);
            let (svg_width, svg_height) = svg_block_size(svg, available_width);

            if y + svg_height as i32 > CONTENT_TOP && y < height as i32 {
                if let Ok(surface) = svg.render(svg_width, svg_height, RasterColor::TRANSPARENT) {
                    surface.blit_xrgb8888(pixels, width, height, 16, y);
                }
            }

            y += svg_height as i32 + block.style.margin_after;
            continue;
        }

        y = draw_wrapped_text(
            pixels,
            width,
            height,
            16,
            y,
            width as i32 - 16,
            &block.text,
            block.style.font_size,
            block.style.font_weight,
            block.style.font_style,
            block.style.color,
            block.link.as_deref(),
            vector_font.as_ref(),
            &mut link_regions,
        );

        y += block.style.margin_after;
    }

    let document_bottom = y + scroll_y;

    PaintResult {
        link_regions,
        max_scroll_y: (document_bottom - height as i32).max(0),
    }
}

fn svg_block_size(svg: &SvgDocument, available_width: usize) -> (usize, usize) {
    const DEFAULT_WIDTH: usize = 300;
    const DEFAULT_HEIGHT: usize = 150;
    const MAX_HEIGHT: usize = 4096;

    let maximum_width = available_width.max(1);

    let declared_width = svg
        .width
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| value.round() as usize);

    let declared_height = svg
        .height
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| value.round() as usize);

    let aspect_ratio = svg
        .view_box
        .filter(|view_box| view_box.width > 0.0 && view_box.height > 0.0)
        .map(|view_box| view_box.width / view_box.height);

    let width = declared_width
        .unwrap_or_else(|| DEFAULT_WIDTH.min(maximum_width))
        .clamp(1, maximum_width);

    let height = match (declared_height, aspect_ratio) {
        (Some(height), _) => height,
        (None, Some(ratio)) => (width as f32 / ratio).round() as usize,
        (None, None) => DEFAULT_HEIGHT,
    }
        .clamp(1, MAX_HEIGHT);

    (width, height)
}

fn draw_wrapped_text(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    left: i32,
    mut y: i32,
    right: i32,
    text: &str,
    font_size: f32,
    font_weight: FontWeight,
    font_style: FontStyle,
    color: u32,
    link_target: Option<&str>,
    vector_font: Option<&TrueTypeFont<'_>>,
    link_regions: &mut Vec<LinkRegion>,
) -> i32 {
    let bitmap_scale = if font_size >= 24.0 { 2 } else { 1 };

    let bitmap_character_width = (8 * bitmap_scale) as i32;
    let bitmap_glyph_height = (8 * bitmap_scale) as i32;
    let bitmap_line_height = (10 * bitmap_scale) as i32;

    let character_width = vector_font
        .and_then(|font| {
            vector_text_width(font, " ", font_size, FontWeight::Normal, FontStyle::Normal)
        })
        .unwrap_or(bitmap_character_width);

    let vector_line_metrics = vector_font.and_then(|font| font.line_metrics(font_size));

    let glyph_height = vector_line_metrics
        .map(|metrics| metrics.line_height.ceil() as i32)
        .unwrap_or(bitmap_glyph_height);

    let line_height = vector_line_metrics
        .map(|metrics| metrics.line_height.ceil() as i32)
        .unwrap_or(bitmap_line_height);

    let mut x = left;

    for word in text.split_whitespace() {
        let word_width = vector_font
            .and_then(|font| vector_text_width(font, word, font_size, font_weight, font_style))
            .unwrap_or_else(|| {
                word.chars().count() as i32 * bitmap_character_width
                    + i32::from(font_weight == FontWeight::Bold)
            });

        if x != left && x + character_width + word_width > right {
            x = left;
            y += line_height;
        }

        if x != left {
            x += character_width;
        }

        let word_x = x;
        let is_visible = y + glyph_height > CONTENT_TOP && y < height as i32;

        if is_visible {
            let rendered_with_vector_font = vector_font.is_some_and(|font| {
                draw_vector_text_line(
                    pixels,
                    width,
                    height,
                    word_x,
                    y,
                    word,
                    font_size,
                    font_weight,
                    font_style,
                    color,
                    font,
                )
            });

            if !rendered_with_vector_font {
                draw_text_line(pixels, width, height, word_x, y, word, bitmap_scale, color);

                if font_weight == FontWeight::Bold {
                    draw_text_line(
                        pixels,
                        width,
                        height,
                        word_x + 1,
                        y,
                        word,
                        bitmap_scale,
                        color,
                    );
                }
            }

            if let Some(target) = link_target {
                link_regions.push(LinkRegion {
                    x: word_x,
                    y,
                    width: word_width,
                    height: glyph_height,
                    target: target.to_owned(),
                });
            }
        }

        x += word_width;
    }

    y + line_height
}

fn vector_text_width(
    font: &TrueTypeFont<'_>,
    text: &str,
    font_size: f32,
    font_weight: FontWeight,
    font_style: FontStyle,
) -> Option<i32> {
    let outline = font.outline_text(text, font_size).ok()?;

    let extra_width = synthetic_bold_offset(font_size, font_weight)
        + synthetic_italic_overhang(font, font_size, font_style);

    Some((outline.advance_width + extra_width).ceil().max(1.0) as i32)
}

fn synthetic_italic_shear(font_style: FontStyle) -> f32 {
    match font_style {
        FontStyle::Normal => 0.0,
        // Glyph outlines above the baseline have negative Y coordinates, so the factor is negative.
        FontStyle::Italic => -0.212_56,
    }
}

fn synthetic_italic_overhang(
    font: &TrueTypeFont<'_>,
    font_size: f32,
    font_style: FontStyle,
) -> f32 {
    let shear = synthetic_italic_shear(font_style).abs();

    if shear == 0.0 {
        return 0.0;
    }

    font.line_metrics(font_size)
        .map(|metrics| metrics.ascent * shear)
        .unwrap_or(font_size * shear)
}

fn synthetic_bold_offset(font_size: f32, font_weight: FontWeight) -> f32 {
    match font_weight {
        FontWeight::Normal => 0.0,
        FontWeight::Bold => (font_size / 24.0).clamp(1.0, 2.0),
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_vector_text_line(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    x: i32,
    y: i32,
    text: &str,
    font_size: f32,
    font_weight: FontWeight,
    font_style: FontStyle,
    color: u32,
    font: &TrueTypeFont<'_>,
) -> bool {
    let Ok(outline) = font.outline_text(text, font_size) else {
        return false;
    };

    let Some(line_metrics) = font.line_metrics(font_size) else {
        return false;
    };

    let padding = 2_i32;
    let bold_offset = synthetic_bold_offset(font_size, font_weight);
    let italic_shear = synthetic_italic_shear(font_style);
    let italic_overhang = synthetic_italic_overhang(font, font_size, font_style);
    let surface_width = (outline.advance_width + bold_offset + italic_overhang)
        .ceil()
        .max(1.0) as usize
        + 4;
    let surface_height = line_metrics.line_height.ceil().max(1.0) as usize + 4;

    let glyph_transform = Transform::skew_x(italic_shear).then(Transform::translate(
        padding as f32,
        line_metrics.ascent + padding as f32,
    ));

    let translated_outline = TextOutline {
        path: outline.path.transformed(glyph_transform),
        advance_width: outline.advance_width,
    };

    let mut surface = RasterSurface::new(surface_width, surface_height, RasterColor::TRANSPARENT);

    if rasterize_text_outline(&mut surface, &translated_outline, raster_color(color)).is_err() {
        return false;
    }

    if font_weight == FontWeight::Bold && bold_offset > 0.0 {
        let bold_outline = TextOutline {
            path: translated_outline
                .path
                .transformed(Transform::translate(bold_offset, 0.0)),
            advance_width: translated_outline.advance_width,
        };

        if rasterize_text_outline(&mut surface, &bold_outline, raster_color(color)).is_err() {
            return false;
        }
    }

    surface.blit_xrgb8888(pixels, width, height, x - padding, y - padding);

    true
}

fn raster_color(color: u32) -> RasterColor {
    RasterColor::rgba(
        ((color >> 16) & 0xff) as u8,
        ((color >> 8) & 0xff) as u8,
        (color & 0xff) as u8,
        255,
    )
}

fn draw_text_line(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    mut x: i32,
    y: i32,
    text: &str,
    scale: usize,
    color: u32,
) {
    for character in text.chars() {
        draw_glyph(pixels, width, height, x, y, character, scale, color);
        x += (8 * scale) as i32;
    }
}

fn draw_glyph(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    x: i32,
    y: i32,
    character: char,
    scale: usize,
    color: u32,
) {
    let glyph = BASIC_FONTS.get(character).or_else(|| BASIC_FONTS.get('?'));

    let Some(rows) = glyph else {
        return;
    };

    for (row, bits) in rows.iter().enumerate() {
        for column in 0..8 {
            if (*bits & (1 << column)) != 0 {
                fill_rect(
                    pixels,
                    width,
                    height,
                    x + (column * scale) as i32,
                    y + (row * scale) as i32,
                    scale as i32,
                    scale as i32,
                    color,
                );
            }
        }
    }
}

fn fill_rect(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    x: i32,
    y: i32,
    rect_width: i32,
    rect_height: i32,
    color: u32,
) {
    let start_x = x.max(0) as usize;
    let start_y = y.max(0) as usize;
    let end_x = (x + rect_width).min(width as i32).max(0) as usize;
    let end_y = (y + rect_height).min(height as i32).max(0) as usize;

    for pixel_y in start_y..end_y {
        for pixel_x in start_x..end_x {
            pixels[pixel_y * width + pixel_x] = color;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_style_overrides_author_rules() {
        let page = parse_html(
            "https://example.test",
            r#"
                <style>
                    p { color: #ff0000; }
                    .note { color: blue; font-size: 24px; }
                    #first { color: #00ff00; }
                </style>
                <p id="first" class="note" style="color: #123456">Hello</p>
            "#,
        );

        assert_eq!(page.blocks.len(), 1);
        assert_eq!(page.blocks[0].text, "Hello");
        assert_eq!(page.blocks[0].style.color, 0x00_12_34_56);
        assert!((page.blocks[0].style.font_size - 24.0).abs() < f32::EPSILON);
    }

    #[test]
    fn display_none_removes_content_from_layout() {
        let page = parse_html(
            "https://example.test",
            r#"
                <style>.hidden { display: none; }</style>
                <p>Visible text</p>
                <p class="hidden">Invisible text</p>
            "#,
        );

        assert_eq!(page.blocks.len(), 1);
        assert_eq!(page.blocks[0].text, "Visible text");
    }

    #[test]
    fn resolves_relative_external_stylesheets() {
        let dom = HtmlDocument::parse(
            r#"
                <link rel="stylesheet" href="../assets/site.css">
                <link rel="icon" href="/favicon.ico">
                <link rel="stylesheet" href="https://cdn.example.test/theme.css">
            "#,
        );

        let page_url = Url::parse("https://example.test/docs/page.html").unwrap();
        let mut sources = Vec::new();

        collect_stylesheet_sources(&dom, &page_url, &mut sources);

        let urls: Vec<String> = sources
            .into_iter()
            .filter_map(|source| match source {
                StylesheetSource::External(url) => Some(url.to_string()),
                StylesheetSource::Inline(_) => None,
            })
            .collect();

        assert_eq!(
            urls,
            vec![
                "https://example.test/assets/site.css",
                "https://cdn.example.test/theme.css",
            ]
        );
    }

    #[test]
    fn resolves_anchor_href_against_document_url() {
        let page = parse_html(
            "https://example.test/docs/guide/index.html",
            r#"<p>Read <a href="../about.html">about us</a>.</p>"#,
        );

        let link = page
            .blocks
            .iter()
            .find(|block| block.text == "about us")
            .unwrap();

        assert_eq!(
            link.link.as_deref(),
            Some("https://example.test/docs/about.html")
        );
    }

    #[test]
    fn clamps_scroll_to_document_bounds() {
        assert_eq!(next_scroll_offset(0, 500, 48), 48);
        assert_eq!(next_scroll_offset(48, 500, -100), 0);
        assert_eq!(next_scroll_offset(490, 500, 100), 500);
    }

    #[test]
    fn new_navigation_discards_forward_history() {
        let mut history = SessionHistory::new("https://example.test/one".to_owned());

        history.commit_new("https://example.test/two".to_owned());

        let (first_index, first_url) = history.back_target().unwrap();
        history.commit_existing(first_index, first_url);

        history.commit_new("https://example.test/three".to_owned());

        assert_eq!(history.forward_target(), None);

        let (_, previous_url) = history.back_target().unwrap();
        assert_eq!(previous_url, "https://example.test/one");
    }

    #[test]
    fn inline_javascript_updates_the_page_title() {
        let page = parse_html(
            "https://example.test/",
            r#"
                <title>Before</title>
                <script>
                    document.title = "FreeDOM script test";
                    assert_equals(document.title, "FreeDOM script test");
                </script>
                <p>Visible content</p>
            "#,
        );

        assert_eq!(page.title, "FreeDOM script test");
        assert_eq!(page.blocks.len(), 1);
        assert_eq!(page.blocks[0].text, "Visible content");
    }

    #[test]
    fn inline_javascript_can_read_an_element_from_the_document() {
        let page = parse_html(
            "https://example.test/",
            r#"
                <title>Before</title>
                <h1 id="welcome">Welcome <em>home</em></h1>
                <script>
                    const heading = document.getElementById("welcome");

                    assert_equals(heading.tagName, "H1");
                    assert_equals(heading.textContent, "Welcome home");

                    document.title = heading.textContent;
                </script>
            "#,
        );

        assert_eq!(page.title, "Welcome home");
    }

    #[test]
    fn javascript_text_content_updates_layout() {
        let page = parse_html(
            "https://example.test/",
            r#"
                <h1 id="welcome">Before</h1>
                <script>
                    const heading = document.getElementById("welcome");

                    heading.textContent = "After";
                    assert_equals(heading.textContent, "After");

                    document.title = heading.textContent;
                </script>
            "#,
        );

        assert_eq!(page.title, "After");
        assert_eq!(page.blocks.len(), 1);
        assert_eq!(page.blocks[0].text, "After");
    }

    #[test]
    fn keeps_inline_svg_in_document_flow() {
        let page = parse_html(
            "https://example.test",
            r##"
                <p>Before</p>
                <svg viewBox="0 0 10 10">
                    <path fill="#ff0000" d="M 0 0 H 10 V 10 H 0 Z"/>
                </svg>
                <p>After</p>
            "##,
        );

        assert_eq!(page.blocks.len(), 3);
        assert_eq!(page.blocks[0].text, "Before");
        assert!(page.blocks[1].svg.is_some());
        assert_eq!(page.blocks[2].text, "After");
    }

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
}
