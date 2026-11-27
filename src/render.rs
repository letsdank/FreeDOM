use crate::{
    graphics::{
        font::{
            TextOutline, TrueTypeFont, normalize_font_container, rasterize_text_outline,
            render::{GLYPH_SUBPIXEL_STEPS, split_subpixel_position},
        },
        frame::XrgbFrame,
        geometry::Transform,
        raster::{Color as RasterColor, Surface as RasterSurface},
    },
    inline_layout::{InlineFonts, PositionedTextRun},
    layout_debug::{clear_preview_content, fill_preview_rect, paint_preview_box},
    layout_preview::{PreviewCommand, PreviewLayout},
    page::{BlockContent, Page, ResolvedFont},
    page_layout::{
        CONTENT_TOP, PageLayout, TextWrap, bitmap_block_size, split_layout_words, stack_ascent,
        stack_text_metrics, stack_word_width, svg_block_size, synthetic_bold_offset,
        synthetic_italic_shear, text_ink_bounds, text_paint_bounds,
    },
};
use font8x8::{BASIC_FONTS, UnicodeFonts};
use freedom_css::{FontStyle, FontWeight, LineHeight, TextStyle};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

pub(crate) const BACKGROUND: u32 = 0x00_F7_F7_F7;
const TOOLBAR: u32 = 0x00_20_25_2B;
const ADDRESS_BAR: u32 = 0x00_38_40_4A;
const LIGHT_TEXT: u32 = 0x00_F4_F6_F8;

pub(crate) struct LinkRegion {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    target: String,
}

pub(crate) struct PaintResult {
    pub(crate) link_regions: Vec<LinkRegion>,
    pub(crate) max_scroll_y: i32,
}

/// Per-frame inputs to [`paint`] other than the page and its layout.
#[derive(Clone, Copy, Default)]
pub(crate) struct PaintOptions<'a> {
    /// Text shown in the address bar.
    pub(crate) address: &'a str,
    /// Text shown in the status line under the address bar.
    pub(crate) status: &'a str,
    /// TrueType data for text that has no usable page font.
    pub(crate) fallback_font_data: Option<&'a [u8]>,
    pub(crate) scroll_y: i32,
    /// Reports slow blocks on standard error.
    pub(crate) profiling: bool,
}

const MAX_BLOCK_CACHE_BYTES: usize = 64 * 1024 * 1024;
const MAX_RASTER_BLOCK_CACHE_BYTES: usize = 8 * 1024 * 1024;
const MAX_GLYPH_CACHE_BYTES: usize = 32 * 1024 * 1024;
const MAX_RASTER_GLYPH_BYTES: usize = 4 * 1024 * 1024;

/// Everything that affects a rasterized glyph, so equal keys mean equal pixels.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct GlyphRasterKey {
    font_identity: usize,
    glyph_id: u16,
    font_size_bits: u32,
    bold: bool,
    italic: bool,
    color: u32,
    phase_x: u8,
    phase_y: u8,
}

struct CachedGlyph {
    surface: RasterSurface,
    offset_x: i32,
    offset_y: i32,
}

pub(crate) struct GlyphRasterCache {
    entries: HashMap<GlyphRasterKey, CachedGlyph>,
    cached_bytes: usize,
}

impl GlyphRasterCache {
    pub(crate) fn new() -> Self {
        Self {
            entries: HashMap::new(),
            cached_bytes: 0,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.cached_bytes = 0;
    }

    fn get_or_rasterize<F>(&mut self, key: GlyphRasterKey, rasterize: F) -> Option<&CachedGlyph>
    where
        F: FnOnce() -> Option<CachedGlyph>,
    {
        if self.entries.contains_key(&key) {
            return self.entries.get(&key);
        }

        let glyph = rasterize()?;
        let byte_count = glyph.byte_count();

        if byte_count > MAX_RASTER_GLYPH_BYTES {
            return None;
        }

        if self.cached_bytes.saturating_add(byte_count) > MAX_GLYPH_CACHE_BYTES {
            self.clear();
        }

        self.cached_bytes = self.cached_bytes.saturating_add(byte_count);
        self.entries.insert(key, glyph);
        self.entries.get(&key)
    }
}

impl CachedGlyph {
    fn byte_count(&self) -> usize {
        self.surface
            .width()
            .saturating_mul(self.surface.height())
            .saturating_mul(std::mem::size_of::<RasterColor>())
    }

