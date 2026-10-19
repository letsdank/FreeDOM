mod freedom_url;

#[allow(dead_code)]
mod html;

#[allow(dead_code)]
mod javascript;

#[cfg(test)]
mod wpt_url;

use crate::freedom_url::Url;
use anyhow::{Context as _, Result, bail};
use font8x8::{BASIC_FONTS, UnicodeFonts};
use html5ever::{parse_document, tendril::TendrilSink};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use reqwest::{blocking::Client, redirect::Policy};
use softbuffer::{Context as SoftbufferContext, Surface};
use std::{num::NonZeroU32, rc::Rc, time::Duration};
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

#[derive(Clone, Copy)]
struct TextStyle {
    color: u32,
    scale: usize,
    margin_after: i32,
    display: Display,
}

struct Block {
    text: String,
    style: TextStyle,
    link: Option<String>,
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

#[derive(Clone, Default)]
struct Declarations {
    color: Option<u32>,
    font_scale: Option<usize>,
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

    if !matches!(url.scheme(), "http" | "https") {
        bail!("FreeDOM пока поддерживает только http:// и https://");
    }

    let client = Client::builder()
        .user_agent("FreeDOM/0.1 (experimental Rust browser)")
        .timeout(Duration::from_secs(15))
        .redirect(Policy::limited(10))
        .build()?;

    let response = client.get(url.to_string()).send()?.error_for_status()?;

    let final_url =
        Url::parse(response.url().as_str()).context("server returned an invalid final URL")?;
    let html = response.text()?;

    let dom = parse_document(RcDom::default(), Default::default()).one(html);
    let stylesheets = load_author_stylesheets(&client, &final_url, &dom.document);
    let final_url_text = final_url.to_string();

    Ok(parse_dom(&final_url_text, dom, &stylesheets))
}

#[cfg(test)]
fn parse_html(url: &str, html: &str) -> Page {
    let dom = parse_document(RcDom::default(), Default::default()).one(html);
    let stylesheets = collect_inline_stylesheets(&dom.document);

    parse_dom(url, dom, &stylesheets)
}

fn parse_dom(url: &str, dom: RcDom, stylesheets: &[String]) -> Page {
    let title = find_title(&dom.document)
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| url.to_owned());

    let rules = build_author_rules(stylesheets);
    let base_style = TextStyle {
        color: TEXT,
        scale: 1,
        margin_after: 0,
        display: Display::Inline,
    };

    let mut blocks = Vec::new();
    let mut current_text = String::new();
    let page_url = Url::parse(url).ok();

    extract_blocks(
        &dom.document,
        &rules,
        &mut blocks,
        &mut current_text,
        base_style,
        page_url.as_ref(),
    );

    flush(&mut blocks, &mut current_text, base_style);

    Page {
        url: url.to_owned(),
        title,
        blocks,
    }
}

fn find_title(node: &Handle) -> Option<String> {
    if let NodeData::Element { name, .. } = &node.data {
        if name.local.as_ref() == "title" {
            return Some(text_content(node).trim().to_owned());
        }
    }

    for child in node.children.borrow().iter() {
        if let Some(title) = find_title(child) {
            return Some(title);
        }
    }

    None
}

fn text_content(node: &Handle) -> String {
    let mut result = String::new();

    if let NodeData::Text { contents } = &node.data {
        result.push_str(contents.borrow().as_ref());
    }

    for child in node.children.borrow().iter() {
        result.push_str(&text_content(child));
    }

    result
}

fn build_author_rules(stylesheets: &[String]) -> Vec<CssRule> {
    let mut rules = Vec::new();
    let mut source_order = 0;

    for stylesheet in stylesheets {
        parse_stylesheet(stylesheet, &mut rules, &mut source_order);
    }

    rules
}

fn collect_inline_stylesheets(root: &Handle) -> Vec<String> {
    let mut stylesheets = Vec::new();
    collect_inline_stylesheets_from_node(root, &mut stylesheets);

    stylesheets
}

fn collect_inline_stylesheets_from_node(node: &Handle, stylesheets: &mut Vec<String>) {
    if let Some(element) = element_info(node) {
        if element.tag == "style" {
            stylesheets.push(text_content(node));
        }
    }

    for child in node.children.borrow().iter() {
        collect_inline_stylesheets_from_node(child, stylesheets);
    }
}

fn load_author_stylesheets(client: &Client, page_url: &Url, root: &Handle) -> Vec<String> {
    let mut sources = Vec::new();
    collect_stylesheet_sources(root, page_url, &mut sources);

    sources
        .into_iter()
        .filter_map(|source| match source {
            StylesheetSource::Inline(css) => Some(css),

            StylesheetSource::External(url) => {
                let response = client
                    .get(url.to_string())
                    .send()
                    .and_then(|response| response.error_for_status())
                    .and_then(|response| response.text());

                match response {
                    Ok(css) => Some(css),

                    Err(error) => {
                        eprintln!("не удалось загрузить stylesheet {url}: {error}");
                        None
                    }
                }
            }
        })
        .collect()
}

