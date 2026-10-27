use crate::{
    css::{FontStyle, FontWeight},
    graphics::{
        font::{TextOutline, TrueTypeFont, normalize_font_container, rasterize_text_outline},
        geometry::Transform,
        raster::{Color as RasterColor, Surface as RasterSurface},
    },
    page::{BlockContent, Page},
    page_layout::{
        CONTENT_TOP, PageLayout, bitmap_block_size, svg_block_size, synthetic_bold_offset,
        synthetic_italic_shear, text_metrics, text_word_width,
    },
};
use font8x8::{BASIC_FONTS, UnicodeFonts};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

const BACKGROUND: u32 = 0x00_F7_F7_F7;
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
const GLYPH_SUBPIXEL_STEPS: i32 = 8;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct GlyphRasterKey {
    font_identity: usize,
    glyph_id: u16,
    font_size_bits: u32,
    bold: bool,
    italic: bool,
    color: u32,
    phase_x: u8,
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

    let page_font = page
        .font_data_for(
            block.style.font_family.as_deref(),
            block.style.font_weight,
            block.style.font_style,
        )
        .and_then(|data| TrueTypeFont::parse(data).ok());

    let vector_font = page_font.as_ref().or(fallback_font);

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
                0,
                cache_width as i32,
                text,
                block.style.font_size,
                block.style.font_weight,
                block.style.font_style,
                block.style.color,
                block.link.as_deref(),
                vector_font,
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
                    let page_font = page
                        .font_data_for(
                            block.style.font_family.as_deref(),
                            block.style.font_weight,
                            block.style.font_style,
                        )
                        .and_then(|data| TrueTypeFont::parse(data).ok());

                    let vector_font = page_font.as_ref().or(fallback_font.as_ref());

                    draw_wrapped_text(
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
                        vector_font,
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
    mut y: i32,
    right: i32,
    text: &str,
    font_size: f32,
    font_weight: FontWeight,
    font_style: FontStyle,
    color: u32,
    link_target: Option<&str>,
    vector_font: Option<&TrueTypeFont<'_>>,
    visible_top: i32,
    link_regions: &mut Vec<LinkRegion>,
    glyph_raster_cache: &mut GlyphRasterCache,
) -> i32 {
    let metrics = text_metrics(vector_font, font_size);
    let mut x = left;

    for word in text.split_whitespace() {
        let word_width = text_word_width(vector_font, word, font_size, font_weight, font_style);

        if x != left && x + metrics.character_width + word_width > right {
            x = left;
            y += metrics.line_height;
        }

        if x != left {
            x += metrics.character_width;
        }

        let word_x = x;
        let is_visible = y + metrics.glyph_height > visible_top && y < height as i32;

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

                if font_weight == FontWeight::Bold {
                    draw_text_line(
                        pixels,
                        width,
                        height,
                        word_x + 1,
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

        x += word_width;
    }

    y + metrics.line_height
}

fn split_subpixel_position(value: f32) -> Option<(i32, u8)> {
    if !value.is_finite() {
        return None;
    }

    let floor = value.floor();

    if floor < i32::MIN as f32 || floor > i32::MAX as f32 {
        return None;
    }

    let mut base = floor as i32;
    let mut phase = ((value - floor) * GLYPH_SUBPIXEL_STEPS as f32).round() as i32;
    phase = phase.clamp(0, GLYPH_SUBPIXEL_STEPS);

    if phase == GLYPH_SUBPIXEL_STEPS {
        base = base.saturating_add(1);
        phase = 0;
    }

    Some((base, phase as u8))
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
    y: i32,
    text: &str,
    font_size: f32,
    font_weight: FontWeight,
    font_style: FontStyle,
    color: u32,
    font: &TrueTypeFont<'_>,
    glyph_raster_cache: &mut GlyphRasterCache,
) -> bool {
    let Some(line_metrics) = font.line_metrics(font_size) else {
        return false;
    };

    let units_per_em = f32::from(font.head().units_per_em);

    if units_per_em <= 0.0 {
        return false;
    }

    let scale = font_size / units_per_em;
    let Some((baseline_y, baseline_phase)) = split_subpixel_position(line_metrics.ascent) else {
        return false;
    };

    let baseline_phase = f32::from(baseline_phase) / GLYPH_SUBPIXEL_STEPS as f32;
    let mut pen_x = 0.0;
    let mut previous_glyph = None;

    for character in text.chars() {
        let glyph_id = font.glyph_id(character);
        let Some(metric) = font.metric(glyph_id) else {
            return false;
        };

        if let Some(previous_glyph) = previous_glyph {
            pen_x += f32::from(font.kerning(previous_glyph, glyph_id)) * scale;
        }

        let Some((glyph_x, phase_x)) = split_subpixel_position(pen_x) else {
            return false;
        };

        let key = GlyphRasterKey {
            font_identity: font.glyph_cache_key(),
            glyph_id,
            font_size_bits: font_size.to_bits(),
            bold: font_weight == FontWeight::Bold,
            italic: font_style == FontStyle::Italic,
            color,
            phase_x,
        };

        let phase_x = f32::from(phase_x) / GLYPH_SUBPIXEL_STEPS as f32;

        let Some(glyph) = glyph_raster_cache.get_or_rasterize(key, || {
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
        }) else {
            return false;
        };

        glyph.blit_to(
            pixels,
            width,
            height,
            x.saturating_add(glyph_x),
            y.saturating_add(baseline_y),
        );

        pen_x += f32::from(metric.advance_width) * scale;
        previous_glyph = Some(glyph_id);
    }

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
    let end_x = (x + rect_width).min(width as i32).max(0) as usize;
    let end_y = (y + rect_height).min(height as i32).max(0) as usize;

    for pixel_y in start_y..end_y {
        for pixel_x in start_x..end_x {
            pixels[pixel_y * width + pixel_x] = color;
        }
    }
}
