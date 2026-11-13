use crate::{
    css::{FontStyle, FontWeight},
    graphics::{
        font::{
            TextOutline, TrueTypeFont, normalize_font_container, rasterize_text_outline,
            render::{GLYPH_SUBPIXEL_STEPS, split_subpixel_position},
        },
        geometry::Transform,
        raster::{Color as RasterColor, Surface as RasterSurface},
    },
    layout_debug::{clear_preview_content, paint_preview_box},
    layout_preview::{PreviewCommand, PreviewFonts, PreviewLayout},
    page::{BlockContent, Page},
    page_layout::{
        CONTENT_TOP, PageLayout, TextWrap, bitmap_block_size, stack_ascent, stack_text_metrics,
        stack_word_width, svg_block_size, synthetic_bold_offset, synthetic_italic_shear,
    },
};
use font8x8::{BASIC_FONTS, UnicodeFonts};
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

            PreviewCommand::Text {
                rect,
                text,
                style,
                runs,
            } => {
                let relative_y = i64::from(rect.y) - i64::from(scroll_y);

                if rect.height <= 0
                    || relative_y >= content_height as i64
                    || relative_y + i64::from(rect.height) <= 0
                {
                    continue;
                }

                let Ok(y) = i32::try_from(relative_y) else {
                    continue;
                };

                let fonts = PreviewFonts::from_styles(
                    page,
                    std::iter::once(style).chain(runs.iter().map(|run| &run.style)),
                );

                // Restrict text painting to the page content below the toolbar.
                let content_pixels = &mut pixels[content_top * width..width * height];

                let mut paint_text =
                    |x: i32,
                     offset_x: f32,
                     y: i32,
                     offset_y: f32,
                     text: &str,
                     run_style: &crate::css::TextStyle| {
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
                            &font_stack,
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

    let content_width = viewport_width.saturating_sub(32).max(1);
    let block_height = usize::try_from(layout.block_height(index).max(0)).unwrap_or(0);

    let page_fonts = page.vector_fonts_for_style(&block.style);
    let page_font = page_fonts.first();
    let vector_font = page_font.or(fallback_font);

    let font_stack: Vec<_> = if page_fonts.len() > 1 {
        page_fonts.iter().collect()
    } else {
        Vec::new()
    };

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
                0,
                0.0,
                cache_width as i32,
                text,
                block.style.font_size,
                page.synthetic_font_weight(&block.style, page_font.is_some()),
                block.style.font_weight,
                page.synthetic_font_style(&block.style, page_font.is_some()),
                block.style.color,
                block.link.as_deref(),
                vector_font,
                &font_stack,
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

        BlockContent::Text(_) => None,
    }
}

#[allow(clippy::too_many_arguments)]
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

    for (index, block) in page.blocks.iter().enumerate() {
        let y = CONTENT_TOP + layout.block_top(index) - scroll_y;

        if y + layout.block_height(index) <= CONTENT_TOP || y >= height as i32 {
            continue;
        }

        let block_started = profiling.then(Instant::now);

        match &block.content {
            BlockContent::Svg(svg) => {
                if let Some(cached) = cache_visual_block(block_raster_cache, page, index, width) {
                    cached.blit_to(pixels, width, height, 16, y);
                } else {
                    let available_width = width.saturating_sub(32);
                    let (svg_width, svg_height) = svg_block_size(svg, available_width);

                    if let Ok(surface) = svg.render(svg_width, svg_height, RasterColor::TRANSPARENT)
                    {
                        surface.blit_xrgb8888(pixels, width, height, 16, y);
                    }
                }
            }

            BlockContent::Bitmap(bitmap) => {
                if let Some(cached) = cache_visual_block(block_raster_cache, page, index, width) {
                    cached.blit_to(pixels, width, height, 16, y);
                } else {
                    let available_width = width.saturating_sub(32);
                    let (image_width, image_height) = bitmap_block_size(bitmap, available_width);

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

            BlockContent::Text(text) => {
                if let Some(cached) = cache_text_block(
                    block_raster_cache,
                    page,
                    layout,
                    index,
                    width,
                    fallback_font.as_ref(),
                    glyph_raster_cache,
                ) {
                    cached.blit_to(pixels, width, height, 16, y);
                    cached.append_link_regions(&mut link_regions, 16, y);
                } else {
                    let page_fonts = page.vector_fonts_for_style(&block.style);
                    let page_font = page_fonts.first();
                    let vector_font = page_font.or(fallback_font.as_ref());

                    let font_stack: Vec<_> = if page_fonts.len() > 1 {
                        page_fonts.iter().collect()
                    } else {
                        Vec::new()
                    };

                    draw_wrapped_text(
                        pixels,
                        width,
                        height,
                        16,
                        0.0,
                        y,
                        0.0,
                        width as i32 - 16,
                        text,
                        block.style.font_size,
                        page.synthetic_font_weight(&block.style, page_font.is_some()),
                        block.style.font_weight,
                        page.synthetic_font_style(&block.style, page_font.is_some()),
                        block.style.color,
                        block.link.as_deref(),
                        vector_font,
                        &font_stack,
                        CONTENT_TOP,
                        &mut link_regions,
                        glyph_raster_cache,
                    );
                }
            }
        }

        if let Some(block_started) = block_started {
            let elapsed = block_started.elapsed();

            if elapsed >= slow_block_threshold {
                let kind = match &block.content {
                    BlockContent::Text(_) => "text",
                    BlockContent::Svg(_) => "svg",
                    BlockContent::Bitmap(_) => "bitmap",
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
    font_stack: &[&TrueTypeFont<'_>],
    visible_top: i32,
    link_regions: &mut Vec<LinkRegion>,
    glyph_raster_cache: &mut GlyphRasterCache,
) -> i32 {
    if font_size == 0.0 {
        return y;
    }

    let metrics = stack_text_metrics(vector_font, font_stack, font_size);
    let mut wrap = TextWrap::new(left, right);

    for word in text.split_whitespace() {
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
        let visibility_padding = if offset_y == 0.0 { 0 } else { 1 };

        let is_visible = y
            .saturating_add(metrics.glyph_height)
            .saturating_add(visibility_padding)
            > visible_top
            && y.saturating_sub(visibility_padding) < height as i32;

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

#[allow(clippy::too_many_arguments)]
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

#[allow(clippy::too_many_arguments)]
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
    font_stack: &[&TrueTypeFont<'_>],
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
    let single_font = [font];

    let result = crate::graphics::font::layout::visit_positioned_glyphs(
        if font_stack.is_empty() {
            &single_font
        } else {
            font_stack
        },
        text,
        font_size,
        |index, glyph_id, pen_x| {
            let font = font_stack.get(index).copied().unwrap_or(font);
            let invalid = || crate::graphics::font::sfnt::FontError {
                offset: 0,
                message: "cannot rasterize vector glyph".to_owned(),
            };

            let (glyph_x, phase_x) =
                split_subpixel_position(offset_x + pen_x).ok_or_else(invalid)?;

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
        },
    );

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
}
