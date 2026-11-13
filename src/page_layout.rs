use crate::{
    css::{FontStyle, FontWeight},
    graphics::{font::TrueTypeFont, raster::Surface as RasterSurface, svg::SvgDocument},
    page::{BlockContent, Page, ResolvedFont},
};

pub(crate) const CONTENT_TOP: i32 = 90;

pub(crate) const CONTENT_HORIZONTAL_PADDING: usize = 16;

pub(crate) struct PageLayout {
    viewport_width: usize,
    blocks: Vec<BlockLayout>,
    content_height: i32,
}

struct BlockLayout {
    top: i32,
    width: i32,
    height: i32,
}

/// Geometry of a current layout block in document coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LayoutRect {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: i32,
    pub(crate) height: i32,
}

impl LayoutRect {
    fn contains(&self, x: i32, y: i32) -> bool {
        if self.width <= 0 || self.height <= 0 {
            return false;
        }

        let (x, y) = (i64::from(x), i64::from(y));
        let (left, top) = (i64::from(self.x), i64::from(self.y));

        x >= left
            && x < left + i64::from(self.width)
            && y >= top
            && y < top + i64::from(self.height)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct TextMetrics {
    pub(crate) bitmap_scale: usize,
    pub(crate) character_width: i32,
    pub(crate) glyph_height: i32,
    pub(crate) line_height: i32,
}

/// Shared word-wrapping state used by measurement and painting.
pub(crate) struct TextWrap {
    left: i32,
    right: i32,
    x: i32,
}

pub(crate) struct WordPlacement {
    pub(crate) x: i32,
    pub(crate) wrapped: bool,
}

impl TextWrap {
    pub(crate) fn new(left: i32, right: i32) -> Self {
        Self {
            left,
            right,
            x: left,
        }
    }

    pub(crate) fn place_word(&mut self, space_width: i32, word_width: i32) -> WordPlacement {
        let wrapped = self.x != self.left
            && self
                .x
                .saturating_add(space_width)
                .saturating_add(word_width)
                > self.right;

        if wrapped {
            self.x = self.left;
        }

        if self.x != self.left {
            self.x = self.x.saturating_add(space_width);
        }

        let x = self.x;
        self.x = self.x.saturating_add(word_width);

        WordPlacement { x, wrapped }
    }
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
            let (block_width, height) = match &block.content {
                BlockContent::Svg(svg) => {
                    let (width, height) = svg_block_size(svg, available_width);

                    (
                        i32::try_from(width).unwrap_or(i32::MAX),
                        i32::try_from(height).unwrap_or(i32::MAX),
                    )
                }

                BlockContent::Bitmap(bitmap) => {
                    let (width, height) = bitmap_block_size(bitmap, available_width);

                    (
                        i32::try_from(width).unwrap_or(i32::MAX),
                        i32::try_from(height).unwrap_or(i32::MAX),
                    )
                }

                BlockContent::Text(text) => {
                    let page_fonts = page.vector_fonts_for_style(&block.style);
                    let page_font = page_fonts.first().map(|entry| &entry.font);
                    let font = page_font.or(fallback_font.as_ref());

                    let font_stack = if page_fonts.len() > 1 {
                        page_fonts.as_slice()
                    } else {
                        &[]
                    };

                    let (weight, font_style) = page_fonts
                        .first()
                        .map(|entry| entry.synthesis)
                        .unwrap_or((block.style.font_weight, block.style.font_style));

                    let height = measure_wrapped_text(
                        text,
                        text_width,
                        block.style.font_size,
                        weight,
                        font_style,
                        font,
                        &font_stack,
                    );

                    (text_width, height)
                }
            };

            blocks.push(BlockLayout {
                top,
                width: block_width,
                height,
            });

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

    pub(crate) fn block_rects(&self) -> impl Iterator<Item = LayoutRect> + '_ {
        self.blocks.iter().map(|block| LayoutRect {
            x: CONTENT_HORIZONTAL_PADDING as i32,
            y: block.top,
            width: block.width,
            height: block.height,
        })
    }

    /// Returns the last painted block under a window-relative point.
    pub(crate) fn block_at_viewport_point(
        &self,
        x: i32,
        y: i32,
        viewport_height: usize,
        scroll_y: i32,
    ) -> Option<usize> {
        if x < 0
            || y < CONTENT_TOP
            || usize::try_from(x).ok()? >= self.viewport_width
            || usize::try_from(y).ok()? >= viewport_height
        {
            return None;
        }

        let document_y =
            i32::try_from(i64::from(y) - i64::from(CONTENT_TOP) + i64::from(scroll_y)).ok()?;

        self.block_rects()
            .enumerate()
            .filter_map(|(index, rect)| rect.contains(x, document_y).then_some(index))
            .last()
    }
}

pub(crate) fn text_metrics(vector_font: Option<&TrueTypeFont<'_>>, font_size: f32) -> TextMetrics {
    if font_size == 0.0 {
        return TextMetrics {
            bitmap_scale: 1,
            character_width: 0,
            glyph_height: 0,
            line_height: 0,
        };
    }

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
    font_stack: &[ResolvedFont<'_>],
) -> i32 {
    let metrics = stack_text_metrics(vector_font, font_stack, font_size);
    let mut wrap = TextWrap::new(0, available_width);
    let mut line_count = 1_i32;

    for word in text.split_whitespace() {
        let word_width = stack_word_width(
            vector_font,
            font_stack,
            word,
            font_size,
            font_weight,
            font_style,
        );

        if wrap.place_word(metrics.character_width, word_width).wrapped {
            line_count = line_count.saturating_add(1);
        }
    }

    line_count.saturating_mul(metrics.line_height)
}

pub(crate) fn stack_ascent(font_stack: &[ResolvedFont<'_>], font_size: f32) -> Option<f32> {
    font_stack
        .iter()
        .filter_map(|entry| entry.font.line_metrics(font_size))
        .map(|metrics| metrics.ascent)
        .reduce(f32::max)
}

pub(crate) fn stack_text_metrics(
    font: Option<&TrueTypeFont<'_>>,
    stack: &[ResolvedFont<'_>],
    size: f32,
) -> TextMetrics {
    let mut metrics = text_metrics(font, size);

    if let Some(ascent) = stack_ascent(stack, size) {
        let below = stack
            .iter()
            .filter_map(|entry| entry.font.line_metrics(size))
            .map(|metrics| (metrics.line_height - metrics.ascent).max(0.0))
            .fold(0.0_f32, f32::max);

        let height = (ascent + below).ceil().max(0.0) as i32;

        metrics.line_height = metrics.line_height.max(height);
        metrics.glyph_height = metrics.glyph_height.max(height);
    }

    metrics
}

pub(crate) fn stack_word_width(
    font: Option<&TrueTypeFont<'_>>,
    stack: &[ResolvedFont<'_>],
    text: &str,
    size: f32,
    weight: FontWeight,
    style: FontStyle,
) -> i32 {
    if !stack.is_empty() && size > 0.0 {
        if let Ok(advance) =
            crate::graphics::font::layout::visit_positioned_glyphs(stack, text, size, |_, _, _| {
                Ok(())
            })
        {
            let extra = stack
                .iter()
                .map(|entry| {
                    let (weight, style) = entry.synthesis;

                    synthetic_bold_offset(size, weight)
                        + synthetic_italic_overhang(&entry.font, size, style)
                })
                .fold(0.0_f32, f32::max);

            return (advance + extra).ceil().max(1.0) as i32;
        }
    }

    text_word_width(font, text, size, weight, style)
}

pub(crate) fn text_word_width(
    vector_font: Option<&TrueTypeFont<'_>>,
    text: &str,
    font_size: f32,
    font_weight: FontWeight,
    font_style: FontStyle,
) -> i32 {
    if font_size == 0.0 {
        return 0;
    }

    vector_font
        .and_then(|font| vector_text_width(font, text, font_size, font_weight, font_style))
        .unwrap_or_else(|| {
            bitmap_text_advance(text, font_size)
                .saturating_add(i32::from(font_weight == FontWeight::Bold))
        })
}

pub(crate) fn bitmap_text_advance(text: &str, font_size: f32) -> i32 {
    if font_size == 0.0 {
        return 0;
    }

    i32::try_from(text.chars().count())
        .unwrap_or(i32::MAX)
        .saturating_mul(if font_size >= 24.0 { 16 } else { 8 })
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
    let advance_width = font.text_advance_width(text, font_size).ok()?;

    let extra_width = synthetic_bold_offset(font_size, font_weight)
        + synthetic_italic_overhang(font, font_size, font_style);

    Some((advance_width + extra_width).ceil().max(1.0) as i32)
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
            &[],
        );

        assert_eq!(height, 20);
    }

    #[test]
    fn hit_tests_scrolled_blocks_and_clips_toolbar() {
        let layout = PageLayout {
            viewport_width: 100,

            blocks: vec![
                BlockLayout {
                    top: 0,
                    width: 68,
                    height: 10,
                },
                BlockLayout {
                    top: 10,
                    width: 30,
                    height: 12,
                },
                BlockLayout {
                    top: 10,
                    width: 12,
                    height: 4,
                },
            ],

            content_height: 22,
        };

        assert_eq!(layout.block_at_viewport_point(16, 90, 130, 0), Some(0));

        // The toolbar is outside page content.
        assert_eq!(layout.block_at_viewport_point(16, 89, 130, 0), None);

        // The last painted overlapping block wins.
        assert_eq!(layout.block_at_viewport_point(16, 100, 130, 0), Some(2));
        assert_eq!(layout.block_at_viewport_point(40, 100, 130, 0), Some(1));

        // The same point after scrolling.
        assert_eq!(layout.block_at_viewport_point(16, 90, 130, 10), Some(2));
        assert_eq!(layout.block_at_viewport_point(46, 100, 130, 0), None);
        assert_eq!(layout.block_at_viewport_point(16, 130, 130, 0), None);
        assert_eq!(layout.block_at_viewport_point(100, 100, 130, 0), None);
    }

    #[test]
    fn positions_words_using_each_space_width() {
        let mut wrap = TextWrap::new(10, 40);

        let first = wrap.place_word(99, 10);
        assert_eq!((first.x, first.wrapped), (10, false));

        let second = wrap.place_word(3, 10);
        assert_eq!((second.x, second.wrapped), (23, false));

        let third = wrap.place_word(8, 2);
        assert_eq!((third.x, third.wrapped), (10, true));
    }

    #[test]
    fn wraps_whole_words_and_saturates_coordinates() {
        let mut wrap = TextWrap::new(0, 10);

        let first = wrap.place_word(2, 15);
        assert_eq!((first.x, first.wrapped), (0, false));

        let second = wrap.place_word(2, 3);
        assert_eq!((second.x, second.wrapped), (0, true));

        let mut wrap = TextWrap::new(i32::MAX - 3, i32::MAX);

        let first = wrap.place_word(1, 10);
        assert_eq!(first.x, i32::MAX - 3);

        let second = wrap.place_word(1, 2);
        assert_eq!((second.x, second.wrapped), (i32::MAX, false));
    }
}