    fn blit_to(&self, frame: &mut XrgbFrame<'_>, origin_x: i32, origin_y: i32) {
        self.surface.blit_xrgb8888(
            frame,
            origin_x.saturating_add(self.offset_x),
            origin_y.saturating_add(self.offset_y),
        );
    }
}

pub(crate) struct BlockRasterCache {
    entries: Vec<BlockRasterCacheEntry>,
    cached_bytes: usize,
}

enum BlockRasterCacheEntry {
    Missing,
    Cached(CachedBlock),
    Uncacheable,
}

struct CachedBlock {
    width: usize,
    height: usize,
    pixels: Vec<u32>,
    link_regions: Vec<LinkRegion>,
}

impl BlockRasterCache {
    pub(crate) fn new() -> Self {
        Self {
            entries: Vec::new(),
            cached_bytes: 0,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.cached_bytes = 0;
    }

    fn cached(&self, index: usize) -> Option<&CachedBlock> {
        match self.entries.get(index)? {
            BlockRasterCacheEntry::Cached(block) => Some(block),
            BlockRasterCacheEntry::Missing | BlockRasterCacheEntry::Uncacheable => None,
        }
    }

    fn get_or_render<F>(
        &mut self,
        index: usize,
        width: usize,
        height: usize,
        render: F,
    ) -> Option<&CachedBlock>
    where
        F: FnOnce(XrgbFrame<'_>, &mut Vec<LinkRegion>),
    {
        let is_cached = matches!(
            self.entries.get(index),
            Some(BlockRasterCacheEntry::Cached(_))
        );
        let is_uncacheable = matches!(
            self.entries.get(index),
            Some(BlockRasterCacheEntry::Uncacheable)
        );

        if is_cached {
            return self.cached(index);
        }

        if is_uncacheable {
            return None;
        }

        self.entries
            .resize_with(index.saturating_add(1), || BlockRasterCacheEntry::Missing);

        let Some(pixel_count) = width.checked_mul(height) else {
            self.entries[index] = BlockRasterCacheEntry::Uncacheable;
            return None;
        };

        let Some(byte_count) = pixel_count.checked_mul(std::mem::size_of::<u32>()) else {
            self.entries[index] = BlockRasterCacheEntry::Uncacheable;
            return None;
        };

        if pixel_count == 0
            || byte_count > MAX_RASTER_BLOCK_CACHE_BYTES
            || self.cached_bytes.saturating_add(byte_count) > MAX_BLOCK_CACHE_BYTES
        {
            self.entries[index] = BlockRasterCacheEntry::Uncacheable;
            return None;
        }

        let mut pixels = vec![BACKGROUND; pixel_count];
        let mut link_regions = Vec::new();

        // The byte limit above keeps both dimensions far inside `i32`.
        let Some(frame) = XrgbFrame::new(&mut pixels, width, height) else {
            self.entries[index] = BlockRasterCacheEntry::Uncacheable;
            return None;
        };

        render(frame, &mut link_regions);

        self.cached_bytes = self.cached_bytes.saturating_add(byte_count);
        self.entries[index] = BlockRasterCacheEntry::Cached(CachedBlock {
            width,
            height,
            pixels,
            link_regions,
        });

        self.cached(index)
    }
}

impl CachedBlock {
    fn blit_to(&self, frame: &mut XrgbFrame<'_>, origin_x: i32, origin_y: i32) {
        blit_opaque(
            &self.pixels,
            self.width,
            self.height,
            frame,
            origin_x,
            origin_y,
        );
    }

    fn append_link_regions(&self, destination: &mut Vec<LinkRegion>, offset_x: i32, offset_y: i32) {
        destination.extend(
            self.link_regions
                .iter()
                .map(|region| region.translated(offset_x, offset_y)),
        );
    }
}

impl LinkRegion {
    pub(crate) fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }

    pub(crate) fn target(&self) -> &str {
        &self.target
    }

    fn translated(&self, offset_x: i32, offset_y: i32) -> Self {
        Self {
            x: self.x.saturating_add(offset_x),
            y: self.y.saturating_add(offset_y),
            width: self.width,
            height: self.height,
            target: self.target.clone(),
        }
    }
}

/// A text pen position: whole pixels plus the sub-pixel remainder that vector
/// glyph rasterization keeps.
#[derive(Clone, Copy)]
struct TextOrigin {
    x: i32,
    y: i32,
    offset_x: f32,
    offset_y: f32,
}

impl TextOrigin {
    fn at(x: i32, y: i32) -> Self {
        Self {
            x,
            y,
            offset_x: 0.0,
            offset_y: 0.0,
        }
    }
}

/// The resolved font and style of one run of text.
struct TextRunStyle<'a> {
    font_size: f32,
    /// Weight that the vector font still has to synthesize.
    font_weight: FontWeight,
    /// Style that the vector font still has to synthesize.
    font_style: FontStyle,
    /// Computed weight; the bitmap fallback font emboldens bold text.
    fallback_weight: FontWeight,
    color: u32,
    font: Option<&'a TrueTypeFont<'a>>,
    /// Faces for per-character fallback; empty when `font` is the only face.
    font_stack: &'a [ResolvedFont<'a>],
}

impl<'a> TextRunStyle<'a> {
    /// Resolves the font of a run produced by inline layout.
    fn inline(
        fonts: &'a InlineFonts<'_, '_>,
        style: &TextStyle,
        fallback: Option<&'a TrueTypeFont<'a>>,
    ) -> Self {
        Self {
            font_size: style.font_size,
            font_weight: fonts.synthetic_weight(style),
            font_style: fonts.synthetic_style(style),
            fallback_weight: style.font_weight,
            color: style.color,
            font: fonts.get(style).or(fallback),
            font_stack: fonts.stack(style),
        }
    }

