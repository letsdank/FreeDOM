mod css;
mod fetch;
mod page;
mod url;

#[allow(dead_code)]
mod html;

#[allow(dead_code)]
mod javascript;

pub mod graphics;

#[cfg(test)]
mod wpt;

use crate::{
    css::{FontStyle, FontWeight},
    graphics::{
        font::{layout::TextOutline, render::rasterize_text_outline, truetype::TrueTypeFont},
        geometry::Transform,
        raster::{Color as RasterColor, Surface as RasterSurface},
        svg::SvgDocument,
    },
    page::{BlockContent, Page, load_page},
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
const LIGHT_TEXT: u32 = 0x00_F4_F6_F8;

const CONTENT_TOP: i32 = 90;

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
        match &block.content {
            BlockContent::Svg(svg) => {
                let available_width = width.saturating_sub(32);
                let (svg_width, svg_height) = svg_block_size(svg, available_width);

                if y + svg_height as i32 > CONTENT_TOP && y < height as i32 {
                    if let Ok(surface) = svg.render(svg_width, svg_height, RasterColor::TRANSPARENT)
                    {
                        surface.blit_xrgb8888(pixels, width, height, 16, y);
                    }
                }

                y += svg_height as i32 + block.style.margin_after;
            }

            BlockContent::Bitmap(bitmap) => {
                let available_width = width.saturating_sub(32);
                let (image_width, image_height) = bitmap_block_size(bitmap, available_width);

                if y + image_height as i32 > CONTENT_TOP && y < height as i32 {
                    bitmap.blit_scaled_xrgb8888(
                        pixels,
                        width,
                        height,
                        16,
                        y,
                        image_width,
                        image_height,
                    );
                }

                y += image_height as i32 + block.style.margin_after;
            }

            BlockContent::Text(text) => {
                y = draw_wrapped_text(
                    pixels,
                    width,
                    height,
                    16,
                    y,
                    width as i32 - 16,
                    text,
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
        }
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

fn bitmap_block_size(bitmap: &RasterSurface, available_width: usize) -> (usize, usize) {
    const MAX_HEIGHT: usize = 4_096;

    let source_width = bitmap.width().max(1);
    let source_height = bitmap.height().max(1);
    let width = source_width.min(available_width.max(1));

    let height = source_height
        .saturating_mul(width)
        .saturating_add(source_width - 1)
        / source_width;

    (width, height.clamp(1, MAX_HEIGHT))
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
