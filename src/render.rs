use crate::{
    graphics::{
        font::{
            TextOutline, TrueTypeFont, normalize_font_container, rasterize_text_outline,
            render::{GLYPH_SUBPIXEL_STEPS, split_subpixel_position},
        },
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
use freedom_css::{FontStyle, FontWeight, LineHeight};
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

const MAX_BLOCK_CACHE_BYTES: usize = 64 * 1024 * 1024;
const MAX_RASTER_BLOCK_CACHE_BYTES: usize = 8 * 1024 * 1024;
const MAX_GLYPH_CACHE_BYTES: usize = 32 * 1024 * 1024;
const MAX_RASTER_GLYPH_BYTES: usize = 4 * 1024 * 1024;

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

    fn blit_to(
        &self,
        pixels: &mut [u32],
        width: usize,
        height: usize,
        origin_x: i32,
        origin_y: i32,
    ) {
        self.surface.blit_xrgb8888(
            pixels,
            width,
            height,
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
        F: FnOnce(&mut [u32], usize, usize, &mut Vec<LinkRegion>),
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

        render(&mut pixels, width, height, &mut link_regions);

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
    fn blit_to(
        &self,
        destination: &mut [u32],
        destination_width: usize,
        destination_height: usize,
        origin_x: i32,
        origin_y: i32,
    ) {
        blit_opaque(
            &self.pixels,
            self.width,
            self.height,
            destination,
            destination_width,
            destination_height,
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

/// Paints an experimental preview, not a conforming CSS rendering.
#[expect(
    clippy::too_many_arguments,
    reason = "the pixel buffer and its size are passed separately until rendering has a paint target type"
)]
pub(crate) fn paint_preview_scene(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    page: &Page,
    layout: &PreviewLayout,
    fallback_data: Option<&[u8]>,
    scroll_y: i32,
    glyph_cache: &mut GlyphRasterCache,
) {
    if i32::try_from(width).is_err()
        || i32::try_from(height).is_err()
        || !clear_preview_content(pixels, width, height)
    {
        return;
    }

    let content_top = (CONTENT_TOP as usize).min(height);
    let content_height = height - content_top;
    let fallback = fallback_data.and_then(|data| TrueTypeFont::parse(data).ok());
    let mut links = Vec::new();

    for command in &layout.display_list {
        match command {
            PreviewCommand::Block(index) => {
                if let Some(item) = layout.boxes.get(*index) {
                    paint_preview_box(pixels, width, height, item, scroll_y);
                }
            }

            PreviewCommand::InlineBackground { rect, color } => {
                fill_preview_rect(pixels, width, height, *rect, scroll_y, *color);
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
                let content_pixels = &mut pixels[content_top * width..width * height];
                let scale = metrics.bitmap_scale;
                draw_text_line(
                    content_pixels,
                    width,
                    content_height,
                    *x,
                    y,
                    text,
                    scale,
                    style.color,
                );
                if style.font_weight == FontWeight::Bold {
                    draw_text_line(
                        content_pixels,
                        width,
                        content_height,
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
                let content_pixels = &mut pixels[content_top * width..width * height];

                let mut paint_text =
                    |x: i32,
                     offset_x: f32,
                     y: i32,
                     offset_y: f32,
                     text: &str,
                     run_style: &freedom_css::TextStyle| {
                        let font = fonts.get(run_style).or(fallback.as_ref());
                        let font_stack = fonts.stack(run_style);

                        draw_wrapped_text(
                            content_pixels,
                            width,
                            content_height,
                            x,
                            offset_x,
                            y,
                            offset_y,
                            rect.x.saturating_add(rect.width),
                            text,
                            run_style.font_size,
                            fonts.synthetic_weight(run_style),
                            run_style.font_weight,
                            fonts.synthetic_style(run_style),
                            run_style.color,
                            None,
                            font,
                            font_stack,
                            0,
                            &mut links,
                            glyph_cache,
                        );
                    };

                if runs.is_empty() {
                    paint_text(rect.x, 0.0, y, 0.0, text, style);
                } else {
                    for run in runs {
                        let run_y = relative_y + i64::from(run.y);

                        if let Ok(run_y) = i32::try_from(run_y) {
                            paint_text(
                                rect.x.saturating_add(run.x),
                                run.offset_x,
                                run_y,
                                run.offset_y,
                                &run.text,
                                &run.style,
                            );
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
    let BlockContent::Text(text) = &block.content else {
        return None;
    };

    // An unwrapped line may paint into the right-hand page padding.
    let content_width = if block.style.white_space == freedom_css::WhiteSpace::Nowrap {
        viewport_width.saturating_sub(16).max(1)
    } else {
        viewport_width.saturating_sub(32).max(1)
    };

    let block_height = usize::try_from(layout.block_height(index).max(0)).unwrap_or(0);

    if let Some(runs) = layout.block_text_runs(index) {
        return text_cache.get_or_render(
            index,
            content_width,
            block_height,
            |pixels, width, height, links| {
                draw_positioned_text(
                    pixels,
                    width,
                    height,
                    0,
                    0,
                    width as i32,
                    runs,
                    page,
                    fallback_font,
                    block.link.as_deref(),
                    0,
                    links,
                    glyph_raster_cache,
                );
            },
        );
    }

    let page_fonts = page.vector_fonts_for_style(&block.style);
    let page_font = page_fonts.first().map(|entry| &entry.font);
    let vector_font = page_font.or(fallback_font);

    let font_stack = if page_fonts.len() > 1 {
        page_fonts.as_slice()
    } else {
        &[]
    };

    let (weight, style) = page_fonts
        .first()
        .map(|entry| entry.synthesis)
        .unwrap_or((block.style.font_weight, block.style.font_style));

    text_cache.get_or_render(
        index,
        content_width,
        block_height,
        |cache_pixels, cache_width, cache_height, cache_links| {
            draw_wrapped_text(
                cache_pixels,
                cache_width,
                cache_height,
                0,
                0.0,
                block.style.padding_top,
                0.0,
                cache_width as i32,
                &text.text,
                block.style.font_size,
                weight,
                block.style.font_weight,
                style,
                block.style.color,
                block.link.as_deref(),
                vector_font,
                font_stack,
                0,
                cache_links,
                glyph_raster_cache,
            );
        },
    )
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
                |cache_pixels, cache_width, cache_height, _cache_links| {
                    if let Ok(surface) =
                        svg.render(cache_width, cache_height, RasterColor::TRANSPARENT)
                    {
                        surface.blit_xrgb8888(cache_pixels, cache_width, cache_height, 0, 0);
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
                |cache_pixels, cache_width, cache_height, _cache_links| {
                    bitmap.blit_scaled_xrgb8888(
                        cache_pixels,
                        cache_width,
                        cache_height,
                        0,
                        0,
                        cache_width,
                        cache_height,
                    );
                },
            )
        }

        BlockContent::Text(_) | BlockContent::Canvas(_) | BlockContent::PendingImage(_) => None,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the pixel buffer and its size are passed separately until rendering has a paint target type"
)]
pub(crate) fn paint(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    page: &Page,
    layout: &PageLayout,
    address: &str,
    status: &str,
    vector_font_data: Option<&[u8]>,
    block_raster_cache: &mut BlockRasterCache,
    glyph_raster_cache: &mut GlyphRasterCache,
    scroll_y: i32,
    profiling: bool,
) -> PaintResult {
    pixels.fill(BACKGROUND);

    let fallback_font = vector_font_data.and_then(|data| TrueTypeFont::parse(data).ok());

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

    let mut link_regions = Vec::new();
    let slow_block_threshold = Duration::from_millis(16);
    // Opaque block catches erase adjacent overflowing ink. Composite only
    // intersecting blocks directly, while preserving other block caches.
    let direct_blocks = layout.has_text_overflow();
    let mut painted_ink: Option<(i64, i64)> = None;
    let content_top = (CONTENT_TOP as usize).min(height);
    let content_start = content_top.saturating_mul(width);
    let content_end = height.saturating_mul(width);
    let content_height = height - content_top;

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
                    cached.blit_to(pixels, width, height, 16, y);
                } else {
                    let available_width = width.saturating_sub(32);
                    let (svg_width, svg_height) = svg_block_size(svg, available_width);

                    if let Ok(surface) = svg.render(svg_width, svg_height, RasterColor::TRANSPARENT)
                    {
                        if direct_blocks {
                            if let Some(content) = pixels.get_mut(content_start..content_end) {
                                surface.blit_xrgb8888(
                                    content,
                                    width,
                                    content_height,
                                    16,
                                    y.saturating_sub(content_top as i32),
                                );
                            }
                        } else {
                            surface.blit_xrgb8888(pixels, width, height, 16, y);
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
                    cached.blit_to(pixels, width, height, 16, y);
                } else {
                    let available_width = width.saturating_sub(32);
                    let (image_width, image_height) = bitmap_block_size(bitmap, available_width);

                    if direct_blocks {
                        if let Some(content) = pixels.get_mut(content_start..content_end) {
                            bitmap.blit_scaled_xrgb8888(
                                content,
                                width,
                                content_height,
                                16,
                                y.saturating_sub(content_top as i32),
                                image_width,
                                image_height,
                            );
                        }
                    } else {
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
                    cached.blit_to(pixels, width, height, 16, y);
                    cached.append_link_regions(&mut link_regions, 16, y);
                } else if direct_blocks {
                    if let Some(content) = pixels.get_mut(content_start..content_end) {
                        let first_link = link_regions.len();
                        let relative_y = y.saturating_sub(content_top as i32);
                        draw_uncached_text_block(
                            content,
                            width,
                            content_height,
                            relative_y,
                            page,
                            layout,
                            index,
                            fallback_font.as_ref(),
                            0,
                            &mut link_regions,
                            glyph_raster_cache,
                        );
                        for region in &mut link_regions[first_link..] {
                            let top = region.y.saturating_add(content_top as i32);
                            let bottom = top.saturating_add(region.height).min(height as i32);
                            region.y = top.max(content_top as i32);
                            region.height = bottom.saturating_sub(region.y).max(0);
                        }
                    }
                    let top = i64::from(y) + i64::from(paint_top);
                    let bottom = i64::from(y) + i64::from(paint_bottom);
                    painted_ink = Some(match painted_ink {
                        Some((old_top, old_bottom)) => (old_top.min(top), old_bottom.max(bottom)),
                        None => (top, bottom),
                    });
                } else {
                    draw_uncached_text_block(
                        pixels,
                        width,
                        height,
                        y,
                        page,
                        layout,
                        index,
                        fallback_font.as_ref(),
                        CONTENT_TOP,
                        &mut link_regions,
                        glyph_raster_cache,
                    );
                }
            }

            BlockContent::Canvas(canvas) => {
                // blit_canvas uses window coordinates and clips to CONTENT_TOP.
                blit_canvas(canvas.canvas.surface(), pixels, width, height, 16, y);
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

#[expect(
    clippy::too_many_arguments,
    reason = "the pixel buffer and its size are passed separately until rendering has a paint target type"
)]
fn draw_uncached_text_block(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    y: i32,
    page: &Page,
    layout: &PageLayout,
    index: usize,
    fallback: Option<&TrueTypeFont<'_>>,
    visible_top: i32,
    links: &mut Vec<LinkRegion>,
    glyph_cache: &mut GlyphRasterCache,
) {
    let block = &page.blocks[index];
    let BlockContent::Text(text) = &block.content else {
        return;
    };
    let right = i32::try_from(width.saturating_sub(16)).unwrap_or(i32::MAX);
    if let Some(runs) = layout.block_text_runs(index) {
        draw_positioned_text(
            pixels,
            width,
            height,
            16,
            y,
            right,
            runs,
            page,
            fallback,
            block.link.as_deref(),
            visible_top,
            links,
            glyph_cache,
        );
        return;
    }
    let fonts = page.vector_fonts_for_style(&block.style);
    let font = fonts.first().map(|entry| &entry.font).or(fallback);
    let stack = if fonts.len() > 1 {
        fonts.as_slice()
    } else {
        &[]
    };
    let (weight, style) = fonts
        .first()
        .map(|entry| entry.synthesis)
        .unwrap_or((block.style.font_weight, block.style.font_style));
    draw_wrapped_text(
        pixels,
        width,
        height,
        16,
        0.0,
        y.saturating_add(block.style.padding_top),
        0.0,
        right,
        &text.text,
        block.style.font_size,
        weight,
        block.style.font_weight,
        style,
        block.style.color,
        block.link.as_deref(),
        font,
        stack,
        visible_top,
        links,
        glyph_cache,
    );
}

// A canvas is mutable: render it directly and only visit visible pixels.
fn blit_canvas(
    source: &RasterSurface,
    pixels: &mut [u32],
    width: usize,
    height: usize,
    x: i32,
    y: i32,
) {
    if width == 0 {
        return;
    }

    let height = height.min(pixels.len() / width);

    let (Ok(width_i64), Ok(height_i64)) = (i64::try_from(width), i64::try_from(height)) else {
        return;
    };

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

#[expect(
    clippy::too_many_arguments,
    reason = "the pixel buffer and its size are passed separately until rendering has a paint target type"
)]
fn draw_positioned_text(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    left: i32,
    top: i32,
    right: i32,
    runs: &[PositionedTextRun],
    page: &Page,
    fallback: Option<&TrueTypeFont<'_>>,
    link_target: Option<&str>,
    visible_top: i32,
    links: &mut Vec<LinkRegion>,
    glyph_cache: &mut GlyphRasterCache,
) {
    let fonts = InlineFonts::from_styles(page, runs.iter().map(|run| &run.style));

    for run in runs {
        let style = &run.style;
        let font = fonts.get(style).or(fallback);

        draw_wrapped_text(
            pixels,
            width,
            height,
            left.saturating_add(run.x),
            run.offset_x,
            top.saturating_add(run.y),
            run.offset_y,
            right,
            &run.text,
            style.font_size,
            fonts.synthetic_weight(style),
            style.font_weight,
            fonts.synthetic_style(style),
            style.color,
            run.link.as_deref().or(link_target),
            font,
            fonts.stack(style),
            visible_top,
            links,
            glyph_cache,
        );
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the pixel buffer and its size are passed separately until rendering has a paint target type"
)]
fn draw_wrapped_text(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    left: i32,
    offset_x: f32,
    mut y: i32,
    offset_y: f32,
    right: i32,
    text: &str,
    font_size: f32,
    font_weight: FontWeight,
    fallback_weight: FontWeight,
    font_style: FontStyle,
    color: u32,
    link_target: Option<&str>,
    vector_font: Option<&TrueTypeFont<'_>>,
    font_stack: &[ResolvedFont<'_>],
    visible_top: i32,
    link_regions: &mut Vec<LinkRegion>,
    glyph_raster_cache: &mut GlyphRasterCache,
) -> i32 {
    if font_size == 0.0 {
        return y;
    }

    let metrics = stack_text_metrics(vector_font, font_stack, font_size);
    let (ink_top, ink_bottom) = text_ink_bounds(vector_font, font_stack, font_size, offset_y);
    let mut wrap = TextWrap::new(left, right);

    for word in split_layout_words(text) {
        let word_width = stack_word_width(
            vector_font,
            font_stack,
            word,
            font_size,
            font_weight,
            font_style,
        );

        let placement = wrap.place_word(metrics.character_width, word_width);

        if placement.wrapped {
            y = y.saturating_add(metrics.line_height);
        }

        let word_x = placement.x;
        let is_visible =
            y.saturating_add(ink_bottom) > visible_top && y.saturating_add(ink_top) < height as i32;

        if is_visible {
            let rendered_with_vector_font = vector_font.is_some_and(|font| {
                draw_vector_text_line(
                    pixels,
                    width,
                    height,
                    word_x,
                    offset_x,
                    y,
                    offset_y,
                    word,
                    font_size,
                    font_weight,
                    font_style,
                    color,
                    font,
                    font_stack,
                    glyph_raster_cache,
                )
            });

            if !rendered_with_vector_font {
                draw_text_line(
                    pixels,
                    width,
                    height,
                    word_x,
                    y,
                    word,
                    metrics.bitmap_scale,
                    color,
                );

                if fallback_weight == FontWeight::Bold {
                    draw_text_line(
                        pixels,
                        width,
                        height,
                        word_x.saturating_add(1),
                        y,
                        word,
                        metrics.bitmap_scale,
                        color,
                    );
                }
            }

            if let Some(target) = link_target {
                link_regions.push(LinkRegion {
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

#[expect(
    clippy::too_many_arguments,
    reason = "the pixel buffer and its size are passed separately until rendering has a paint target type"
)]
fn rasterize_vector_glyph(
    font: &TrueTypeFont<'_>,
    glyph_id: u16,
    font_size: f32,
    font_weight: FontWeight,
    font_style: FontStyle,
    phase_x: f32,
    phase_y: f32,
    color: u32,
) -> Option<CachedGlyph> {
    let units_per_em = f32::from(font.head().units_per_em);

    if !font_size.is_finite() || font_size <= 0.0 || units_per_em <= 0.0 {
        return None;
    }

    let scale = font_size / units_per_em;
    let italic_shear = synthetic_italic_shear(font_style);
    let bold_offset = synthetic_bold_offset(font_size, font_weight);

    let transform = Transform::scale(scale, -scale)
        .then(Transform::skew_x(italic_shear))
        .then(Transform::translate(phase_x, phase_y));

    let path = font.glyph_path(glyph_id).ok()?.transformed(transform);

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

    rasterize_text_outline(&mut surface, &outline, raster_color(color)).ok()?;

    if font_weight == FontWeight::Bold && bold_offset > 0.0 {
        let bold_outline = TextOutline {
            path: outline
                .path
                .transformed(Transform::translate(bold_offset, 0.0)),
            advance_width: 0.0,
        };

        rasterize_text_outline(&mut surface, &bold_outline, raster_color(color)).ok()?;
    }

    Some(CachedGlyph {
        surface,
        offset_x: left as i32,
        offset_y: top as i32,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "the pixel buffer and its size are passed separately until rendering has a paint target type"
)]
fn draw_vector_text_line(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    x: i32,
    offset_x: f32,
    y: i32,
    offset_y: f32,
    text: &str,
    font_size: f32,
    font_weight: FontWeight,
    font_style: FontStyle,
    color: u32,
    font: &TrueTypeFont<'_>,
    font_stack: &[ResolvedFont<'_>],
    glyph_raster_cache: &mut GlyphRasterCache,
) -> bool {
    let Some(line_metrics) = font.line_metrics(font_size) else {
        return false;
    };

    let ascent = stack_ascent(font_stack, font_size).unwrap_or(line_metrics.ascent);

    let Some((baseline_y, phase_y)) = split_subpixel_position(ascent + offset_y) else {
        return false;
    };

    let baseline_phase = f32::from(phase_y) / GLYPH_SUBPIXEL_STEPS as f32;

    let mut visit = |index: usize, glyph_id: u16, pen_x: f32| {
        let (font, (font_weight, font_style)) = font_stack
            .get(index)
            .map(|entry| (&entry.font, entry.synthesis))
            .unwrap_or((font, (font_weight, font_style)));

        let invalid = || crate::graphics::font::sfnt::FontError {
            offset: 0,
            message: "cannot rasterize vector glyph".to_owned(),
        };

        let (glyph_x, phase_x) = split_subpixel_position(offset_x + pen_x).ok_or_else(invalid)?;

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

        let phase_x = f32::from(phase_x) / GLYPH_SUBPIXEL_STEPS as f32;

        let glyph = glyph_raster_cache
            .get_or_rasterize(key, || {
                rasterize_vector_glyph(
                    font,
                    glyph_id,
                    font_size,
                    font_weight,
                    font_style,
                    phase_x,
                    baseline_phase,
                    color,
                )
            })
            .ok_or_else(invalid)?;

        glyph.blit_to(
            pixels,
            width,
            height,
            x.saturating_add(glyph_x),
            y.saturating_add(baseline_y),
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

#[expect(
    clippy::too_many_arguments,
    reason = "the pixel buffer and its size are passed separately until rendering has a paint target type"
)]
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
        x = x.saturating_add((8 * scale) as i32);
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the pixel buffer and its size are passed separately until rendering has a paint target type"
)]
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
                fill_rect(
                    pixels,
                    width,
                    height,
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

#[expect(
    clippy::too_many_arguments,
    reason = "the pixel buffer and its size are passed separately until rendering has a paint target type"
)]
fn blit_opaque(
    source: &[u32],
    source_width: usize,
    source_height: usize,
    destination: &mut [u32],
    destination_width: usize,
    destination_height: usize,
    origin_x: i32,
    origin_y: i32,
) {
    if source_width == 0 || source_height == 0 || destination_width == 0 || destination_height == 0
    {
        return;
    }

    let source_width_i32 = i32::try_from(source_width).unwrap_or(i32::MAX);
    let source_height_i32 = i32::try_from(source_height).unwrap_or(i32::MAX);
    let destination_width_i32 = i32::try_from(destination_width).unwrap_or(i32::MAX);
    let destination_height_i32 = i32::try_from(destination_height).unwrap_or(i32::MAX);

    let left = origin_x.max(0);
    let top = origin_y.max(0);
    let right = origin_x
        .saturating_add(source_width_i32)
        .min(destination_width_i32);
    let bottom = origin_y
        .saturating_add(source_height_i32)
        .min(destination_height_i32);

    if left >= right || top >= bottom {
        return;
    }

    let source_left = usize::try_from(left - origin_x).unwrap_or(0);
    let source_top = usize::try_from(top - origin_y).unwrap_or(0);
    let copy_width = usize::try_from(right - left).unwrap_or(0);
    let copy_height = usize::try_from(bottom - top).unwrap_or(0);

    for row in 0..copy_height {
        let source_start = (source_top + row) * source_width + source_left;
        let destination_start = (usize::try_from(top).unwrap_or(0) + row) * destination_width
            + usize::try_from(left).unwrap_or(0);

        destination[destination_start..destination_start + copy_width]
            .copy_from_slice(&source[source_start..source_start + copy_width]);
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the pixel buffer and its size are passed separately until rendering has a paint target type"
)]
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
    let end_x = x.saturating_add(rect_width).min(width as i32).max(0) as usize;
    let end_y = y.saturating_add(rect_height).min(height as i32).max(0) as usize;

    for pixel_y in start_y..end_y {
        for pixel_x in start_x..end_x {
            pixels[pixel_y * width + pixel_x] = color;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page_layout::measure_wrapped_text;

    #[test]
    fn line_height_zero_paints_outside_block_and_clips_to_content() {
        let page=crate::page::parse_xhtml_for_test(
            "https://example.test/",
            "<html xmlns='http://www.w3.org/1999/xhtml'><body><div style='line-height:20px'>header</div><div style='line-height:0'><a href='/x' style='color:red'>XXXX</a></div><div style='color:blue'>i</div></body></html>"
        ).unwrap();
        let (width, height) = (128, 150);
        let layout = PageLayout::build(&page, width, None);
        assert!(layout.has_text_overflow());
        assert_eq!(layout.block_height(1), 0);
        let (top, bottom) = layout.block_paint_bounds(1);
        assert!(top < 0 && bottom > 0);
        let mut pixels = vec![0; width * height];
        let mut cache = BlockRasterCache::new();
        let result = paint(
            &mut pixels,
            width,
            height,
            &page,
            &layout,
            "",
            "",
            None,
            &mut cache,
            &mut GlyphRasterCache::new(),
            0,
            false,
        );
        let boundary = (CONTENT_TOP + layout.block_top(1)) as usize;
        assert!(
            (boundary..boundary + 4)
                .any(|y| pixels[y * width + 24..y * width + 48].contains(&0x00ff0000))
        );
        assert!(
            result
                .link_regions
                .iter()
                .any(|region| region.target() == "https://example.test/x")
        );
        assert!(cache.cached(1).is_none());
        assert!(
            cache.cached(0).is_some(),
            "unaffected blocks should stay checked"
        );
        assert!(
            cache.cached(2).is_none(),
            "overlapping blocks cannot use an opaque cache"
        );

        let mut scrolled = vec![0; width * height];
        let result = paint(
            &mut scrolled,
            width,
            height,
            &page,
            &layout,
            "",
            "",
            None,
            &mut BlockRasterCache::new(),
            &mut GlyphRasterCache::new(),
            20,
            false,
        );
        let empty = crate::page::parse_xhtml_for_test(
            "https://example.test/",
            "<html xmlns='http://www.w3.org/1999/xhtml'><body/></html>",
        )
        .unwrap();
        let empty_layout = PageLayout::build(&empty, width, None);
        let mut empty_pixels = vec![0; width * height];
        paint(
            &mut empty_pixels,
            width,
            height,
            &empty,
            &empty_layout,
            "",
            "",
            None,
            &mut BlockRasterCache::new(),
            &mut GlyphRasterCache::new(),
            0,
            false,
        );
        assert_eq!(
            &scrolled[..CONTENT_TOP as usize * width],
            &empty_pixels[..CONTENT_TOP as usize * width]
        );
        assert!(
            result
                .link_regions
                .iter()
                .all(|region| region.y >= CONTENT_TOP)
        );
        assert!(
            result
                .link_regions
                .iter()
                .any(|region| { region.target() == "https://example.test/x" && region.height > 0 })
        );
    }

    #[test]
    fn vector_glyph_survives_zero_height_line() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/fonts/noto/nko-hmtx.woff2");
        let font = normalize_font_container(std::fs::read(path).unwrap()).unwrap();
        let page=crate::page::parse_xhtml_for_test(
            "https://example.test/",
            "<html xmlns='http://www.w3.org/1999/xhtml'><body><div style='line-height:40px'>ߊ</div><div style='line-height:0;font-size:32px;color:red'>ߊ</div></body></html>"
        ).unwrap();
        let (width, height) = (128, 170);
        let layout = PageLayout::build(&page, width, Some(&font));
        assert!(layout.has_text_overflow());
        assert_eq!(layout.block_height(1), 0);
        let mut pixels = vec![0; width * height];
        paint(
            &mut pixels,
            width,
            height,
            &page,
            &layout,
            "",
            "",
            Some(&font),
            &mut BlockRasterCache::new(),
            &mut GlyphRasterCache::new(),
            0,
            false,
        );
        assert!(pixels[CONTENT_TOP as usize * width..].iter().any(|pixel| {
            let red = (pixel >> 16) & 0xff;
            let green = (pixel >> 8) & 0xff;
            red > green && green < 240
        }));
    }

    #[test]
    fn wrapped_text_advances_once_per_word_and_line() {
        let text = "a a a a a a";
        let mut pixels = vec![0; 64 * 30];
        let mut regions = Vec::new();
        let mut cache = GlyphRasterCache::new();

        let end_y = draw_wrapped_text(
            &mut pixels,
            64,
            30,
            0,
            0.0,
            0,
            0.0,
            64,
            text,
            16.0,
            FontWeight::Normal,
            FontWeight::Normal,
            FontStyle::Normal,
            0,
            Some("/link"),
            None,
            &[],
            0,
            &mut regions,
            &mut cache,
        );

        let positions: Vec<_> = regions.iter().map(|region| (region.x, region.y)).collect();

        assert_eq!(
            positions,
            vec![(0, 0), (16, 0), (32, 0), (48, 0), (0, 10), (16, 10)]
        );

        assert_eq!(
            end_y,
            measure_wrapped_text(
                text,
                64,
                16.0,
                FontWeight::Normal,
                FontStyle::Normal,
                None,
                &[],
            )
        );

        assert_eq!(end_y, 20);
    }

    #[test]
    fn long_wrapped_text_does_not_overflow() {
        let text = "a ".repeat(4096);
        let mut pixels = vec![0; 8 * 10];
        let mut regions = Vec::new();
        let mut cache = GlyphRasterCache::new();

        let end_y = draw_wrapped_text(
            &mut pixels,
            8,
            10,
            0,
            0.0,
            0,
            0.0,
            8,
            &text,
            16.0,
            FontWeight::Normal,
            FontWeight::Normal,
            FontStyle::Normal,
            0,
            None,
            None,
            &[],
            0,
            &mut regions,
            &mut cache,
        );

        assert_eq!(end_y, 40_960);

        assert_eq!(
            end_y,
            measure_wrapped_text(
                &text,
                8,
                16.0,
                FontWeight::Normal,
                FontStyle::Normal,
                None,
                &[],
            ),
        );
    }

    #[test]
    fn glyph_cache_distinguishes_vertical_subpixel_phases() {
        use crate::graphics::font::normalize_font_container;

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/fonts/noto/nko-hmtx.woff2"
        );

        let data =
            normalize_font_container(std::fs::read(path).expect("Nko font fixture must exist"))
                .unwrap();

        let font = TrueTypeFont::parse(&data).unwrap();

        let render = |offset_x: f32, offset_y: f32, cache: &mut GlyphRasterCache| {
            let mut pixels = vec![0x00_FF_FF_FF; 80 * 80];

            assert!(draw_vector_text_line(
                &mut pixels,
                80,
                80,
                10,
                offset_x,
                10,
                offset_y,
                "ߊ",
                32.0,
                FontWeight::Normal,
                FontStyle::Normal,
                0x00_00_00_00,
                &font,
                &[],
                cache,
            ));

            pixels
        };

        let mut cache = GlyphRasterCache::new();

        let at_zero = render(0.0, 0.0, &mut cache);
        let at_half = render(0.0, 0.5, &mut cache);

        assert_eq!(cache.entries.len(), 2);
        assert_ne!(at_zero, at_half);

        // Repeated draws must reuse the correct cached glyph.
        assert_eq!(at_half, render(0.0, 0.5, &mut cache));
        assert_eq!(at_zero, render(0.0, 0.0, &mut cache));
        assert_eq!(cache.entries.len(), 2);

        // A fresh cache must produce the same result.
        assert_eq!(at_half, render(0.0, 0.5, &mut GlyphRasterCache::new()));

        // A whole-pixel shift reuses the same phase and moves the image down.
        let at_one = render(0.0, 1.0, &mut cache);

        assert_eq!(&at_one[80..], &at_zero[..79 * 80]);
        assert_eq!(cache.entries.len(), 2);

        let right_half = render(0.5, 0.0, &mut cache);

        assert_ne!(right_half, at_zero);
        assert_eq!(cache.entries.len(), 3);
        assert_eq!(right_half, render(0.5, 0.0, &mut cache));

        let right_one = render(1.0, 0.0, &mut cache);

        for y in 0..80 {
            assert_eq!(
                &right_one[y * 80 + 1..(y + 1) * 80],
                &at_zero[y * 80..y * 80 + 79]
            );
        }

        assert_eq!(cache.entries.len(), 3);
    }

    #[test]
    fn styled_text_without_block_cache_matches_cached_rendering() {
        let page = crate::page::parse_xhtml_for_test(
            "https://example.test/",
            concat!(
                "<html xmlns='http://www.w3.org/1999/xhtml'>",
                "<body><div>A <span style='color:red'>B</span> ",
                "<strong>C</strong></div></body></html>"
            ),
        )
        .unwrap();

        let layout = PageLayout::build(&page, 64, None);
        assert!(layout.block_text_runs(0).is_some());

        let draw = |cache: &mut BlockRasterCache| {
            let mut pixels = vec![0; 64 * 130];

            paint(
                &mut pixels,
                64,
                130,
                &page,
                &layout,
                "",
                "",
                None,
                cache,
                &mut GlyphRasterCache::new(),
                0,
                false,
            );

            pixels
        };

        let cached = draw(&mut BlockRasterCache::new());

        let mut disabled = BlockRasterCache::new();
        disabled.entries.push(BlockRasterCacheEntry::Uncacheable);

        let direct = draw(&mut disabled);

        assert_eq!(cached, direct);
    }

    #[test]
    fn styled_link_remains_clickable_with_and_without_cache() {
        let page = crate::page::parse_xhtml_for_test(
            "https://example.test/",
            concat!(
                "<html xmlns='http://www.w3.org/1999/xhtml'>",
                "<head><style>a strong { color: red; }</style></head>",
                "<body><a href='/target'>A <strong>B</strong> C</a></body>",
                "</html>"
            ),
        )
        .unwrap();

        let layout = PageLayout::build(&page, 64, None);
        assert!(layout.block_text_runs(0).is_some());

        let run = |cache: &mut BlockRasterCache| {
            let mut pixels = vec![0; 64 * 130];

            let result = paint(
                &mut pixels,
                64,
                130,
                &page,
                &layout,
                "",
                "",
                None,
                cache,
                &mut GlyphRasterCache::new(),
                0,
                false,
            );

            (pixels, result.link_regions)
        };

        let (cached_pixels, cached_regions) = run(&mut BlockRasterCache::new());

        let mut cache = BlockRasterCache::new();
        cache.entries.push(BlockRasterCacheEntry::Uncacheable);

        let (direct_pixels, direct_regions) = run(&mut cache);

        assert_eq!(cached_pixels, direct_pixels);
        assert!(cached_pixels[90 * 64..].contains(&0x00ff0000));

        for regions in [&cached_regions, &direct_regions] {
            for (x, y) in [(16, 90), (32, 90), (16, 100)] {
                assert!(
                    regions.iter().any(|region| {
                        region.contains(x, y) && region.target() == "https://example.test/target"
                    }),
                    "missing link at {x},{y}"
                );
            }
        }
    }

    #[test]
    fn link_targets_survive_inline_layout_and_cache() {
        use crate::page::{TextBlock, TextRun};

        let mut page = crate::page::parse_xhtml_for_test(
            "https://example.test/",
            "<html xmlns='http://www.w3.org/1999/xhtml'><body><p>abcd</p></body></html>",
        )
        .unwrap();

        let block = &mut page.blocks[0];
        let style = block.style.clone();

        block.content = BlockContent::Text(TextBlock {
            text: "abcd".into(),
            runs: vec![
                TextRun {
                    range: 0..1,
                    style: style.clone(),
                    link: None,
                },
                TextRun {
                    range: 1..3,
                    style: style.clone(),
                    link: Some("https://example.test/bc".into()),
                },
                TextRun {
                    range: 3..4,
                    style,
                    link: None,
                },
            ],
        });

        let layout = PageLayout::build(&page, 120, None);
        let runs = layout.block_text_runs(0).expect("positioned runs");

        assert_eq!(
            runs.iter()
                .map(|run| (run.text.as_str(), run.link.as_deref()))
                .collect::<Vec<_>>(),
            [
                ("a", None),
                ("bc", Some("https://example.test/bc")),
                ("d", None)
            ]
        );

        let paint_page = |cache: &mut BlockRasterCache| {
            let mut pixels = vec![0; 120 * 120];

            let result = paint(
                &mut pixels,
                120,
                120,
                &page,
                &layout,
                "",
                "",
                None,
                cache,
                &mut GlyphRasterCache::new(),
                0,
                false,
            );

            (pixels, result.link_regions)
        };

        let (cached_pixels, cached_regions) = paint_page(&mut BlockRasterCache::new());

        let mut no_cache = BlockRasterCache::new();
        no_cache.entries.push(BlockRasterCacheEntry::Uncacheable);

        let (direct_pixels, direct_regions) = paint_page(&mut no_cache);

        assert_eq!(cached_pixels, direct_pixels);

        for regions in [&cached_regions, &direct_regions] {
            assert!(regions.iter().any(|region| {
                region.contains(24, 90) && region.target() == "https://example.test/bc"
            }));

            assert!(
                !regions
                    .iter()
                    .any(|region| { region.contains(16, 90) || region.contains(40, 90) })
            );
        }
    }

    #[test]
    fn inline_html_links_render_and_remain_clickable_with_fallback() {
        for html in [
            "<p>aa <a href='/one'>bb</a> cc <a href='/two'>dd</a> ee</p>",
            "<p>aa <a href='/one'><span style='font-size:24px'>bb</span></a> cc <a href='/two'>dd</a> ee</p>",
            "<p>aa\u{a0}<a href='/one'>bb</a> cc <a href='/two'>dd</a> ee</p>",
        ] {
            let html =
                format!("<html xmlns='http://www.w3.org/1999/xhtml'><body>{html}</body></html>");

            let page = crate::page::parse_xhtml_for_test("https://example.test/", &html).unwrap();

            assert_eq!(page.blocks.len(), 1);

            let layout = PageLayout::build(&page, 80, None);
            let runs = layout.block_text_runs(0).expect("positioned link runs");

            let targets: Vec<_> = runs.iter().filter_map(|run| run.link.as_deref()).collect();

            assert_eq!(
                targets,
                ["https://example.test/one", "https://example.test/two"]
            );

            let first = runs
                .iter()
                .find(|run| run.link.as_deref() == Some("https://example.test/one"))
                .unwrap();

            let second = runs
                .iter()
                .find(|run| run.link.as_deref() == Some("https://example.test/two"))
                .unwrap();

            assert!(second.y > first.y);

            let draw = |cache: &mut BlockRasterCache| {
                let mut pixels = vec![0; 80 * 160];

                let result = paint(
                    &mut pixels,
                    80,
                    160,
                    &page,
                    &layout,
                    "",
                    "",
                    None,
                    cache,
                    &mut GlyphRasterCache::new(),
                    0,
                    false,
                );

                (pixels, result.link_regions)
            };

            let (cached, regions) = draw(&mut BlockRasterCache::new());

            let mut disabled = BlockRasterCache::new();
            disabled.entries.push(BlockRasterCacheEntry::Uncacheable);

            let (direct, direct_regions) = draw(&mut disabled);

            assert_eq!(cached, direct);

            for regions in [&regions, &direct_regions] {
                for run in runs {
                    let x = 16 + run.x;
                    let y = CONTENT_TOP + run.y;

                    if let Some(target) = &run.link {
                        assert!(
                            regions.iter().any(|region| {
                                region.contains(x, y) && region.target() == target
                            }),
                            "{html}"
                        );
                    } else {
                        assert!(
                            !regions.iter().any(|region| region.contains(x, y)),
                            "{html}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn mixed_inline_sizes_align_and_preserve_link_hit_testing() {
        let page = crate::page::parse_xhtml_for_test(
            "https://example.test/",
            concat!(
                "<html xmlns='http://www.w3.org/1999/xhtml'><body>",
                "<p>A<a href='/link' style='font-size:32px;color:red'>B</a>C</p>",
                "</body></html>",
            ),
        )
        .unwrap();

        let layout = PageLayout::build(&page, 120, None);
        let runs = layout.block_text_runs(0).expect("mixed-size runs");

        assert_eq!(runs.len(), 3);

        assert_eq!(
            runs.iter()
                .map(|run| run.style.font_size)
                .collect::<Vec<_>>(),
            [16.0, 32.0, 16.0]
        );

        assert!(runs[0].y > runs[1].y);
        assert_eq!(runs[0].y, runs[2].y);
        assert_eq!(runs[1].link.as_deref(), Some("https://example.test/link"));
        assert!(runs[0].link.is_none() && runs[2].link.is_none());
        assert!(runs[2].x > runs[1].x);

        let draw = |cache: &mut BlockRasterCache| {
            let mut pixels = vec![0; 120 * 150];

            let result = paint(
                &mut pixels,
                120,
                150,
                &page,
                &layout,
                "",
                "",
                None,
                cache,
                &mut GlyphRasterCache::new(),
                0,
                false,
            );

            (pixels, result.link_regions)
        };

        let (pixels, regions) = draw(&mut BlockRasterCache::new());

        let mut no_cache = BlockRasterCache::new();
        no_cache.entries.push(BlockRasterCacheEntry::Uncacheable);

        let (direct, direct_regions) = draw(&mut no_cache);

        assert_eq!(pixels, direct);
        assert!(pixels[90 * 120..].contains(&0x00ff0000));

        for regions in [&regions, &direct_regions] {
            assert!(regions.iter().any(|region| {
                region.contains(16 + runs[1].x, 90 + runs[1].y)
                    && region.target() == "https://example.test/link"
            }));

            assert!(!regions.iter().any(|region| {
                region.contains(16 + runs[0].x, 90 + runs[0].y)
                    || region.contains(16 + runs[2].x, 90 + runs[2].y)
            }));
        }
    }

    #[test]
    fn nbsp_stays_unbroken_and_blank_in_normal_and_link_text() {
        let parse = |text: &str| {
            crate::page::parse_xhtml_for_test(
                "https://example.test/",
                &format!("<html xmlns='http://www.w3.org/1999/xhtml'><body>{text}</body></html>"),
            )
            .unwrap()
        };

        let plain = parse("<p>A\u{00a0}B</p>");
        let regular = parse("<p>A B</p>");

        assert_eq!(PageLayout::build(&plain, 48, None).block_height(0), 10);
        assert_eq!(PageLayout::build(&regular, 48, None).block_height(0), 20);

        for html in [
            "<p>A<a href='/x'>\u{00a0}B</a> C</p>",
            "<p>A<a href='/x' style='font-family:Other'>\u{00a0}B</a> C</p>",
        ] {
            let page = parse(html);
            let layout = PageLayout::build(&page, 64, None);
            let runs = layout.block_text_runs(0).unwrap();

            assert_eq!(
                runs.iter().map(|run| run.text.as_str()).collect::<Vec<_>>(),
                ["A", "\u{00a0}B", "C"]
            );

            assert_eq!((runs[0].y, runs[1].y), (0, 0));
            assert!(runs[2].y > 0);

            let draw = |cache: &mut BlockRasterCache| {
                let mut pixels = vec![0; 64 * 130];

                let result = paint(
                    &mut pixels,
                    64,
                    130,
                    &page,
                    &layout,
                    "",
                    "",
                    None,
                    cache,
                    &mut GlyphRasterCache::new(),
                    0,
                    false,
                );

                (pixels, result.link_regions)
            };

            let (pixels, regions) = draw(&mut BlockRasterCache::new());

            let mut disabled = BlockRasterCache::new();
            disabled.entries.push(BlockRasterCacheEntry::Uncacheable);

            let (direct, direct_regions) = draw(&mut disabled);

            assert_eq!(pixels, direct);

            for y in 90..98 {
                for x in 24..32 {
                    assert_eq!(pixels[y * 64 + x], BACKGROUND, "{html}");
                }
            }

            for regions in [&regions, &direct_regions] {
                assert!(regions.iter().any(|region| {
                    region.contains(32, 90) && region.target() == "https://example.test/x"
                }));
            }
        }

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/fonts/noto/NotoSansMongolian-regular.woff2"
        );

        let bytes = normalize_font_container(std::fs::read(path).unwrap()).unwrap();
        let font = TrueTypeFont::parse(&bytes).unwrap();

        let page = parse("<p>ᠠ<a href='/x' style='color:black'>\u{00a0}</a>ᠠ</p>");
        let layout = PageLayout::build(&page, 120, Some(&bytes));
        let runs = layout.block_text_runs(0).unwrap();

        assert_eq!(runs.len(), 3);

        let mut positions = Vec::new();

        crate::graphics::font::layout::visit_positioned_glyphs(
            &[&font],
            "ᠠ\u{00a0}ᠠ",
            16.0,
            |_, _, x| {
                positions.push(x);
                Ok(())
            },
        )
        .unwrap();

        for (run, x) in runs.iter().zip(positions) {
            let (base, phase) = split_subpixel_position(x).unwrap();

            assert_eq!(
                (run.x, run.offset_x),
                (base, f32::from(phase) / GLYPH_SUBPIXEL_STEPS as f32)
            );
        }
    }

    #[test]
    fn inherited_nowrap_keeps_one_line_and_link_clickable() {
        let parse = |body: &str| {
            crate::page::parse_xhtml_for_test(
                "https://example.test/",
                &format!("<html xmlns='http://www.w3.org/1999/xhtml'><body>{body}</body></html>"),
            )
            .unwrap()
        };

        let page = parse("<div style='white-space:nowrap'><p>A <a href='/x'>B</a> C</p></div>");

        assert_eq!(page.blocks.len(), 1);

        let layout = PageLayout::build(&page, 64, None);
        assert_eq!(layout.block_height(0), 10);

        let runs = layout.block_text_runs(0).unwrap();
        assert!(runs.iter().all(|run| run.y == 0));

        let draw = |cache: &mut BlockRasterCache| {
            let mut pixels = vec![0; 64 * 130];

            let result = paint(
                &mut pixels,
                64,
                130,
                &page,
                &layout,
                "",
                "",
                None,
                cache,
                &mut GlyphRasterCache::new(),
                0,
                false,
            );

            (pixels, result.link_regions)
        };

        let (pixels, regions) = draw(&mut BlockRasterCache::new());

        let mut no_cache = BlockRasterCache::new();
        no_cache.entries.push(BlockRasterCacheEntry::Uncacheable);

        let (direct, direct_regions) = draw(&mut no_cache);

        assert_eq!(pixels, direct);

        for regions in [&regions, &direct_regions] {
            assert!(regions.iter().any(|region| {
                region.contains(32, 90) && region.target() == "https://example.test/x"
            }));
        }

        for keyword in ["normal", "initial"] {
            let page = parse(&format!(
                "<div style='white-space:nowrap'><p style='white-space: {keyword}'>A B C</p></div>"
            ));

            assert_eq!(
                PageLayout::build(&page, 64, None).block_height(0),
                20,
                "{keyword}"
            );
        }

        for keyword in ["inherit", "unset"] {
            let page = parse(&format!(
                "<div style='white-space:nowrap'><p style='white-space:normal;white-space:{keyword}'>A B C</p></div>"
            ));

            assert_eq!(
                PageLayout::build(&page, 64, None).block_height(0),
                10,
                "{keyword}"
            );
        }

        let page = parse("<p style='white-space:nowrap;white-space:pre'>A B C</p>");
        assert_eq!(PageLayout::build(&page, 64, None).block_height(0), 10);

        let page =
            parse("<p style='white-space:nowrap'>A <span style='font-family:Other'>B</span> C</p>");

        let fallback_layout = PageLayout::build(&page, 64, None);

        assert_eq!(fallback_layout.block_height(0), 10);
        assert!(fallback_layout.block_text_runs(0).is_some());
    }

    #[test]
    fn canvas_is_mutable_clipped_and_not_cached() {
        let mut page = crate::page::parse_xhtml_for_test(
            "https://example.test/",
            concat!(
                "<html xmlns='http://www.w3.org/1999/xhtml'>",
                "<body><canvas width='4' height='4'></canvas></body>",
                "</html>"
            ),
        )
        .unwrap();

        let layout = PageLayout::build(&page, 64, None);
        let mut cache = BlockRasterCache::new();

        for (color, expected) in [
            (RasterColor::rgba(255, 0, 0, 255), 0xff0000),
            (RasterColor::rgba(0, 255, 0, 255), 0x00ff00),
        ] {
            let BlockContent::Canvas(canvas) = &mut page.blocks[0].content else {
                panic!("canvas expected");
            };

            canvas.canvas.set_fill_style(color);
            canvas.canvas.fill_rect(0.0, 0.0, 4.0, 4.0);

            let mut pixels = vec![0; 64 * 100];

            paint(
                &mut pixels,
                64,
                100,
                &page,
                &layout,
                "",
                "",
                None,
                &mut cache,
                &mut GlyphRasterCache::new(),
                2,
                false,
            );

            assert_eq!(pixels[90 * 64 + 16], expected);
            assert_eq!(pixels[89 * 64 + 16], BACKGROUND);
        }
    }

    #[test]
    fn canvas_renders_and_clips_when_text_overflows() {
        let mut page=crate::page::parse_xhtml_for_test(
            "https://example.test/",
            "<html xmlns='http://www.w3.org/1999/xhtml'><body><div style='line-height:0'>X</div><canvas width='4' height='4'></canvas></body></html>",
        ).unwrap();
        let layout = PageLayout::build(&page, 64, None);
        assert!(layout.has_text_overflow());
        let index = page
            .blocks
            .iter()
            .position(|block| matches!(block.content, BlockContent::Canvas(_)))
            .unwrap();
        let BlockContent::Canvas(canvas) = &mut page.blocks[index].content else {
            unreachable!();
        };
        canvas
            .canvas
            .set_fill_style(RasterColor::rgba(255, 0, 0, 255));
        canvas.canvas.fill_rect(0.0, 0.0, 4.0, 4.0);

        let mut pixels = vec![0; 64 * 100];
        paint(
            &mut pixels,
            64,
            100,
            &page,
            &layout,
            "",
            "",
            None,
            &mut BlockRasterCache::new(),
            &mut GlyphRasterCache::new(),
            2,
            false,
        );
        assert_eq!(pixels[90 * 64 + 16], 0xff0000);
        assert_eq!(pixels[89 * 64 + 16], BACKGROUND);
    }
}
