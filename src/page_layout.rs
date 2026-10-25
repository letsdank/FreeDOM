use crate::{
    css::{FontStyle, FontWeight},
    graphics::{font::truetype::TrueTypeFont, raster::Surface as RasterSurface, svg::SvgDocument},
    page::{BlockContent, Page},
};

pub(crate) const CONTENT_TOP: i32 = 90;

const CONTENT_HORIZONTAL_PADDING: usize = 16;

pub(crate) struct PageLayout {
    viewport_width: usize,
    blocks: Vec<BlockLayout>,
    content_height: i32,
}

struct BlockLayout {
    top: i32,
    height: i32,
}

#[derive(Clone, Copy)]
pub(crate) struct TextMetrics {
    pub(crate) bitmap_scale: usize,
    pub(crate) character_width: i32,
    pub(crate) glyph_height: i32,
    pub(crate) line_height: i32,
}

impl PageLayout {
    pub(crate) fn build(
        page: &Page,
        viewport_width: usize,
        fallback_font_data: Option<&[u8]>,
    ) -> Self {
        let fallback_font = fallback_font_data.and_then(|data| TrueTypeFont::parse(data).ok());
        let available_width = viewport_width
            .saturating_sub(CONTENT_HORIZONTAL_PADDING * 2)
            .max(1);
        let text_width = i32::try_from(available_width).unwrap_or(i32::MAX);

        let mut blocks = Vec::with_capacity(page.blocks.len());
        let mut top = 0_i32;

        for block in &page.blocks {
            let height = match &block.content {
                BlockContent::Svg(svg) => {
                    let (_, height) = svg_block_size(svg, available_width);
                    i32::try_from(height).unwrap_or(i32::MAX)
                }

                BlockContent::Bitmap(bitmap) => {
                    let (_, height) = bitmap_block_size(bitmap, available_width);
                    i32::try_from(height).unwrap_or(i32::MAX)
                }

                BlockContent::Text(text) => {
                    let page_font = page
                        .font_data_for(
                            block.style.font_family.as_deref(),
                            block.style.font_weight,
                            block.style.font_style,
                        )
                        .and_then(|data| TrueTypeFont::parse(data).ok());

                    let font = page_font.as_ref().or(fallback_font.as_ref());

                    measure_wrapped_text(
                        text,
                        text_width,
                        block.style.font_size,
                        block.style.font_weight,
                        block.style.font_style,
                        font,
                    )
                }
            };

            blocks.push(BlockLayout { top, height });

            top = top
                .saturating_add(height)
                .saturating_add(block.style.margin_after);
        }

        Self {
            viewport_width,
            blocks,
            content_height: top.max(0),
        }
    }

    pub(crate) fn viewport_width(&self) -> usize {
        self.viewport_width
    }

    pub(crate) fn block_top(&self, index: usize) -> i32 {
        self.blocks[index].top
    }

    pub(crate) fn block_height(&self, index: usize) -> i32 {
        self.blocks[index].height
    }

    pub(crate) fn content_height(&self) -> i32 {
        self.content_height
    }
}

pub(crate) fn text_metrics(vector_font: Option<&TrueTypeFont<'_>>, font_size: f32) -> TextMetrics {
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

    TextMetrics {
        bitmap_scale,
        character_width,
        glyph_height: vector_line_metrics
            .map(|metrics| metrics.line_height.ceil() as i32)
            .unwrap_or(bitmap_glyph_height),
        line_height: vector_line_metrics
            .map(|metrics| metrics.line_height.ceil() as i32)
            .unwrap_or(bitmap_line_height),
    }
}

pub(crate) fn measure_wrapped_text(
    text: &str,
    available_width: i32,
    font_size: f32,
    font_weight: FontWeight,
    font_style: FontStyle,
    vector_font: Option<&TrueTypeFont<'_>>,
) -> i32 {
    let metrics = text_metrics(vector_font, font_size);
    let mut line_width = 0_i32;
    let mut line_count = 1_i32;

    for word in text.split_whitespace() {
        let word_width = text_word_width(vector_font, word, font_size, font_weight, font_style);

        if line_width != 0
            && line_width
                .saturating_add(metrics.character_width)
                .saturating_add(word_width)
                > available_width
        {
            line_width = 0;
            line_count = line_count.saturating_add(1);
        }

        if line_width != 0 {
            line_width = line_width.saturating_add(metrics.character_width);
        }

        line_width = line_width.saturating_add(word_width);
    }

    line_count.saturating_mul(metrics.line_height)
}

pub(crate) fn text_word_width(
    vector_font: Option<&TrueTypeFont<'_>>,
    text: &str,
    font_size: f32,
    font_weight: FontWeight,
    font_style: FontStyle,
) -> i32 {
    vector_font
        .and_then(|font| vector_text_width(font, text, font_size, font_weight, font_style))
        .unwrap_or_else(|| {
            i32::try_from(text.chars().count())
                .unwrap_or(i32::MAX)
                .saturating_mul(if font_size >= 24.0 { 16 } else { 8 })
                .saturating_add(i32::from(font_weight == FontWeight::Bold))
        })
}

pub(crate) fn svg_block_size(svg: &SvgDocument, available_width: usize) -> (usize, usize) {
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

pub(crate) fn bitmap_block_size(bitmap: &RasterSurface, available_width: usize) -> (usize, usize) {
    const MAX_HEIGHT: usize = 4096;

    let source_width = bitmap.width().max(1);
    let source_height = bitmap.height().max(1);
    let width = source_width.min(available_width.max(1));

    let height = source_height
        .saturating_mul(width)
        .saturating_add(source_width - 1)
        / source_width;

    (width, height.clamp(1, MAX_HEIGHT))
}

pub(crate) fn vector_text_width(
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

pub(crate) fn synthetic_italic_shear(font_style: FontStyle) -> f32 {
    match font_style {
        FontStyle::Normal => 0.0,
        FontStyle::Italic => -0.212_56,
    }
}

pub(crate) fn synthetic_italic_overhang(
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

pub(crate) fn synthetic_bold_offset(font_size: f32, font_weight: FontWeight) -> f32 {
    match font_weight {
        FontWeight::Normal => 0.0,
        FontWeight::Bold => (font_size / 24.0).clamp(1.0, 2.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_wrapped_bitmap_text_once_per_line() {
        let height = measure_wrapped_text(
            "aa aa",
            20,
            16.0,
            FontWeight::Normal,
            FontStyle::Normal,
            None,
        );

        assert_eq!(height, 20);
    }
}
