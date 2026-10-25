use crate::{
    css::{FontStyle, FontWeight},
    graphics::{
        font::{
            layout::TextOutline, render::rasterize_text_outline, truetype::TrueTypeFont,
            woff::normalize_font_container,
        },
        geometry::Transform,
        raster::{Color as RasterColor, Surface as RasterSurface},
    },
    page::{BlockContent, Page},
    page_layout::{
        CONTENT_TOP, PageLayout, bitmap_block_size, svg_block_size, synthetic_bold_offset,
        synthetic_italic_overhang, synthetic_italic_shear, text_metrics, text_word_width,
    },
};
use font8x8::{BASIC_FONTS, UnicodeFonts};
use std::time::{Duration, Instant};

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

fn cache_page_block<'cache>(
    block_raster_cache: &'cache mut BlockRasterCache,
    page: &Page,
    layout: &PageLayout,
    index: usize,
    viewport_width: usize,
    fallback_font: Option<&TrueTypeFont<'_>>,
) -> Option<&'cache CachedBlock> {
    match &page.blocks.get(index)?.content {
        BlockContent::Text(_) => cache_text_block(
            block_raster_cache,
            page,
            layout,
            index,
            viewport_width,
            fallback_font,
        ),

        BlockContent::Svg(_) | BlockContent::Bitmap(_) => {
            cache_visual_block(block_raster_cache, page, index, viewport_width)
        }
    }
}

fn prefetch_next_text_block(
    block_raster_cache: &mut BlockRasterCache,
    page: &Page,
    layout: &PageLayout,
    viewport_width: usize,
    viewport_height: usize,
    scroll_y: i32,
    fallback_font: Option<&TrueTypeFont<'_>>,
) {
    let visible_height = i32::try_from(viewport_height)
        .unwrap_or(i32::MAX)
        .saturating_sub(CONTENT_TOP)
        .max(0);

    let viewport_bottom = scroll_y.saturating_add(visible_height);

    for index in 0..page.blocks.len() {
        if layout.block_top(index) < viewport_bottom {
            continue;
        }

        if !matches!(&page.blocks[index].content, BlockContent::Text(_)) {
            continue;
        }

        let _ = cache_text_block(
            block_raster_cache,
            page,
            layout,
            index,
            viewport_width,
            fallback_font,
        );

        break;
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

    let prefetch_started = profiling.then(Instant::now);

    prefetch_next_text_block(
        block_raster_cache,
        page,
        layout,
        width,
        height,
        scroll_y,
        fallback_font.as_ref(),
    );

    if let Some(prefetch_started) = prefetch_started {
        let elapsed = prefetch_started.elapsed();

        if elapsed >= slow_block_threshold {
            eprintln!(
                "FreeDOM slow prefetch: {:.2} ms",
                elapsed.as_secs_f64() * 1_000.0,
            );
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
    let Ok(run) = font.layout_text(text, font_size) else {
        return false;
    };

    let Some(line_metrics) = font.line_metrics(font_size) else {
        return false;
    };

    let padding = 2_i32;
    let bold_offset = synthetic_bold_offset(font_size, font_weight);
    let italic_shear = synthetic_italic_shear(font_style);
    let italic_overhang = synthetic_italic_overhang(font, font_size, font_style);
    let surface_width = (run.advance_width + bold_offset + italic_overhang)
        .ceil()
        .max(1.0) as usize
        + 4;
    let surface_height = line_metrics.line_height.ceil().max(1.0) as usize + 4;

    let glyph_transform = Transform::skew_x(italic_shear).then(Transform::translate(
        padding as f32,
        line_metrics.ascent + padding as f32,
    ));

    let mut surface = RasterSurface::new(surface_width, surface_height, RasterColor::TRANSPARENT);

    for glyph_path in &run.glyph_paths {
        let glyph_outline = TextOutline {
            path: glyph_path.transformed(glyph_transform),
            advance_width: 0.0,
        };

        if rasterize_text_outline(&mut surface, &glyph_outline, raster_color(color)).is_err() {
            return false;
        }
    }

    if font_weight == FontWeight::Bold && bold_offset > 0.0 {
        let bold_transform = glyph_transform.then(Transform::translate(bold_offset, 0.0));

        for glyph_path in &run.glyph_paths {
            let glyph_outline = TextOutline {
                path: glyph_path.transformed(bold_transform),
                advance_width: 0.0,
            };

            if rasterize_text_outline(&mut surface, &glyph_outline, raster_color(color)).is_err() {
                return false;
            }
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