    /// Resolves the font of a whole text block from the page faces that
    /// match its style.
    fn block(
        fonts: &'a [ResolvedFont<'a>],
        style: &TextStyle,
        fallback: Option<&'a TrueTypeFont<'a>>,
    ) -> Self {
        let (font_weight, font_style) = fonts
            .first()
            .map(|entry| entry.synthesis)
            .unwrap_or((style.font_weight, style.font_style));

        Self {
            font_size: style.font_size,
            font_weight,
            font_style,
            fallback_weight: style.font_weight,
            color: style.color,
            font: fonts.first().map(|entry| &entry.font).or(fallback),
            font_stack: if fonts.len() > 1 { fonts } else { &[] },
        }
    }
}

/// The destination and per-frame state shared while painting text.
struct TextPainter<'a> {
    frame: XrgbFrame<'a>,
    /// Words whose ink ends at or above this row are skipped.
    visible_top: i32,
    /// Font for text without a usable page font.
    fallback_font: Option<&'a TrueTypeFont<'a>>,
    links: &'a mut Vec<LinkRegion>,
    glyph_cache: &'a mut GlyphRasterCache,
}

impl<'a> TextPainter<'a> {
    fn new(
        frame: XrgbFrame<'a>,
        visible_top: i32,
        fallback_font: Option<&'a TrueTypeFont<'a>>,
        links: &'a mut Vec<LinkRegion>,
        glyph_cache: &'a mut GlyphRasterCache,
    ) -> Self {
        Self {
            frame,
            visible_top,
            fallback_font,
            links,
            glyph_cache,
        }
    }

    /// Paints text block `index` with its box at `left` and `top`, wrapping
    /// lines at `right`.
    fn draw_text_block(
        &mut self,
        page: &Page,
        layout: &PageLayout,
        index: usize,
        left: i32,
        top: i32,
        right: i32,
    ) {
        let Some(block) = page.blocks.get(index) else {
            return;
        };
        let BlockContent::Text(text) = &block.content else {
            return;
        };
        let link_target = block.link.as_deref();

        if let Some(runs) = layout.block_text_runs(index) {
            self.draw_positioned_runs(page, runs, left, top, right, link_target);
            return;
        }

        let fonts = page.vector_fonts_for_style(&block.style);
        let style = TextRunStyle::block(&fonts, &block.style, self.fallback_font);
        let origin = TextOrigin::at(left, top.saturating_add(block.style.padding_top));

        self.draw_wrapped_text(origin, right, &text.text, &style, link_target);
    }

    /// Paints runs placed by inline layout relative to `left` and `top`.
    fn draw_positioned_runs(
        &mut self,
        page: &Page,
        runs: &[PositionedTextRun],
        left: i32,
        top: i32,
        right: i32,
        link_target: Option<&str>,
    ) {
        let fonts = InlineFonts::from_styles(page, runs.iter().map(|run| &run.style));

        for run in runs {
            let style = TextRunStyle::inline(&fonts, &run.style, self.fallback_font);
            let origin = TextOrigin {
                x: left.saturating_add(run.x),
                y: top.saturating_add(run.y),
                offset_x: run.offset_x,
                offset_y: run.offset_y,
            };

            self.draw_wrapped_text(
                origin,
                right,
                &run.text,
                &style,
                run.link.as_deref().or(link_target),
            );
        }
    }

    /// Paints words from `origin`, wrapping lines at `right`, and returns the
    /// top of the line after the last one.
    fn draw_wrapped_text(
        &mut self,
        origin: TextOrigin,
        right: i32,
        text: &str,
        style: &TextRunStyle<'_>,
        link_target: Option<&str>,
    ) -> i32 {
        let TextOrigin {
            x: left,
            mut y,
            offset_x,
            offset_y,
        } = origin;

        if style.font_size == 0.0 {
            return y;
        }

        let metrics = stack_text_metrics(style.font, style.font_stack, style.font_size);
        let (ink_top, ink_bottom) =
            text_ink_bounds(style.font, style.font_stack, style.font_size, offset_y);
        let mut wrap = TextWrap::new(left, right);
        let height = self.frame.height() as i32;

        for word in split_layout_words(text) {
            let word_width = stack_word_width(
                style.font,
                style.font_stack,
                word,
                style.font_size,
                style.font_weight,
                style.font_style,
            );

            let placement = wrap.place_word(metrics.character_width, word_width);

            if placement.wrapped {
                y = y.saturating_add(metrics.line_height);
            }

            let word_x = placement.x;
            let is_visible = y.saturating_add(ink_bottom) > self.visible_top
                && y.saturating_add(ink_top) < height;

            if is_visible {
                let word_origin = TextOrigin {
                    x: word_x,
                    y,
                    offset_x,
                    offset_y,
                };

                let rendered_with_vector_font = style.font.is_some_and(|font| {
                    draw_vector_text_line(
                        &mut self.frame,
                        self.glyph_cache,
                        word_origin,
                        word,
                        style,
                        font,
                    )
                });

                if !rendered_with_vector_font {
                    draw_text_line(
                        &mut self.frame,
                        word_x,
                        y,
                        word,
                        metrics.bitmap_scale,
                        style.color,
                    );

                    if style.fallback_weight == FontWeight::Bold {
                        draw_text_line(
                            &mut self.frame,
                            word_x.saturating_add(1),
                            y,
                            word,
                            metrics.bitmap_scale,
                            style.color,
                        );
                    }
                }

                if let Some(target) = link_target {
                    self.links.push(LinkRegion {
                        x: word_x,
                        y,
                        width: word_width,
                        height: metrics.glyph_height,
                        target: target.to_owned(),
                    });
                }
            }
        }

        y.saturating_add(metrics.line_height)
    }
}

