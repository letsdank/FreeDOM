mod css;
mod fetch;
mod page;
mod page_layout;
mod render;
mod url;

#[allow(dead_code)]
mod html;

#[allow(dead_code)]
mod javascript;

pub mod graphics;

#[cfg(test)]
mod wpt;

use crate::{
    page::{Page, load_page},
    page_layout::PageLayout,
    render::{BlockRasterCache, LinkRegion, load_vector_font_from_environment, paint},
};
use anyhow::{Context as _, Result};
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
    layout: Option<PageLayout>,
    block_raster_cache: BlockRasterCache,
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
            layout: None,
            block_raster_cache: BlockRasterCache::new(),
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
            .map(|region| region.target().to_owned())
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
        let viewport_width = width as usize;

        let rebuild_layout = match &self.layout {
            Some(layout) => layout.viewport_width() != viewport_width,
            None => true,
        };

        if rebuild_layout {
            self.layout = Some(PageLayout::build(
                &self.page,
                viewport_width,
                self.vector_font_data.as_deref(),
            ));
            self.block_raster_cache.clear();
        }

        let layout = self.layout.as_ref().expect("page layout must exist");

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
            layout,
            &self.address,
            &self.status,
            self.vector_font_data.as_deref(),
            &mut self.block_raster_cache,
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
                        self.layout = None;
                        self.block_raster_cache.clear();
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