fn collect_stylesheet_sources(node: &Handle, page_url: &Url, sources: &mut Vec<StylesheetSource>) {
    if let NodeData::Element { name, attrs, .. } = &node.data {
        let tag = name.local.to_string();

        if tag == "style" {
            sources.push(StylesheetSource::Inline(text_content(node)));
        }

        if tag == "link" {
            let attributes = attrs.borrow();

            let rel = attributes.iter().find_map(|attribute| {
                (attribute.name.local.to_string() == "rel").then(|| attribute.value.to_string())
            });

            let href = attributes.iter().find_map(|attribute| {
                (attribute.name.local.to_string() == "href").then(|| attribute.value.to_string())
            });

            let is_stylesheet = rel
                .as_deref()
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
    }

    for child in node.children.borrow().iter() {
        collect_stylesheet_sources(child, page_url, sources);
    }
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

            "font-size" => declarations.font_scale = parse_font_scale(value),

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

fn parse_font_scale(value: &str) -> Option<usize> {
    let value = value.trim().to_ascii_lowercase();

    if let Some(value) = value.strip_suffix("px") {
        let pixels = value.trim().parse::<f32>().ok()?;
        return Some(if pixels >= 20.0 { 2 } else { 1 });
    }

    if let Some(value) = value.strip_suffix("em") {
        let em = value.trim().parse::<f32>().ok()?;
        return Some(if em >= 1.5 { 2 } else { 1 });
    }

    None
}

fn parse_length(value: &str) -> Option<i32> {
    let value = value.split_whitespace().next()?.trim();
    let value = value.trim_end_matches("px");

    value.parse::<f32>().ok().map(|value| value.round() as i32)
}

fn element_info(node: &Handle) -> Option<ElementInfo> {
    let NodeData::Element { name, attrs, .. } = &node.data else {
        return None;
    };

    let mut id = None;
    let mut classes = Vec::new();
    let mut inline_style = None;
    let mut href = None;

    for attribute in attrs.borrow().iter() {
        let attribute_name = attribute.name.local.to_string();
        let value = attribute.value.to_string();

        match attribute_name.as_str() {
            "id" => id = Some(value),

            "class" => {
                classes = value.split_whitespace().map(ToOwned::to_owned).collect();
            }

            "style" => inline_style = Some(value),

            "href" => href = Some(value),

            _ => {}
        }
    }

    Some(ElementInfo {
        tag: name.local.to_string(),
        id,
        classes,
        inline_style,
        href,
    })
}

fn extract_blocks(
    node: &Handle,
    rules: &[CssRule],
    blocks: &mut Vec<Block>,
    current: &mut String,
    inherited_style: TextStyle,
    page_url: Option<&Url>,
) {
    if let NodeData::Text { contents } = &node.data {
        append_collapsed(current, contents.borrow().as_ref());
        return;
    }

    let Some(element) = element_info(node) else {
        for child in node.children.borrow().iter() {
            extract_blocks(child, rules, blocks, current, inherited_style, page_url);
        }

        return;
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

    if element.tag == "a" {
        flush(blocks, current, inherited_style);

        let mut text = String::new();
        append_collapsed(&mut text, &text_content(node));

        let target = element
            .href
            .as_deref()
            .and_then(|href| page_url.and_then(|url| url.join(href.trim()).ok()))
            .filter(|url| matches!(url.scheme(), "http" | "https"))
            .map(|url| url.to_string());

        if !text.is_empty() {
            blocks.push(Block {
                text,
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

    for child in node.children.borrow().iter() {
        extract_blocks(child, rules, blocks, current, style, page_url);
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
    ) {
        style.display = Display::Block;
    }

    match tag {
        "h1" => {
            style.scale = 2;
            style.margin_after = 16;
        }

        "h2" | "h3" => {
            style.scale = 1;
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

    if let Some(scale) = declarations.font_scale {
        style.scale = scale;
    }

    if let Some(display) = declarations.display {
        style.display = display;
    }

    if let Some(margin_after) = declarations.margin_after {
        style.margin_after = margin_after;
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
}

impl App {
    fn new(page: Page, event_proxy: EventLoopProxy<AppEvent>) -> Self {
        let initial_url = page.url.clone();

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
    scroll_y: i32,
) -> PaintResult {
    pixels.fill(BACKGROUND);

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
        y = draw_wrapped_text(
            pixels,
            width,
            height,
            16,
            y,
            width as i32 - 16,
            &block.text,
            block.style.scale,
            block.style.color,
            block.link.as_deref(),
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

fn draw_wrapped_text(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    left: i32,
    mut y: i32,
    right: i32,
    text: &str,
    scale: usize,
    color: u32,
    link_target: Option<&str>,
    link_regions: &mut Vec<LinkRegion>,
) -> i32 {
    let character_width = (8 * scale) as i32;
    let glyph_height = (8 * scale) as i32;
    let line_height = (10 * scale) as i32;
    let mut x = left;

    for word in text.split_whitespace() {
        let word_width = word.chars().count() as i32 * character_width;

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
            draw_text_line(pixels, width, height, word_x, y, word, scale, color);

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
        assert_eq!(page.blocks[0].style.scale, 2);
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
        let dom = parse_document(RcDom::default(), Default::default()).one(
            r#"
                <link rel="stylesheet" href="../assets/site.css">
                <link rel="icon" href="/favicon.ico">
                <link rel="stylesheet" href="https://cdn.example.test/theme.css">
            "#,
        );

        let page_url = Url::parse("https://example.test/docs/page.html").unwrap();
        let mut sources = Vec::new();

        collect_stylesheet_sources(&dom.document, &page_url, &mut sources);

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
}