/// Paints an experimental preview, not a conforming CSS rendering.
pub(crate) fn paint_preview_scene(
    frame: &mut XrgbFrame<'_>,
    page: &Page,
    layout: &PreviewLayout,
    fallback_data: Option<&[u8]>,
    scroll_y: i32,
    glyph_cache: &mut GlyphRasterCache,
) {
    clear_preview_content(frame);

    let content_top = (CONTENT_TOP as usize).min(frame.height());
    let content_height = frame.height() - content_top;
    let fallback = fallback_data.and_then(|data| TrueTypeFont::parse(data).ok());
    let mut links = Vec::new();

    for command in &layout.display_list {
        match command {
            PreviewCommand::Block(index) => {
                if let Some(item) = layout.boxes.get(*index) {
                    paint_preview_box(frame, item, scroll_y);
                }
            }

            PreviewCommand::InlineBackground { rect, color } => {
                fill_preview_rect(frame, *rect, scroll_y, *color);
            }

            PreviewCommand::InlineText {
                x, y, text, style, ..
            } => {
                if style.font_size == 0.0 {
                    continue;
                }
                let relative_y = i64::from(*y) - i64::from(scroll_y);
                let Ok(y) = i32::try_from(relative_y) else {
                    continue;
                };
                let metrics = crate::page_layout::text_metrics(None, style.font_size);
                if y.saturating_add(metrics.glyph_height) <= 0 || y >= content_height as i32 {
                    continue;
                }
                let mut content = frame.rows_from(content_top);
                let scale = metrics.bitmap_scale;
                draw_text_line(&mut content, *x, y, text, scale, style.color);
                if style.font_weight == FontWeight::Bold {
                    draw_text_line(
                        &mut content,
                        x.saturating_add(1),
                        y,
                        text,
                        scale,
                        style.color,
                    );
                }
            }

            PreviewCommand::Text {
                rect,
                text,
                style,
                runs,
            } => {
                let relative_y = i64::from(rect.y) - i64::from(scroll_y);

                let fonts = InlineFonts::from_styles(
                    page,
                    std::iter::once(style).chain(runs.iter().map(|run| &run.style)),
                );
                let (paint_top, paint_bottom) = if runs
                    .iter()
                    .any(|run| run.style.line_height != LineHeight::Normal)
                {
                    text_paint_bounds(&fonts, runs, rect.height, fallback.as_ref())
                } else {
                    (0, rect.height)
                };
                if relative_y + i64::from(paint_top) >= content_height as i64
                    || relative_y + i64::from(paint_bottom) <= 0
                {
                    continue;
                }

                let Ok(y) = i32::try_from(relative_y) else {
                    continue;
                };

                // Restrict text painting to the page content below the toolbar.
                let mut painter = TextPainter::new(
                    frame.rows_from(content_top),
                    0,
                    fallback.as_ref(),
                    &mut links,
                    glyph_cache,
                );
                let right = rect.x.saturating_add(rect.width);

                if runs.is_empty() {
                    let run_style = TextRunStyle::inline(&fonts, style, fallback.as_ref());

                    painter.draw_wrapped_text(
                        TextOrigin::at(rect.x, y),
                        right,
                        text,
                        &run_style,
                        None,
                    );
                } else {
                    for run in runs {
                        let run_y = relative_y + i64::from(run.y);

                        if let Ok(run_y) = i32::try_from(run_y) {
                            let run_style =
                                TextRunStyle::inline(&fonts, &run.style, fallback.as_ref());
                            let origin = TextOrigin {
                                x: rect.x.saturating_add(run.x),
                                y: run_y,
                                offset_x: run.offset_x,
                                offset_y: run.offset_y,
                            };

                            painter.draw_wrapped_text(origin, right, &run.text, &run_style, None);
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn load_vector_font_from_environment() -> Option<Vec<u8>> {
    let path = std::env::var("FREEDOM_FONT").ok()?;

    let raw_data = match std::fs::read(&path) {
        Ok(data) => data,
        Err(error) => {
            eprintln!("Could not read FREEDOM_FONT {path:?}: {error}");
            return None;
        }
    };

    let data = match normalize_font_container(raw_data) {
        Ok(data) => data,
        Err(error) => {
            eprintln!("Could not decode FREEDOM_FONT {path:?}: {error}");
            return None;
        }
    };

    match TrueTypeFont::parse(&data) {
        Ok(_) => Some(data),
        Err(error) => {
            eprintln!("FREEDOM_FONT {path:?} is not a supported TrueType font: {error}");
            None
        }
    }
}

fn cache_text_block<'cache>(
    text_cache: &'cache mut BlockRasterCache,
    page: &Page,
    layout: &PageLayout,
    index: usize,
    viewport_width: usize,
    fallback_font: Option<&TrueTypeFont<'_>>,
    glyph_raster_cache: &mut GlyphRasterCache,
) -> Option<&'cache CachedBlock> {
    let already_cached = text_cache.cached(index).is_some();

    if already_cached {
        return text_cache.cached(index);
    }

    let block = page.blocks.get(index)?;
    let BlockContent::Text(_) = &block.content else {
        return None;
    };

    // An unwrapped line may paint into the right-hand page padding.
    let content_width = if block.style.white_space == freedom_css::WhiteSpace::Nowrap {
        viewport_width.saturating_sub(16).max(1)
    } else {
        viewport_width.saturating_sub(32).max(1)
    };

    let block_height = usize::try_from(layout.block_height(index).max(0)).unwrap_or(0);

    text_cache.get_or_render(index, content_width, block_height, |frame, links| {
        let right = frame.width() as i32;

        TextPainter::new(frame, 0, fallback_font, links, glyph_raster_cache)
            .draw_text_block(page, layout, index, 0, 0, right);
    })
}

fn cache_visual_block<'cache>(
    block_raster_cache: &'cache mut BlockRasterCache,
    page: &Page,
    index: usize,
    viewport_width: usize,
) -> Option<&'cache CachedBlock> {
    let already_cached = block_raster_cache.cached(index).is_some();

    if already_cached {
        return block_raster_cache.cached(index);
    }

    let block = page.blocks.get(index)?;
    let available_width = viewport_width.saturating_sub(32).max(1);

    match &block.content {
        BlockContent::Svg(svg) => {
            let (svg_width, svg_height) = svg_block_size(svg, available_width);

            block_raster_cache.get_or_render(
                index,
                svg_width,
                svg_height,
                |mut frame, _cache_links| {
                    if let Ok(surface) =
                        svg.render(frame.width(), frame.height(), RasterColor::TRANSPARENT)
                    {
                        surface.blit_xrgb8888(&mut frame, 0, 0);
                    }
                },
            )
        }

        BlockContent::Bitmap(bitmap) => {
            let (image_width, image_height) = bitmap_block_size(bitmap, available_width);

            block_raster_cache.get_or_render(
                index,
                image_width,
                image_height,
                |mut frame, _cache_links| {
                    let (width, height) = (frame.width(), frame.height());

                    bitmap.blit_scaled_xrgb8888(&mut frame, 0, 0, width, height);
                },
            )
        }

        BlockContent::Text(_) | BlockContent::Canvas(_) | BlockContent::PendingImage(_) => None,
    }
}

pub(crate) fn paint(
    frame: &mut XrgbFrame<'_>,
    page: &Page,
    layout: &PageLayout,
    options: PaintOptions<'_>,
    block_raster_cache: &mut BlockRasterCache,
    glyph_raster_cache: &mut GlyphRasterCache,
) -> PaintResult {
    let PaintOptions {
        address,
        status,
        fallback_font_data,
        scroll_y,
        profiling,
    } = options;
    let (width, height) = (frame.width(), frame.height());

    frame.pixels_mut().fill(BACKGROUND);

    let fallback_font = fallback_font_data.and_then(|data| TrueTypeFont::parse(data).ok());

    frame.fill_rect(0, 0, width as i32, 70, TOOLBAR);
    frame.fill_rect(12, 9, width.saturating_sub(24) as i32, 30, ADDRESS_BAR);

    draw_text_line(frame, 20, 18, address, 1, LIGHT_TEXT);
    draw_text_line(frame, 16, 51, status, 1, LIGHT_TEXT);

    let mut link_regions = Vec::new();
    let slow_block_threshold = Duration::from_millis(16);
    // Opaque block catches erase adjacent overflowing ink. Composite only
    // intersecting blocks directly, while preserving other block caches.
    let direct_blocks = layout.has_text_overflow();
    let mut painted_ink: Option<(i64, i64)> = None;
    let content_top = (CONTENT_TOP as usize).min(height);
    let text_right = i32::try_from(width.saturating_sub(16)).unwrap_or(i32::MAX);

    for (index, block) in page.blocks.iter().enumerate() {
        let y = CONTENT_TOP
            .saturating_add(layout.block_top(index))
            .saturating_sub(scroll_y);
        let (paint_top, paint_bottom) = layout.block_paint_bounds(index);
        let block_top = i64::from(y);
        let block_bottom = block_top + i64::from(layout.block_height(index));
        let overlaps_ink =
            painted_ink.is_some_and(|(top, bottom)| block_top < bottom && block_bottom > top);

        if i64::from(y) + i64::from(paint_bottom) <= i64::from(CONTENT_TOP)
            || i64::from(y) + i64::from(paint_top) >= height as i64
        {
            continue;
        }

        let block_started = profiling.then(Instant::now);

        match &block.content {
            BlockContent::Svg(svg) => {
                let cached = if overlaps_ink {
                    None
                } else {
                    cache_visual_block(block_raster_cache, page, index, width)
                };
                if let Some(cached) = cached {
                    cached.blit_to(frame, 16, y);
                } else {
                    let available_width = width.saturating_sub(32);
                    let (svg_width, svg_height) = svg_block_size(svg, available_width);

                    if let Ok(surface) = svg.render(svg_width, svg_height, RasterColor::TRANSPARENT)
                    {
                        if direct_blocks {
                            surface.blit_xrgb8888(
                                &mut frame.rows_from(content_top),
                                16,
                                y.saturating_sub(content_top as i32),
                            );
                        } else {
                            surface.blit_xrgb8888(frame, 16, y);
                        }
                    }
                }
            }

            BlockContent::Bitmap(bitmap) => {
                let cached = if overlaps_ink {
                    None
                } else {
                    cache_visual_block(block_raster_cache, page, index, width)
                };
                if let Some(cached) = cached {
                    cached.blit_to(frame, 16, y);
                } else {
                    let available_width = width.saturating_sub(32);
                    let (image_width, image_height) = bitmap_block_size(bitmap, available_width);

                    if direct_blocks {
                        bitmap.blit_scaled_xrgb8888(
                            &mut frame.rows_from(content_top),
                            16,
                            y.saturating_sub(content_top as i32),
                            image_width,
                            image_height,
                        );
                    } else {
                        bitmap.blit_scaled_xrgb8888(frame, 16, y, image_width, image_height);
                    }
                }
            }

            BlockContent::Text(_) => {
                let own_overflow = paint_top < 0 || paint_bottom > layout.block_height(index);
                let cached = if own_overflow || overlaps_ink {
                    None
                } else {
                    cache_text_block(
                        block_raster_cache,
                        page,
                        layout,
                        index,
                        width,
                        fallback_font.as_ref(),
                        glyph_raster_cache,
                    )
                };
                if let Some(cached) = cached {
                    cached.blit_to(frame, 16, y);
                    cached.append_link_regions(&mut link_regions, 16, y);
                } else if direct_blocks {
                    let first_link = link_regions.len();
                    let relative_y = y.saturating_sub(content_top as i32);

                    TextPainter::new(
                        frame.rows_from(content_top),
                        0,
                        fallback_font.as_ref(),
                        &mut link_regions,
                        glyph_raster_cache,
                    )
                    .draw_text_block(page, layout, index, 16, relative_y, text_right);

                    for region in &mut link_regions[first_link..] {
                        let top = region.y.saturating_add(content_top as i32);
                        let bottom = top.saturating_add(region.height).min(height as i32);
                        region.y = top.max(content_top as i32);
                        region.height = bottom.saturating_sub(region.y).max(0);
                    }
                    let top = i64::from(y) + i64::from(paint_top);
                    let bottom = i64::from(y) + i64::from(paint_bottom);
                    painted_ink = Some(match painted_ink {
                        Some((old_top, old_bottom)) => (old_top.min(top), old_bottom.max(bottom)),
                        None => (top, bottom),
                    });
                } else {
                    TextPainter::new(
                        frame.reborrow(),
                        CONTENT_TOP,
                        fallback_font.as_ref(),
                        &mut link_regions,
                        glyph_raster_cache,
                    )
                    .draw_text_block(page, layout, index, 16, y, text_right);
                }
            }

            BlockContent::Canvas(canvas) => {
                // blit_canvas uses window coordinates and clips to CONTENT_TOP.
                blit_canvas(canvas.canvas.surface(), frame, 16, y);
            }

            BlockContent::PendingImage(_) => {}
        }

        if let Some(block_started) = block_started {
            let elapsed = block_started.elapsed();

            if elapsed >= slow_block_threshold {
                let kind = match &block.content {
                    BlockContent::Text(_) => "text",
                    BlockContent::Svg(_) => "svg",
                    BlockContent::Bitmap(_) => "bitmap",
                    BlockContent::Canvas(_) => "canvas",
                    BlockContent::PendingImage(_) => "pending image",
                };

                eprintln!(
                    "FreeDOM slow block: index={index}, kind={kind}, width={width}, height={}, time={:.2} ms",
                    layout.block_height(index),
                    elapsed.as_secs_f64() * 1_000.0,
                );
            }
        }
    }

    let content_viewport_height = (height as i32).saturating_sub(CONTENT_TOP).max(0);

    PaintResult {
        link_regions,
        max_scroll_y: layout
            .content_height()
            .saturating_sub(content_viewport_height)
            .max(0),
    }
}

// A canvas is mutable: render it directly and only visit visible pixels.
fn blit_canvas(source: &RasterSurface, frame: &mut XrgbFrame<'_>, x: i32, y: i32) {
    let width = frame.width();
    let (width_i64, height_i64) = (width as i64, frame.height() as i64);
    let pixels = frame.pixels_mut();

    let (Ok(source_width), Ok(source_height)) = (
        i64::try_from(source.width()),
        i64::try_from(source.height()),
    ) else {
        return;
    };

    let (x, y) = (i64::from(x), i64::from(y));

    let left = x.max(0);
    let top = y.max(i64::from(CONTENT_TOP));
    let right = (x + source_width).min(width_i64);
    let bottom = (y + source_height).min(height_i64);

    if left >= right || top >= bottom {
        return;
    }

    for dy in top..bottom {
        for dx in left..right {
            let Some(foreground) = source.pixel((dx - x) as usize, (dy - y) as usize) else {
                continue;
            };

            if foreground.alpha == 0 {
                continue;
            }

            let index = dy as usize * width + dx as usize;

            let Some(pixel) = pixels.get_mut(index) else {
                continue;
            };

            let background =
                RasterColor::rgba((*pixel >> 16) as u8, (*pixel >> 8) as u8, *pixel as u8, 255);

            let color = crate::graphics::raster::blend_source_over(background, foreground, 1.0);

            *pixel = (u32::from(color.red) << 16)
                | (u32::from(color.green) << 8)
                | u32::from(color.blue);
        }
    }
}

/// Rasterizes one glyph exactly as its cache key describes. `font` must be
/// the font that `key.font_identity` was taken from.
fn rasterize_vector_glyph(font: &TrueTypeFont<'_>, key: GlyphRasterKey) -> Option<CachedGlyph> {
    let units_per_em = f32::from(font.head().units_per_em);
    let font_size = f32::from_bits(key.font_size_bits);

    if !font_size.is_finite() || font_size <= 0.0 || units_per_em <= 0.0 {
        return None;
    }

    let font_weight = if key.bold {
        FontWeight::Bold
    } else {
        FontWeight::Normal
    };
    let font_style = if key.italic {
        FontStyle::Italic
    } else {
        FontStyle::Normal
    };
    let phase_x = f32::from(key.phase_x) / GLYPH_SUBPIXEL_STEPS as f32;
    let phase_y = f32::from(key.phase_y) / GLYPH_SUBPIXEL_STEPS as f32;

    let scale = font_size / units_per_em;
    let italic_shear = synthetic_italic_shear(font_style);
    let bold_offset = synthetic_bold_offset(font_size, font_weight);

    let transform = Transform::scale(scale, -scale)
        .then(Transform::skew_x(italic_shear))
        .then(Transform::translate(phase_x, phase_y));

    let path = font.glyph_path(key.glyph_id).ok()?.transformed(transform);

    let Some(bounds) = path.control_bounds() else {
        return Some(CachedGlyph {
            surface: RasterSurface::new(0, 0, RasterColor::TRANSPARENT),
            offset_x: 0,
            offset_y: 0,
        });
    };

    let padding = 2.0;
    let left = (bounds.min.x - padding).floor();
    let top = (bounds.min.y - padding).floor();
    let right = (bounds.max.x + bold_offset + padding).ceil();
    let bottom = (bounds.max.y + padding).ceil();

    if !left.is_finite()
        || !top.is_finite()
        || !right.is_finite()
        || !bottom.is_finite()
        || left < i32::MIN as f32
        || top < i32::MIN as f32
        || right > i32::MAX as f32
        || bottom > i32::MAX as f32
    {
        return None;
    }

    let glyph_width = (right - left).max(1.0) as usize;
    let glyph_height = (bottom - top).max(1.0) as usize;
    let byte_count = glyph_width
        .checked_mul(glyph_height)?
        .checked_mul(std::mem::size_of::<RasterColor>())?;

    if byte_count > MAX_RASTER_GLYPH_BYTES {
        return None;
    }

    let translated_path = path.transformed(Transform::translate(-left, -top));
    let outline = TextOutline {
        path: translated_path,
        advance_width: 0.0,
    };

    let mut surface = RasterSurface::new(glyph_width, glyph_height, RasterColor::TRANSPARENT);

    rasterize_text_outline(&mut surface, &outline, raster_color(key.color)).ok()?;

    if font_weight == FontWeight::Bold && bold_offset > 0.0 {
        let bold_outline = TextOutline {
            path: outline
                .path
                .transformed(Transform::translate(bold_offset, 0.0)),
            advance_width: 0.0,
        };

        rasterize_text_outline(&mut surface, &bold_outline, raster_color(key.color)).ok()?;
    }

    Some(CachedGlyph {
        surface,
        offset_x: left as i32,
        offset_y: top as i32,
    })
}

/// Paints one word with `font` and the faces of `style.font_stack`.
/// Returns `false` when a glyph cannot be rasterized.
fn draw_vector_text_line(
    frame: &mut XrgbFrame<'_>,
    glyph_raster_cache: &mut GlyphRasterCache,
    origin: TextOrigin,
    text: &str,
    style: &TextRunStyle<'_>,
    font: &TrueTypeFont<'_>,
) -> bool {
    let TextRunStyle {
        font_size,
        font_weight,
        font_style,
        color,
        font_stack,
        ..
    } = *style;

    let Some(line_metrics) = font.line_metrics(font_size) else {
        return false;
    };

    let ascent = stack_ascent(font_stack, font_size).unwrap_or(line_metrics.ascent);

    let Some((baseline_y, phase_y)) = split_subpixel_position(ascent + origin.offset_y) else {
        return false;
    };

    let mut visit = |index: usize, glyph_id: u16, pen_x: f32| {
        let (font, (font_weight, font_style)) = font_stack
            .get(index)
            .map(|entry| (&entry.font, entry.synthesis))
            .unwrap_or((font, (font_weight, font_style)));

        let invalid = || crate::graphics::font::sfnt::FontError {
            offset: 0,
            message: "cannot rasterize vector glyph".to_owned(),
        };

        let (glyph_x, phase_x) =
            split_subpixel_position(origin.offset_x + pen_x).ok_or_else(invalid)?;

        let key = GlyphRasterKey {
            font_identity: font.glyph_cache_key(),
            glyph_id,
            font_size_bits: font_size.to_bits(),
            bold: font_weight == FontWeight::Bold,
            italic: font_style == FontStyle::Italic,
            color,
            phase_x,
            phase_y,
        };

        let glyph = glyph_raster_cache
            .get_or_rasterize(key, || rasterize_vector_glyph(font, key))
            .ok_or_else(invalid)?;

        glyph.blit_to(
            frame,
            origin.x.saturating_add(glyph_x),
            origin.y.saturating_add(baseline_y),
        );

        Ok(())
    };

    let result = if font_stack.is_empty() {
        crate::graphics::font::layout::visit_positioned_glyphs(&[font], text, font_size, &mut visit)
    } else {
        crate::graphics::font::layout::visit_positioned_glyphs(
            font_stack, text, font_size, &mut visit,
        )
    };

    result.is_ok()
}

fn raster_color(color: u32) -> RasterColor {
    RasterColor::rgba(
        ((color >> 16) & 0xff) as u8,
        ((color >> 8) & 0xff) as u8,
        (color & 0xff) as u8,
        255,
    )
}

/// Paints text with the 8x8 bitmap font, `scale` pixels per font pixel.
fn draw_text_line(
    frame: &mut XrgbFrame<'_>,
    mut x: i32,
    y: i32,
    text: &str,
    scale: usize,
    color: u32,
) {
    for character in text.chars() {
        draw_glyph(frame, x, y, character, scale, color);
        x = x.saturating_add((8 * scale) as i32);
    }
}

fn draw_glyph(
    frame: &mut XrgbFrame<'_>,
    x: i32,
    y: i32,
    character: char,
    scale: usize,
    color: u32,
) {
    if character == '\u{00a0}' {
        return;
    }

    let glyph = BASIC_FONTS.get(character).or_else(|| BASIC_FONTS.get('?'));

    let Some(rows) = glyph else {
        return;
    };

    for (row, bits) in rows.iter().enumerate() {
        for column in 0..8 {
            if (*bits & (1 << column)) != 0 {
                frame.fill_rect(
                    x.saturating_add((column * scale) as i32),
                    y.saturating_add((row * scale) as i32),
                    scale as i32,
                    scale as i32,
                    color,
                );
            }
        }
    }
}

/// Copies `source` into `destination` without blending, clipped to the frame.
fn blit_opaque(
    source: &[u32],
    source_width: usize,
    source_height: usize,
    destination: &mut XrgbFrame<'_>,
    origin_x: i32,
    origin_y: i32,
) {
    let destination_width = destination.width();
    let destination_height = destination.height();

    if source_width == 0 || source_height == 0 || destination_width == 0 || destination_height == 0
    {
        return;
    }

    let source_width_i32 = i32::try_from(source_width).unwrap_or(i32::MAX);
    let source_height_i32 = i32::try_from(source_height).unwrap_or(i32::MAX);

    let left = origin_x.max(0);
    let top = origin_y.max(0);
    let right = origin_x
        .saturating_add(source_width_i32)
        .min(destination_width as i32);
    let bottom = origin_y
        .saturating_add(source_height_i32)
        .min(destination_height as i32);

    if left >= right || top >= bottom {
        return;
    }

    let source_left = usize::try_from(left - origin_x).unwrap_or(0);
    let source_top = usize::try_from(top - origin_y).unwrap_or(0);
    let copy_width = usize::try_from(right - left).unwrap_or(0);
    let copy_height = usize::try_from(bottom - top).unwrap_or(0);
    let destination = destination.pixels_mut();

    for row in 0..copy_height {
        let source_start = (source_top + row) * source_width + source_left;
        let destination_start = (usize::try_from(top).unwrap_or(0) + row) * destination_width
            + usize::try_from(left).unwrap_or(0);

        destination[destination_start..destination_start + copy_width]
            .copy_from_slice(&source[source_start..source_start + copy_width]);
    }
}

#[cfg(test)]
mod tests;
