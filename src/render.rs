use crate::{
    css::{FontStyle, FontWeight},
    graphics::{
        font::{
            layout::TextOutline, render::rasterize_text_outline, truetype::TrueTypeFont,
            woff::normalize_font_container,
        },
        geometry::Transform,
        raster::{Color as RasterColor, Surface as RasterSurface},
        svg::SvgDocument,
    },
    page::{BlockContent, Page},
};
use font8x8::{BASIC_FONTS, UnicodeFonts};

const BACKGROUND: u32 = 0x00_F7_F7_F7;
const TOOLBAR: u32 = 0x00_20_25_2B;
const ADDRESS_BAR: u32 = 0x00_38_40_4A;
const LIGHT_TEXT: u32 = 0x00_F4_F6_F8;

const CONTENT_TOP: i32 = 90;

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

impl LinkRegion {
    pub(crate) fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }

    pub(crate) fn target(&self) -> &str {
        &self.target
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn paint(
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
                let page_font = page
                    .font_data_for(
                        block.style.font_family.as_deref(),
                        block.style.font_weight,
                        block.style.font_style,
                    )
                    .and_then(|data| TrueTypeFont::parse(data).ok());

                let vector_font = page_font.as_ref().or(fallback_font.as_ref());

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
                    vector_font,
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
