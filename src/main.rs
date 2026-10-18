use std::{num::NonZeroU32, rc::Rc, time::Duration};

use anyhow::{Context as _, Result, bail};
use font8x8::{BASIC_FONTS, UnicodeFonts};
use html5ever::{parse_document, tendril::TendrilSink};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use reqwest::{blocking::Client, redirect::Policy};
use softbuffer::{Context as SoftbufferContext, Surface};
use url::Url;
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::{ElementState, KeyEvent, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    keyboard::{Key, NamedKey},
    window::{Window, WindowId},
};

const BACKGROUND: u32 = 0x00_F7_F7_F7;
const TOOLBAR: u32 = 0x00_20_25_2B;
const ADDRESS_BAR: u32 = 0x00_38_40_4A;
const TEXT: u32 = 0x00_18_1B_20;
const LIGHT_TEXT: u32 = 0x00_F4_F6_F8;
const HEADING: u32 = 0x00_15_5E_A8;

#[derive(Clone, Copy)]
enum Style {
    Body,
    Heading1,
    Heading2,
    Code,
}

struct Block {
    text: String,
    style: Style,
}

struct Page {
    url: String,
    title: String,
    blocks: Vec<Block>,
}

enum AppEvent {
    NavigationFinished {
        request_id: u64,
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

    let response = client.get(url.as_str()).send()?.error_for_status()?;

    let final_url = response.url().to_string();
    let html = response.text()?;

    Ok(parse_html(&final_url, &html))
}

fn parse_html(url: &str, html: &str) -> Page {
    let dom = parse_document(RcDom::default(), Default::default()).one(html);

    let title = find_title(&dom.document)
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| url.to_owned());

    let mut blocks = Vec::new();
    let mut current_text = String::new();

    extract_blocks(&dom.document, &mut blocks, &mut current_text, Style::Body);

    flush(&mut blocks, &mut current_text, Style::Body);

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

fn extract_blocks(
    node: &Handle,
    blocks: &mut Vec<Block>,
    current: &mut String,
    inherited_style: Style,
) {
    if let NodeData::Text { contents } = &node.data {
        append_collapsed(current, contents.borrow().as_ref());
        return;
    }

    let tag = match &node.data {
        NodeData::Element { name, .. } => Some(name.local.to_string()),
        _ => None,
    };

    if matches!(
        tag.as_deref(),
        Some("script" | "style" | "template" | "head" | "title" | "meta" | "link")
    ) {
        return;
    }

    if tag.as_deref() == Some("br") {
        flush(blocks, current, inherited_style);
        return;
    }

    let is_block = matches!(
        tag.as_deref(),
        Some(
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
        )
    );

    if is_block {
        flush(blocks, current, inherited_style);
    }

    let child_style = match tag.as_deref() {
        Some("h1") => Style::Heading1,
        Some("h2" | "h3") => Style::Heading2,
        Some("pre") => Style::Code,
        _ => inherited_style,
    };

    for child in node.children.borrow().iter() {
        extract_blocks(child, blocks, current, child_style);
    }

    if is_block {
        flush(blocks, current, child_style);
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

fn flush(blocks: &mut Vec<Block>, current: &mut String, style: Style) {
    let text = current.trim().to_owned();
    current.clear();

    if !text.is_empty() {
        blocks.push(Block { text, style });
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
}

impl App {
    fn new(page: Page, event_proxy: EventLoopProxy<AppEvent>) -> Self {
        Self {
            address: page.url.clone(),
            page,
            status: "Ready".to_owned(),
            loading: false,
            next_request_id: 0,
            active_request_id: 0,
            event_proxy,
            window: None,
            _context: None,
            surface: None,
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

        self.next_request_id += 1;
        let request_id = self.next_request_id;

        self.active_request_id = request_id;
        self.loading = true;
        self.status = "Loading...".to_owned();
        self.request_redraw();

        let proxy = self.event_proxy.clone();

        std::thread::spawn(move || {
            let result = load_page(&address).map_err(|error| error.to_string());

            let _ = proxy.send_event(AppEvent::NavigationFinished { request_id, result });
        });
    }

    fn handle_key(&mut self, event: KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }

        match &event.logical_key {
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

        paint(
            &mut buffer,
            width as usize,
            height as usize,
            &self.page,
            &self.address,
            &self.status,
        );

        buffer
            .present()
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;

        Ok(())
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
            AppEvent::NavigationFinished { request_id, result } => {
                if request_id != self.active_request_id {
                    return;
                }

                self.loading = false;

                match result {
                    Ok(page) => {
                        self.address = page.url.clone();
                        self.status = "Ready".to_owned();

                        if let Some(window) = &self.window {
                            window.set_title(&format!("FreeDOM - {}", page.title));
                        }

                        self.page = page;
                    }

                    Err(error) => {
                        self.status = format!("Load failed: {error}");
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

            WindowEvent::KeyboardInput { event, .. } => self.handle_key(event),

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
) {
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

    let mut y = 90;

    for block in &page.blocks {
        if y >= height as i32 {
            break;
        }

        let (scale, color, margin_after) = match block.style {
            Style::Body => (1, TEXT, 10),
            Style::Heading1 => (2, HEADING, 16),
            Style::Heading2 => (1, HEADING, 12),
            Style::Code => (1, TEXT, 10),
        };

        y = draw_wrapped_text(
            pixels,
            width,
            height,
            16,
            y,
            width as i32 - 16,
            &block.text,
            scale,
            color,
        );

        y += margin_after;
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
) -> i32 {
    let character_width = (8 * scale) as i32;
    let line_height = (10 * scale) as i32;
    let mut x = left;

    for word in text.split_whitespace() {
        let word_width = word.chars().count() as i32 * character_width;

        if x != left && x + character_width + word_width > right {
            x = left;
            y += line_height;
        }

        if y + (8 * scale) as i32 >= height as i32 {
            return y;
        }

        if x != left {
            x += character_width;
        }

        draw_text_line(pixels, width, height, x, y, word, scale, color);
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
