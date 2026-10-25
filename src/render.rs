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
    page_layout::{
        CONTENT_TOP, PageLayout, bitmap_block_size, svg_block_size, synthetic_bold_offset,
        synthetic_italic_overhang, synthetic_italic_shear, text_metrics, text_word_width,
    },
};
use font8x8::{BASIC_FONTS, UnicodeFonts};

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
    layout: &PageLayout,
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

    for (index, block) in page.blocks.iter().enumerate() {
        let y = CONTENT_TOP + layout.block_top(index) - scroll_y;

        if y + layout.block_height(index) <= CONTENT_TOP || y >= height as i32 {
            continue;
        }

        match &block.content {
            BlockContent::Svg(svg) => {
                let available_width = width.saturating_sub(32);
                let (svg_width, svg_height) = svg_block_size(svg, available_width);

                if let Ok(surface) = svg.render(svg_width, svg_height, RasterColor::TRANSPARENT) {
                    surface.blit_xrgb8888(pixels, width, height, 16, y);
                }
            }

            BlockContent::Bitmap(bitmap) => {
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

            BlockContent::Text(text) => {
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
                    &mut link_regions,
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
        let is_visible = y + metrics.glyph_height > CONTENT_TOP && y < height as i32;

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
