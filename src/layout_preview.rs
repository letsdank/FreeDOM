//! Experimental geometry for a small subset of normal-flow block layout.
//!
//! CSS 2.2, sections 9.4.1 and 10.6.3:
//! <https://www.w3.org/TR/CSS22/visuren.html#block-formatting>
//! <https://www.w3.org/TR/CSS22/visudet.html#normal-block>
//!
//! This is an inspector preview, not the page renderer or a conforming layout.

use crate::{
    box_tree::{BlockBox, BlockContent, BoxTree, BoxTreeError, FlowChild, InlineItem},
    css::{BackgroundColor, BorderEdge, CssSize, TextStyle},
    graphics::font::{
        TrueTypeFont,
        render::{GLYPH_SUBPIXEL_STEPS, split_subpixel_position},
    },
    page::{Page, ResolvedFont},
    page_layout::{
        CONTENT_HORIZONTAL_PADDING, LayoutRect, TextWrap, bitmap_text_advance, stack_ascent,
        stack_text_metrics, stack_word_width, text_metrics, text_word_width,
    },
    styled_tree::SourcePath,
};
use std::fmt::Write;

pub(crate) struct PreviewBox {
    pub(crate) source: SourcePath,
    pub(crate) rect: LayoutRect,
    pub(crate) background_color: BackgroundColor,
    pub(crate) border_top: BorderEdge,
    pub(crate) border_bottom: BorderEdge,
    pub(crate) border_left: BorderEdge,
    pub(crate) border_right: BorderEdge,
}

pub(crate) struct PreviewLayout {
    pub(crate) boxes: Vec<PreviewBox>,
    pub(crate) display_list: Vec<PreviewCommand>,
}

pub(crate) enum PreviewCommand {
    Block(usize),
    Text {
        rect: LayoutRect,
        text: String,
        style: TextStyle,
        runs: Vec<PreviewTextRun>,
    },
}

pub(crate) struct PreviewTextRun {
    pub(crate) x: i32,
    pub(crate) offset_x: f32,
    pub(crate) y: i32,
    pub(crate) offset_y: f32,
    pub(crate) text: String,
    pub(crate) style: TextStyle,
}

struct PendingMargin {
    positive: i32,
    negative: i32,
    path: Vec<usize>,
    has_empty: bool,
}

impl PendingMargin {
    fn new(value: i32, path: Vec<usize>) -> Self {
        Self {
            positive: value.max(0),
            negative: value.min(0),
            path,
            has_empty: false,
        }
    }

    fn add(&mut self, value: i32) {
        self.positive = self.positive.max(value);
        self.negative = self.negative.min(value);
    }

    fn value(&self) -> i32 {
        collapse_sibling_margins(self.positive, self.negative)
    }
}

struct FlowResult {
    end: i32,
    trailing_margin: Option<PendingMargin>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FlowStart {
    Document,
    Collapsed,
    RootContent,
}

#[derive(Debug)]
pub(crate) enum PreviewError {
    Tree(BoxTreeError),
    Margin(Vec<usize>),
    Replaced(Vec<usize>),
    LineBreak(Vec<usize>),
    InlineFont(Vec<usize>),
    WhiteSpace,
    SizeOverflow,
    EmptyBlock(Vec<usize>),
    InlineStyle(Vec<usize>),
    SideBorder(Vec<usize>),
}

impl std::fmt::Display for PreviewError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tree(error) => {
                write!(formatter, "box tree: {error}")
            }

            Self::Margin(path) => {
                write!(
                    formatter,
                    "margin at a flow boundary is not supported at {path:?}"
                )
            }

            Self::EmptyBlock(path) => {
                write!(
                    formatter,
                    "empty block at a flow boundary or with unsupported content at {path:?}"
                )
            }

            Self::Replaced(path) => {
                write!(formatter, "replaced content is not supported at {path:?}")
            }

            Self::LineBreak(path) => {
                write!(formatter, "line breaks are not supported at {path:?}")
            }

            Self::InlineFont(path) => {
                write!(
                    formatter,
                    "mixed inline fonts are not supported at {path:?}"
                )
            }

            Self::WhiteSpace => {
                write!(formatter, "white-space handling is not supported")
            }

            Self::SizeOverflow => {
                write!(formatter, "preview geometry is too large")
            }

            Self::InlineStyle(path) => {
                write!(
                    formatter,
                    "inline color, background, border or horizontal margin is not supported at {path:?}"
                )
            }

            Self::SideBorder(path) => {
                write!(
                    formatter,
                    "side borders combined with top or bottom borders are not supported at {path:?}"
                )
            }
        }
    }
}

impl PreviewLayout {
    pub(crate) fn build(
        page: &Page,
        viewport_width: usize,
        fallback_data: Option<&[u8]>,
    ) -> Result<Self, PreviewError> {
        let tree = BoxTree::build(&page.styled_tree).map_err(PreviewError::Tree)?;

        let width = viewport_width
            .saturating_sub(CONTENT_HORIZONTAL_PADDING * 2)
            .max(1);

        let width = i32::try_from(width).map_err(|_| PreviewError::SizeOverflow)?;

        let fallback = fallback_data.and_then(|data| TrueTypeFont::parse(data).ok());

        let mut output = Self {
            boxes: Vec::new(),
            display_list: Vec::new(),
        };

        let root_style = TextStyle::initial();

        let flow = output.flow(
            &tree.children,
            &root_style,
            page,
            fallback.as_ref(),
            CONTENT_HORIZONTAL_PADDING as i32,
            width,
            0,
            FlowStart::Document,
        )?;

        if let Some(group) = flow.trailing_margin {
            if group.value() < 0 {
                return Err(PreviewError::Margin(group.path));
            }
        }

        Ok(output)
    }

    fn flow(
        &mut self,
        children: &[FlowChild<'_>],
        inherited: &TextStyle,
        page: &Page,
        fallback: Option<&TrueTypeFont<'_>>,
        left: i32,
        width: i32,
        top: i32,
        flow_start: FlowStart,
    ) -> Result<FlowResult, PreviewError> {
        let mut cursor = top;
        let mut pending_margin: Option<PendingMargin> = None;
        let mut at_start = true;

        for child in children {
            match child {
                FlowChild::Block(block) => {
                    if (block.style.border_left.width != 0 || block.style.border_right.width != 0)
                        && (block.style.border_top.width != 0
                            || block.style.border_bottom.width != 0)
                    {
                        return Err(PreviewError::SideBorder(block.source.indices().to_vec()));
                    }

                    let mut previous = pending_margin.take();

                    let is_root = flow_start == FlowStart::Document && block.tag == "html";

                    let leading = if is_root {
                        PendingMargin::new(
                            block.style.margin_before,
                            block.source.indices().to_vec(),
                        )
                    } else {
                        leading_margin(block)?
                    };

                    let start = if let Some(group) = previous.as_mut() {
                        group.add(leading.positive);
                        group.add(leading.negative);

                        cursor
                            .checked_add(group.value())
                            .ok_or(PreviewError::SizeOverflow)?
                    } else if at_start && flow_start == FlowStart::Collapsed {
                        cursor
                    } else if at_start && flow_start == FlowStart::Document && leading.value() != 0
                    {
                        return Err(PreviewError::Margin(block.source.indices().to_vec()));
                    } else {
                        cursor
                            .checked_add(leading.value())
                            .ok_or(PreviewError::SizeOverflow)?
                    };

                    at_start = false;

                    if block.tag == "pre" {
                        return Err(PreviewError::WhiteSpace);
                    }

                    let BlockContent::Flow(children) = &block.content else {
                        return Err(PreviewError::Replaced(block.source.indices().to_vec()));
                    };

                    let index = self.boxes.len();

                    let block_left = left
                        .checked_add(block.style.margin_left)
                        .ok_or(PreviewError::SizeOverflow)?;

                    let content_width = match block.style.width {
                        CssSize::Auto => {
                            let available = i64::from(width)
                                - i64::from(block.style.margin_left)
                                - i64::from(block.style.margin_right)
                                - i64::from(block.style.border_left.width)
                                - i64::from(block.style.border_right.width);

                            i32::try_from(available.max(0))
                                .map_err(|_| PreviewError::SizeOverflow)?
                        }

                        CssSize::Pixels(value) => value,
                    };

                    let content_left = block_left
                        .checked_add(block.style.border_left.width)
                        .ok_or(PreviewError::SizeOverflow)?;

                    let block_width = block
                        .style
                        .border_left
                        .width
                        .checked_add(content_width)
                        .and_then(|value| value.checked_add(block.style.border_right.width))
                        .ok_or(PreviewError::SizeOverflow)?;

                    block_left
                        .checked_add(block_width)
                        .ok_or(PreviewError::SizeOverflow)?;

                    self.boxes.push(PreviewBox {
                        source: block.source.clone(),
                        rect: LayoutRect {
                            x: block_left,
                            y: start,
                            width: block_width,
                            height: 0,
                        },
                        background_color: block.style.background_color,
                        border_top: block.style.border_top,
                        border_bottom: block.style.border_bottom,
                        border_left: block.style.border_left,
                        border_right: block.style.border_right,
                    });

                    self.display_list.push(PreviewCommand::Block(index));

                    let child_start = if is_root || block.style.border_top.width > 0 {
                        FlowStart::RootContent
                    } else {
                        FlowStart::Collapsed
                    };

                    let mut child_flow = self.flow(
                        children,
                        block.style,
                        page,
                        fallback,
                        content_left,
                        content_width,
                        start
                            .checked_add(block.style.border_top.width)
                            .ok_or(PreviewError::SizeOverflow)?,
                        child_start,
                    )?;

                    // The root element's margins do not collapse with its children.
                    // https://www.w3.org/TR/CSS22/box.html#collapsing-margins
                    if (is_root || block.style.border_bottom.width > 0)
                        && block.style.height == CssSize::Auto
                    {
                        if let Some(group) = child_flow.trailing_margin.take() {
                            if group.value() < 0 {
                                return Err(PreviewError::Margin(group.path));
                            }

                            child_flow.end = child_flow
                                .end
                                .checked_add(group.value())
                                .ok_or(PreviewError::SizeOverflow)?;
                        }
                    }

                    let end = match block.style.height {
                        CssSize::Auto => child_flow.end,

                        CssSize::Pixels(height) => {
                            // A fixed-height parent does not collapse its bottom margin
                            // with the last child's bottom margin.
                            child_flow.trailing_margin = None;

                            start
                                .checked_add(block.style.border_top.width)
                                .and_then(|value| value.checked_add(height))
                                .ok_or(PreviewError::SizeOverflow)?
                        }
                    };

                    let end = end
                        .checked_add(block.style.border_bottom.width)
                        .ok_or(PreviewError::SizeOverflow)?;

                    let height = end.checked_sub(start).ok_or(PreviewError::SizeOverflow)?;

                    if height < 0 || (height == 0 && !children.is_empty()) {
                        return Err(PreviewError::EmptyBlock(block.source.indices().to_vec()));
                    }

                    if height == 0 {
                        let Some(mut group) = previous else {
                            return Err(PreviewError::EmptyBlock(block.source.indices().to_vec()));
                        };

                        group.add(block.style.margin_after);
                        group.has_empty = true;
                        group.path = block.source.indices().to_vec();

                        pending_margin = Some(group);
                        continue;
                    }

                    self.boxes[index].rect.height = height;
                    cursor = end;

                    let mut group = PendingMargin::new(
                        block.style.margin_after,
                        block.source.indices().to_vec(),
                    );

                    if let Some(child_margin) = child_flow.trailing_margin {
                        group.add(child_margin.positive);
                        group.add(child_margin.negative);
                    }

                    pending_margin = Some(group);
                }

                FlowChild::InlineRun { items, .. } => {
                    let mut text = String::new();

                    collect_text(items, inherited, &mut text)?;

                    let has_content = text
                        .chars()
                        .any(|character| !matches!(character, ' ' | '\t' | '\n' | '\r' | '\x0c'));

                    if !has_content {
                        continue;
                    }

                    at_start = false;

                    if let Some(group) = pending_margin.take() {
                        if group.has_empty {
                            return Err(PreviewError::EmptyBlock(group.path));
                        }

                        if group.value() < 0 {
                            return Err(PreviewError::Margin(group.path));
                        }

                        cursor = cursor
                            .checked_add(group.value())
                            .ok_or(PreviewError::SizeOverflow)?;
                    }

                    let mut fragments = Vec::new();

                    collect_styled_fragments(items, inherited, None, &mut fragments)?;

                    let fonts = PreviewFonts::new(page, &fragments);
                    let words = split_styled_words(&fragments);

                    let changed_weight = fragments
                        .iter()
                        .find(|fragment| fragment.style.font_weight != inherited.font_weight);

                    if let Some(fragment) = changed_weight {
                        let has_vector_font = fragments
                            .iter()
                            .any(|item| fonts.get(item.style).or(fallback).is_some());

                        let has_bitmap_font = fragments
                            .iter()
                            .any(|item| fonts.get(item.style).or(fallback).is_none());

                        let unsupported_word = words.iter().any(|word| {
                            word.parts.first().is_some_and(|first| {
                                word.parts
                                    .iter()
                                    .any(|part| part.style.font_weight != first.style.font_weight)
                                    && (word
                                        .parts
                                        .iter()
                                        .any(|part| !fonts.stack(part.style).is_empty())
                                        || fonts.measure_vector_word(word, fallback).is_none())
                            })
                        });

                        if has_vector_font && (unsupported_word || has_bitmap_font) {
                            let path = fragment
                                .style_source
                                .map_or_else(Vec::new, |path| path.indices().to_vec());

                            return Err(PreviewError::InlineFont(path));
                        }
                    }

                    let font = fonts.get(inherited).or(fallback);
                    let base_stack = fonts.stack(inherited);

                    let base_metrics =
                        PreviewLineMetrics::from_stack(font, &base_stack, inherited.font_size);

                    let mut wrap = TextWrap::new(0, width);
                    let mut line_metrics = base_metrics;
                    let mut line_top = 0_i32;

                    let mut runs = Vec::new();
                    let mut pending_runs = Vec::new();
                    let use_positioned_runs = fragments.len() > 1
                        || fragments.iter().any(|fragment| {
                            fragment.style.color != inherited.color
                                || !same_font(inherited, fragment.style)
                        });

                    for word in &words {
                        let measured = fonts.measure_word(word, fallback);
                        let space_width = fonts.space_width(word, fallback);
                        let placement = wrap.place_word(space_width, measured.width());

                        if placement.wrapped {
                            finish_preview_line(
                                &mut pending_runs,
                                &mut runs,
                                line_metrics,
                                line_top,
                            );

                            line_top = line_top.saturating_add(line_metrics.height());
                            line_metrics = base_metrics;
                        }

                        for run in measured.runs {
                            let run_font = fonts.get(run.style).or(fallback);
                            let run_stack = fonts.stack(run.style);

                            let run_metrics = PreviewLineMetrics::from_stack(
                                run_font,
                                &run_stack,
                                run.style.font_size,
                            );

                            line_metrics.include(run_metrics);

                            if use_positioned_runs {
                                pending_runs.push(PendingPreviewRun {
                                    x: placement.x.saturating_add(run.x),
                                    offset_x: run.offset_x,
                                    above_subpixels: run_metrics.above_subpixels,
                                    text: run.text.into_owned(),
                                    style: run.style.clone(),
                                });
                            }
                        }
                    }

                    finish_preview_line(&mut pending_runs, &mut runs, line_metrics, line_top);

                    let height = line_top.saturating_add(line_metrics.height());

                    self.display_list.push(PreviewCommand::Text {
                        rect: LayoutRect {
                            x: left,
                            y: cursor,
                            width,
                            height,
                        },
                        text,
                        style: inherited.clone(),
                        runs,
                    });

                    cursor = cursor
                        .checked_add(height)
                        .ok_or(PreviewError::SizeOverflow)?;
                }
            }
        }

        if let Some(group) = &pending_margin {
            if group.has_empty {
                return Err(PreviewError::Margin(group.path.clone()));
            }
        }

        Ok(FlowResult {
            end: cursor,
            trailing_margin: pending_margin,
        })
    }

    pub(crate) fn format_debug(&self) -> String {
        let mut output = format!(
            "FreeDOM preview geometry: {} blocks (not painted)\n",
            self.boxes.len()
        );

        for item in self.boxes.iter().take(32) {
            let rect = item.rect;

            let _ = writeln!(
                output,
                "  path={:?} x={} y={} width={} height={}",
                item.source.indices(),
                rect.x,
                rect.y,
                rect.width,
                rect.height
            );
        }

        if self.boxes.len() > 32 {
            output.push_str("  ... output limit reached\n");
        }

        output
    }
}

// CSS 2.2 section 8.3.1:
// https://www.w3.org/TR/CSS22/box.html#collapsing-margins
fn collapse_sibling_margins(first: i32, second: i32) -> i32 {
    first.max(second).max(0) + first.min(second).min(0)
}

// CSS 2.2 section 8.3.1: first-child top margins can collapse
// with the parent.
// https://www.w3.org/TR/CSS22/box.html#collapsing-margins
fn leading_margin(block: &BlockBox<'_>) -> Result<PendingMargin, PreviewError> {
    let mut group = PendingMargin::new(block.style.margin_before, block.source.indices().to_vec());
    let mut current = block;

    loop {
        if current.style.border_top.width > 0 {
            break;
        }

        let BlockContent::Flow(children) = &current.content else {
            break;
        };

        let mut first_block = None;

        for child in children {
            match child {
                FlowChild::Block(child) => {
                    first_block = Some(child);
                    break;
                }

                FlowChild::InlineRun { items, .. } => {
                    let mut text = String::new();

                    collect_text(items, current.style, &mut text)?;

                    let has_content = text
                        .chars()
                        .any(|character| !matches!(character, ' ' | '\t' | '\n' | '\r' | '\x0c'));

                    if has_content {
                        break;
                    }
                }
            }
        }

        let Some(child) = first_block else {
            break;
        };

        if matches!(&child.content, BlockContent::Flow(children) if children.is_empty())
            && !matches!(child.style.height, CssSize::Pixels(height) if height > 0)
            && child.style.border_top.width == 0
            && child.style.border_bottom.width == 0
        {
            return Err(PreviewError::EmptyBlock(child.source.indices().to_vec()));
        }

        group.add(child.style.margin_before);
        current = child;
    }

    Ok(group)
}

fn same_font(left: &TextStyle, right: &TextStyle) -> bool {
    same_font_except_weight(left, right) && left.font_weight == right.font_weight
}

fn same_font_except_weight(left: &TextStyle, right: &TextStyle) -> bool {
    left.font_size.to_bits() == right.font_size.to_bits()
        && left.font_style == right.font_style
        && left.font_families == right.font_families
}

struct InlineTextFragment<'a> {
    text: &'a str,
    style: &'a TextStyle,
    style_source: Option<&'a SourcePath>,
}

struct InlineWordPart<'a> {
    text: &'a str,
    style: &'a TextStyle,
}

struct InlineWord<'a> {
    space_before: Option<&'a TextStyle>,
    parts: Vec<InlineWordPart<'a>>,
}

// Preview-only quantized approximation; this is not CSS baseline alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreviewLineMetrics {
    above_subpixels: i64,
    below_subpixels: i64,
}

impl PreviewLineMetrics {
    #[cfg(test)]
    fn from_font(font: Option<&TrueTypeFont<'_>>, font_size: f32) -> Self {
        Self::from_stack(font, &[], font_size)
    }

    fn from_stack(
        font: Option<&TrueTypeFont<'_>>,
        stack: &[&TrueTypeFont<'_>],
        font_size: f32,
    ) -> Self {
        let metrics = stack_text_metrics(font, stack, font_size);
        let height = metrics.line_height.max(0);

        let steps = i64::from(GLYPH_SUBPIXEL_STEPS);
        let height_subpixels = i64::from(height) * steps;

        let above = stack_ascent(stack, font_size)
            .or_else(|| {
                font.and_then(|font| font.line_metrics(font_size))
                    .map(|metrics| metrics.ascent)
            })
            .and_then(split_subpixel_position)
            .map(|(base, phase)| i64::from(base) * steps + i64::from(phase))
            .unwrap_or(i64::from(metrics.glyph_height) * steps)
            .clamp(0, height_subpixels);

        Self {
            above_subpixels: above,
            below_subpixels: height_subpixels - above,
        }
    }

    fn include(&mut self, other: Self) {
        self.above_subpixels = self.above_subpixels.max(other.above_subpixels);
        self.below_subpixels = self.below_subpixels.max(other.below_subpixels);
    }

    fn height(self) -> i32 {
        let steps = i64::from(GLYPH_SUBPIXEL_STEPS);
        let height = (self.above_subpixels + self.below_subpixels + steps - 1) / steps;

        i32::try_from(height).unwrap_or(i32::MAX)
    }
}

struct PendingPreviewRun {
    x: i32,
    offset_x: f32,
    above_subpixels: i64,
    text: String,
    style: TextStyle,
}

fn finish_preview_line(
    pending: &mut Vec<PendingPreviewRun>,
    output: &mut Vec<PreviewTextRun>,
    metrics: PreviewLineMetrics,
    line_top: i32,
) {
    let steps = i64::from(GLYPH_SUBPIXEL_STEPS);

    for run in pending.drain(..) {
        let offset = metrics
            .above_subpixels
            .saturating_sub(run.above_subpixels)
            .max(0);

        let whole = i32::try_from(offset / steps).unwrap_or(i32::MAX);
        let fraction = (offset % steps) as f32 / GLYPH_SUBPIXEL_STEPS as f32;

        output.push(PreviewTextRun {
            x: run.x,
            offset_x: run.offset_x,
            y: line_top.saturating_add(whole),
            offset_y: fraction,
            text: run.text,
            style: run.style,
        });
    }
}

pub(crate) struct PreviewFonts<'style, 'font> {
    entries: Vec<(&'style TextStyle, Vec<ResolvedFont<'font>>)>,
}

impl<'style, 'font> PreviewFonts<'style, 'font> {
    fn new(page: &'font Page, fragments: &[InlineTextFragment<'style>]) -> Self {
        Self::from_styles(page, fragments.iter().map(|fragment| fragment.style))
    }

    pub(crate) fn from_styles(
        page: &'font Page,
        styles: impl IntoIterator<Item = &'style TextStyle>,
    ) -> Self {
        let mut entries: Vec<(&TextStyle, Vec<ResolvedFont<'font>>)> = Vec::new();

        for style in styles {
            if entries
                .iter()
                .any(|(existing, _)| same_font(existing, style))
            {
                continue;
            }

            let font = page.vector_fonts_for_style(style);

            entries.push((style, font));
        }

        Self { entries }
    }

    pub(crate) fn get(&self, style: &TextStyle) -> Option<&TrueTypeFont<'font>> {
        self.entries
            .iter()
            .find(|(existing, _)| same_font(existing, style))
            .and_then(|(_, fonts)| fonts.first().map(|entry| &entry.font))
    }

    pub(crate) fn stack(&self, style: &TextStyle) -> Vec<&TrueTypeFont<'font>> {
        self.entries
            .iter()
            .find(|(existing, _)| same_font(existing, style))
            .filter(|(_, fonts)| fonts.len() > 1)
            .map(|(_, fonts)| fonts.iter().map(|entry| &entry.font).collect())
            .unwrap_or_default()
    }

    pub(crate) fn syntheses(
        &self,
        style: &TextStyle,
    ) -> Vec<(crate::css::FontWeight, crate::css::FontStyle)> {
        self.entries
            .iter()
            .find(|(existing, _)| same_font(existing, style))
            .map(|(_, fonts)| fonts.iter().map(|entry| entry.synthesis).collect())
            .unwrap_or_default()
    }

    fn measure_word<'a>(
        &self,
        word: &InlineWord<'a>,
        fallback: Option<&TrueTypeFont<'_>>,
    ) -> MeasuredWord<'a> {
        if word.parts.first().is_some_and(|first| {
            word.parts
                .iter()
                .any(|part| part.style.font_weight != first.style.font_weight)
        }) {
            if let Some(measured) = self.measure_vector_word(word, fallback) {
                return measured;
            }
        }

        measure_styled_word(word, |text, style| {
            let font = self.get(style).or(fallback);
            let stack = self.stack(style);
            let synthesis = self.syntheses(style);

            let width = stack_word_width(
                font,
                &stack,
                text,
                style.font_size,
                self.synthetic_weight(style),
                self.synthetic_style(style),
                &synthesis,
            );

            let advance = if font.is_none() {
                bitmap_text_advance(text, style.font_size)
            } else {
                width
            };

            (width, advance)
        })
    }

    fn measure_vector_word<'a>(
        &self,
        word: &InlineWord<'a>,
        fallback: Option<&TrueTypeFont<'_>>,
    ) -> Option<MeasuredWord<'a>> {
        if word
            .parts
            .iter()
            .any(|part| self.get(part.style).or(fallback).is_none())
        {
            return None;
        }

        let mut measured = measure_styled_word(word, |text, style| {
            let font = self.get(style).or(fallback).expect("font checked above");

            let width = text_word_width(
                Some(font),
                text,
                style.font_size,
                self.synthetic_weight(style),
                self.synthetic_style(style),
            );

            (width, 0)
        });

        let mut pen = 0.0_f32;
        let mut previous: Option<(usize, u16)> = None;

        for run in &mut measured.runs {
            let font = self.get(run.style).or(fallback)?;
            let identity = font.glyph_cache_key();
            let first_glyph = font.glyph_id(run.text.chars().next()?);

            if let Some((previous_font, left)) = previous {
                if previous_font == identity {
                    let scale = run.style.font_size / f32::from(font.head().units_per_em);

                    pen += f32::from(font.kerning(left, first_glyph)) * scale;
                }
            }

            let (x, phase) = split_subpixel_position(pen)?;

            run.x = x;
            run.offset_x = f32::from(phase) / GLYPH_SUBPIXEL_STEPS as f32;

            pen += font
                .text_advance_width(&run.text, run.style.font_size)
                .ok()?;

            previous = run
                .text
                .chars()
                .last()
                .map(|character| (identity, font.glyph_id(character)));
        }

        Some(measured)
    }

    fn space_width(&self, word: &InlineWord<'_>, fallback: Option<&TrueTypeFont<'_>>) -> i32 {
        word.space_before.map_or(0, |style| {
            let font = self.get(style).or(fallback);
            text_metrics(font, style.font_size).character_width
        })
    }

    pub(crate) fn synthetic_weight(&self, style: &TextStyle) -> crate::css::FontWeight {
        self.entries
            .iter()
            .find(|(existing, _)| same_font(existing, style))
            .and_then(|(_, fonts)| fonts.first())
            .map_or(style.font_weight, |entry| entry.synthesis.0)
    }

    pub(crate) fn synthetic_style(&self, style: &TextStyle) -> crate::css::FontStyle {
        self.entries
            .iter()
            .find(|(existing, _)| same_font(existing, style))
            .and_then(|(_, fonts)| fonts.first())
            .map_or(style.font_style, |entry| entry.synthesis.1)
    }
}

struct MeasuredRun<'a> {
    x: i32,
    offset_x: f32,
    width: i32,
    text: std::borrow::Cow<'a, str>,
    style: &'a TextStyle,
}

struct MeasuredWord<'a> {
    runs: Vec<MeasuredRun<'a>>,
}

impl MeasuredWord<'_> {
    fn width(&self) -> i32 {
        self.runs
            .iter()
            .map(|run| {
                run.x
                    .saturating_add(run.width)
                    .saturating_add(i32::from(run.offset_x > 0.0))
            })
            .max()
            .unwrap_or(0)
    }
}

// Adjacent fragments with the same font and color are measured together so
// that element boundaries do not change kerning within a word.
fn measure_styled_word<'a>(
    word: &InlineWord<'a>,
    mut measure: impl FnMut(&str, &TextStyle) -> (i32, i32),
) -> MeasuredWord<'a> {
    let mut runs = Vec::new();
    let mut index = 0;
    let mut x = 0_i32;

    while index < word.parts.len() {
        let start = index;
        let style = word.parts[index].style;

        while index < word.parts.len()
            && same_font(style, word.parts[index].style)
            && style.color == word.parts[index].style.color
        {
            index += 1;
        }

        let text = if index == start + 1 {
            std::borrow::Cow::Borrowed(word.parts[start].text)
        } else {
            std::borrow::Cow::Owned(
                word.parts[start..index]
                    .iter()
                    .map(|part| part.text)
                    .collect::<String>(),
            )
        };

        let (width, advance) = measure(text.as_ref(), style);

        runs.push(MeasuredRun {
            x,
            offset_x: 0.0,
            width,
            text,
            style,
        });

        x = x.saturating_add(advance);
    }

    MeasuredWord { runs }
}

// A partial white-space: normal model for the experimental preview.
// https://www.w3.org/TR/css-text-3/#white-space-processing
fn split_styled_words<'a>(fragments: &[InlineTextFragment<'a>]) -> Vec<InlineWord<'a>> {
    let mut words = Vec::new();
    let mut in_word = false;
    let mut pending_space = None;

    for fragment in fragments {
        let mut start = 0;

        for (index, character) in fragment.text.char_indices() {
            if matches!(character, ' ' | '\t' | '\n' | '\r' | '\x0c') {
                if start < index {
                    push_word_part(
                        &mut words,
                        &mut in_word,
                        &mut pending_space,
                        &fragment.text[start..index],
                        fragment.style,
                    );
                }

                if in_word {
                    in_word = false;
                    pending_space = Some(fragment.style);
                }

                start = index + character.len_utf8();
            }
        }

        if start < fragment.text.len() {
            push_word_part(
                &mut words,
                &mut in_word,
                &mut pending_space,
                &fragment.text[start..],
                fragment.style,
            );
        }
    }

    words
}

fn push_word_part<'a>(
    words: &mut Vec<InlineWord<'a>>,
    in_word: &mut bool,
    pending_space: &mut Option<&'a TextStyle>,
    text: &'a str,
    style: &'a TextStyle,
) {
    let part = InlineWordPart { text, style };

    if *in_word {
        words
            .last_mut()
            .expect("current word exists")
            .parts
            .push(part);
    } else {
        words.push(InlineWord {
            space_before: pending_space.take(),
            parts: vec![part],
        });

        *in_word = true;
    }
}

fn collect_styled_fragments<'a>(
    items: &'a [InlineItem<'_>],
    inherited: &'a TextStyle,
    style_source: Option<&'a SourcePath>,
    output: &mut Vec<InlineTextFragment<'a>>,
) -> Result<(), PreviewError> {
    for item in items {
        match item {
            InlineItem::Text { text, .. } => {
                let unsupported_whitespace = text.chars().any(|character| {
                    character.is_whitespace()
                        && !matches!(character, ' ' | '\t' | '\n' | '\r' | '\x0c')
                });

                if unsupported_whitespace {
                    return Err(PreviewError::WhiteSpace);
                }

                if !text.is_empty() {
                    output.push(InlineTextFragment {
                        text,
                        style: inherited,
                        style_source,
                    });
                }
            }

            InlineItem::Element {
                source,
                style,
                children,
                ..
            } => {
                if style.background_color != BackgroundColor::Transparent
                    || style.border_top.width != 0
                    || style.border_bottom.width != 0
                    || style.border_left.width != 0
                    || style.border_right.width != 0
                    || style.margin_left != 0
                    || style.margin_right != 0
                {
                    return Err(PreviewError::InlineStyle(source.indices().to_vec()));
                }

                collect_styled_fragments(children, style, Some(source), output)?;
            }

            InlineItem::Replaced { source, .. } => {
                return Err(PreviewError::Replaced(source.indices().to_vec()));
            }

            InlineItem::Break { source } => {
                return Err(PreviewError::LineBreak(source.indices().to_vec()));
            }
        }
    }

    Ok(())
}

fn collect_text(
    items: &[InlineItem<'_>],
    inherited: &TextStyle,
    output: &mut String,
) -> Result<(), PreviewError> {
    let mut fragments = Vec::new();

    collect_styled_fragments(items, inherited, None, &mut fragments)?;

    let mut word_style: Option<&TextStyle> = None;

    for fragment in &fragments {
        let source_path = || {
            fragment
                .style_source
                .map_or_else(Vec::new, |path| path.indices().to_vec())
        };

        if !same_font_except_weight(inherited, fragment.style) {
            return Err(PreviewError::InlineFont(source_path()));
        }

        for character in fragment.text.chars() {
            if matches!(character, ' ' | '\t' | '\n' | '\r' | '\x0c') {
                word_style = None;
            } else if let Some(style) = word_style {
                if style.color != fragment.style.color {
                    return Err(PreviewError::InlineStyle(source_path()));
                }
            } else {
                word_style = Some(fragment.style);
            }
        }
    }

    for word in split_styled_words(&fragments) {
        if word.space_before.is_some() {
            output.push(' ');
        }

        for part in word.parts {
            debug_assert!(same_font_except_weight(inherited, part.style));
            output.push_str(part.text);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{collapse_sibling_margins, collect_styled_fragments, collect_text};

    use crate::{
        box_tree::InlineItem,
        css::{FontWeight, TextStyle},
        styled_tree::SourcePath,
    };

    #[test]
    fn collapses_signed_margins_without_overflow() {
        assert_eq!(collapse_sibling_margins(8, 12), 12);
        assert_eq!(collapse_sibling_margins(8, -3), 5);
        assert_eq!(collapse_sibling_margins(-5, -3), -5);
        assert_eq!(collapse_sibling_margins(i32::MAX, i32::MIN), -1);
    }

    #[test]
    fn collects_styled_fragments_without_inserting_or_trimming_whitespace() {
        let normal = TextStyle::initial();

        let mut bold = normal.clone();
        bold.font_weight = FontWeight::Bold;

        let path = SourcePath::from_indices(&[1, 2]);

        let items = vec![
            InlineItem::Text {
                source: None,
                text: "  hel",
            },
            InlineItem::Element {
                source: &path,
                tag: "strong",
                style: &bold,
                children: vec![InlineItem::Text {
                    source: None,
                    text: "lo ",
                }],
            },
            InlineItem::Text {
                source: None,
                text: " world\n",
            },
        ];

        let mut fragments = Vec::new();

        collect_styled_fragments(&items, &normal, None, &mut fragments).unwrap();

        assert_eq!(
            fragments
                .iter()
                .map(|fragment| fragment.text)
                .collect::<Vec<_>>(),
            vec!["  hel", "lo ", " world\n"]
        );

        assert_eq!(
            fragments
                .iter()
                .map(|fragment| fragment.style.font_weight)
                .collect::<Vec<_>>(),
            vec![FontWeight::Normal, FontWeight::Bold, FontWeight::Normal,]
        );

        assert_eq!(fragments[1].style_source.unwrap().indices(), &[1, 2]);

        let mut text = String::new();

        collect_text(&items, &normal, &mut text).unwrap();
        assert_eq!(text, "hello world");
    }

    #[test]
    fn nested_inline_styles_restore_parent_and_keep_whitespace() {
        let normal = TextStyle::initial();

        let mut bold = normal.clone();
        bold.font_weight = FontWeight::Bold;

        let mut colored = bold.clone();
        colored.color = 0x00_FF_00_00;

        let strong = SourcePath::from_indices(&[1]);
        let span = SourcePath::from_indices(&[1, 0]);

        let items = vec![InlineItem::Element {
            source: &strong,
            tag: "strong",
            style: &bold,
            children: vec![
                InlineItem::Text {
                    source: None,
                    text: "A ",
                },
                InlineItem::Element {
                    source: &span,
                    tag: "span",
                    style: &colored,
                    children: vec![InlineItem::Text {
                        source: None,
                        text: "B",
                    }],
                },
                InlineItem::Text {
                    source: None,
                    text: " C",
                },
            ],
        }];

        let mut fragments = Vec::new();

        collect_styled_fragments(&items, &normal, None, &mut fragments).unwrap();

        assert_eq!(
            fragments
                .iter()
                .map(|fragment| fragment.text)
                .collect::<String>(),
            "A B C"
        );

        assert_eq!(
            fragments
                .iter()
                .map(|fragment| fragment.style.color)
                .collect::<Vec<_>>(),
            vec![normal.color, colored.color, normal.color]
        );

        assert_eq!(fragments[2].style_source.unwrap().indices(), &[1]);
    }

    #[test]
    fn splits_words_across_styled_fragments() {
        use super::{InlineTextFragment, split_styled_words};

        let normal = TextStyle::initial();

        let mut bold = normal.clone();
        bold.font_weight = FontWeight::Bold;

        let fragments = vec![
            InlineTextFragment {
                text: "  hel",
                style: &normal,
                style_source: None,
            },
            InlineTextFragment {
                text: "lo \t",
                style: &bold,
                style_source: None,
            },
            InlineTextFragment {
                text: "\nмир  ",
                style: &normal,
                style_source: None,
            },
        ];

        let words = split_styled_words(&fragments);

        assert_eq!(words.len(), 2);
        assert!(words[0].space_before.is_none());

        assert_eq!(
            words[0]
                .parts
                .iter()
                .map(|part| part.text)
                .collect::<Vec<_>>(),
            vec!["hel", "lo"]
        );

        assert_eq!(words[0].parts[0].style.font_weight, FontWeight::Normal);
        assert_eq!(words[0].parts[1].style.font_weight, FontWeight::Bold);

        assert_eq!(words[1].parts[0].text, "мир");
        assert_eq!(words[1].space_before.unwrap().font_weight, FontWeight::Bold);
    }

    #[test]
    fn collapses_whitespace_across_fragments() {
        use super::{InlineTextFragment, split_styled_words};

        let normal = TextStyle::initial();

        let fragments = vec![
            InlineTextFragment {
                text: " \t",
                style: &normal,
                style_source: None,
            },
            InlineTextFragment {
                text: "one",
                style: &normal,
                style_source: None,
            },
            InlineTextFragment {
                text: "\n",
                style: &normal,
                style_source: None,
            },
            InlineTextFragment {
                text: "\t two \r",
                style: &normal,
                style_source: None,
            },
        ];

        let words = split_styled_words(&fragments);

        assert_eq!(words.len(), 2);
        assert_eq!(words[0].parts[0].text, "one");
        assert!(words[0].space_before.is_none());
        assert_eq!(words[1].parts[0].text, "two");
        assert!(words[1].space_before.is_some());

        let items = vec![InlineItem::Text {
            source: None,
            text: "  one\n\t two \r",
        }];

        let mut text = String::new();
        collect_text(&items, &normal, &mut text).unwrap();

        assert_eq!(text, "one two");

        let whitespace_only = [InlineTextFragment {
            text: " \t\n",
            style: &normal,
            style_source: None,
        }];

        assert!(split_styled_words(&whitespace_only).is_empty());
    }

    #[test]
    fn measures_styled_runs_with_offsets_and_combines_equal_styles() {
        use super::{InlineWord, InlineWordPart, measure_styled_word};

        let normal = TextStyle::initial();

        let mut bold = normal.clone();
        bold.font_weight = FontWeight::Bold;

        let word = InlineWord {
            space_before: None,
            parts: vec![
                InlineWordPart {
                    text: "h",
                    style: &normal,
                },
                InlineWordPart {
                    text: "el",
                    style: &normal,
                },
                InlineWordPart {
                    text: "lo",
                    style: &bold,
                },
            ],
        };

        let mut calls = Vec::new();

        let measured = measure_styled_word(&word, |text, style| {
            calls.push((text.to_owned(), style.font_weight));

            if style.font_weight == FontWeight::Bold {
                (20, 20)
            } else {
                (21, 21)
            }
        });

        assert_eq!(
            calls,
            vec![
                ("hel".into(), FontWeight::Normal),
                ("lo".into(), FontWeight::Bold),
            ]
        );

        assert_eq!(measured.runs.len(), 2);
        assert_eq!((measured.runs[0].x, measured.runs[0].width), (0, 21));
        assert_eq!((measured.runs[1].x, measured.runs[1].width), (21, 20));
        assert_eq!(measured.runs[0].text, "hel");
        assert_eq!(measured.runs[1].text, "lo");
        assert_eq!(measured.runs[0].style.font_weight, FontWeight::Normal);
        assert_eq!(measured.runs[1].style.font_weight, FontWeight::Bold);
        assert_eq!(measured.width(), 41);
    }

    #[test]
    fn measures_empty_and_very_wide_words_safely() {
        use super::{InlineWord, InlineWordPart, measure_styled_word};

        let normal = TextStyle::initial();

        let mut bold = normal.clone();
        bold.font_weight = FontWeight::Bold;

        let empty = InlineWord {
            space_before: None,
            parts: Vec::new(),
        };

        assert_eq!(
            measure_styled_word(&empty, |_, _| unreachable!()).width(),
            0
        );

        let word = InlineWord {
            space_before: None,
            parts: vec![
                InlineWordPart {
                    text: "a",
                    style: &normal,
                },
                InlineWordPart {
                    text: "b",
                    style: &bold,
                },
            ],
        };

        let measured = measure_styled_word(&word, |_, _| (i32::MAX, i32::MAX));

        assert_eq!(measured.runs[1].x, i32::MAX);
        assert_eq!(measured.width(), i32::MAX);
    }

    #[test]
    fn measures_words_and_spaces_using_their_styles() {
        use super::{InlineTextFragment, PreviewFonts, split_styled_words};

        let page = crate::page::parse_xhtml_for_test(
            "https://example.org/",
            "<html xmlns='http://www.w3.org/1999/xhtml'><head/><body/></html>",
        )
        .unwrap();

        let normal = TextStyle::initial();

        let mut same_font_other_color = normal.clone();
        same_font_other_color.color = 0x00_FF_00_00;

        let mut bold = normal.clone();
        bold.font_size = 32.0;
        bold.font_weight = FontWeight::Bold;

        let fragments = vec![
            InlineTextFragment {
                text: "a",
                style: &normal,
                style_source: None,
            },
            InlineTextFragment {
                text: "b ",
                style: &bold,
                style_source: None,
            },
            InlineTextFragment {
                text: "c",
                style: &same_font_other_color,
                style_source: None,
            },
        ];

        let fonts = PreviewFonts::new(&page, &fragments);

        // Color does not require a separate font.
        assert_eq!(fonts.entries.len(), 2);

        let words = split_styled_words(&fragments);
        assert_eq!(words.len(), 2);

        // Without loaded fonts, measurement uses the bitmap fallback.
        let first = fonts.measure_word(&words[0], None);

        assert_eq!(first.runs[0].width, 8);
        assert_eq!(first.runs[1].x, 8);
        assert_eq!(first.runs[1].width, 17);
        assert_eq!(first.width(), 25);

        assert_eq!(fonts.space_width(&words[0], None), 0);
        assert_eq!(fonts.space_width(&words[1], None), 16);
        assert_eq!(fonts.measure_word(&words[1], None).width(), 8);
    }

    #[test]
    fn preview_line_metrics_merge_above_and_below_baseline() {
        use super::PreviewLineMetrics;

        let mut line = PreviewLineMetrics::from_font(None, 16.0);

        assert_eq!(
            line,
            PreviewLineMetrics {
                above_subpixels: 64,
                below_subpixels: 16
            }
        );

        line.include(PreviewLineMetrics::from_font(None, 32.0));

        assert_eq!(
            line,
            PreviewLineMetrics {
                above_subpixels: 128,
                below_subpixels: 32,
            }
        );

        line.include(PreviewLineMetrics {
            above_subpixels: 40,
            below_subpixels: 96,
        });

        assert_eq!(line.height(), 28);
        assert_eq!(PreviewLineMetrics::from_font(None, 0.0).height(), 0);
    }

    #[test]
    fn preview_line_height_preserves_the_existing_vector_font_height() {
        use super::PreviewLineMetrics;
        use crate::graphics::font::{TrueTypeFont, normalize_font_container};
        use crate::page_layout::text_metrics;

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/fonts/noto/nko-hmtx.woff2",
        );

        let data =
            normalize_font_container(std::fs::read(path).expect("Nko font fixture must exist"))
                .unwrap();

        let font = TrueTypeFont::parse(&data).unwrap();

        for size in [0.0, 12.0, 16.0, 32.0, 64.0] {
            let preview = PreviewLineMetrics::from_font(Some(&font), size);
            let existing = text_metrics(Some(&font), size);

            assert_eq!(preview.height(), existing.line_height, "size={size}");
        }
    }

    #[test]
    fn aligns_preview_runs_after_all_line_metrics_are_known() {
        use super::{PendingPreviewRun, PreviewLineMetrics, finish_preview_line};

        let small = TextStyle::initial();

        let mut large = small.clone();
        large.font_size = 32.0;

        let small_metrics = PreviewLineMetrics::from_font(None, small.font_size);
        let large_metrics = PreviewLineMetrics::from_font(None, large.font_size);

        let mut line_metrics = small_metrics;
        line_metrics.include(large_metrics);

        let mut pending = vec![
            PendingPreviewRun {
                x: 4,
                offset_x: 0.0,
                above_subpixels: small_metrics.above_subpixels,
                text: "small".into(),
                style: small,
            },
            PendingPreviewRun {
                x: 40,
                offset_x: 0.0,
                above_subpixels: large_metrics.above_subpixels,
                text: "large".into(),
                style: large,
            },
        ];

        let mut runs = Vec::new();

        finish_preview_line(&mut pending, &mut runs, line_metrics, 20);

        assert!(pending.is_empty());
        assert_eq!(runs.len(), 2);

        assert_eq!(
            (runs[0].x, runs[0].y, runs[0].text.as_str()),
            (4, 28, "small")
        );

        assert_eq!(
            (runs[1].x, runs[1].y, runs[1].text.as_str()),
            (40, 20, "large")
        );

        assert_eq!(
            i64::from(runs[0].y) * 8 + small_metrics.above_subpixels,
            i64::from(runs[1].y) * 8 + line_metrics.above_subpixels
        );

        assert_eq!(runs[0].style.font_size, 16.0);
        assert_eq!(runs[1].style.font_size, 32.0);
    }

    #[test]
    fn preview_vector_baseline_matches_rasterizer_integer_coordinate() {
        use super::PreviewLineMetrics;
        use crate::graphics::font::{
            TrueTypeFont, normalize_font_container, render::split_subpixel_position,
        };

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/fonts/noto/nko-hmtx.woff2",
        );

        let data =
            normalize_font_container(std::fs::read(path).expect("Nko font fixture must exist"))
                .unwrap();
        let font = TrueTypeFont::parse(&data).unwrap();

        let mut saw_fractional_phase = false;
        let mut saw_round_mismatch = false;

        for size in 1..=128 {
            let size = size as f32;
            let metrics = font.line_metrics(size).unwrap();

            let (base, phase) = split_subpixel_position(metrics.ascent).unwrap();

            let preview = PreviewLineMetrics::from_font(Some(&font), size);

            assert_eq!(
                preview.above_subpixels,
                i64::from(base) * 8 + i64::from(phase),
                "size={size}",
            );

            saw_fractional_phase |= phase != 0;
            saw_round_mismatch |= metrics.ascent.round() as i32 != base;
        }

        assert!(saw_fractional_phase);
        assert!(saw_round_mismatch);
    }

    #[test]
    fn aligns_preview_runs_with_fractional_baselines() {
        use super::{PendingPreviewRun, PreviewLineMetrics, finish_preview_line};

        let style = TextStyle::initial();

        let mut line = PreviewLineMetrics {
            above_subpixels: 85,
            below_subpixels: 11,
        };

        line.include(PreviewLineMetrics {
            above_subpixels: 98,
            below_subpixels: 6,
        });

        let mut pending = vec![
            PendingPreviewRun {
                x: 0,
                offset_x: 0.0,
                above_subpixels: 85,
                text: "a".into(),
                style: style.clone(),
            },
            PendingPreviewRun {
                x: 20,
                offset_x: 0.0,
                above_subpixels: 98,
                text: "b".into(),
                style,
            },
        ];

        let mut runs = Vec::new();

        finish_preview_line(&mut pending, &mut runs, line, 10);

        assert_eq!(runs.len(), 2);
        assert_eq!((runs[0].y, runs[0].offset_y), (11, 0.625));
        assert_eq!((runs[1].y, runs[1].offset_y), (10, 0.0));

        let first_baseline = i64::from(runs[0].y) * 8 + (runs[0].offset_y * 8.0) as i64 + 85;
        let second_baseline = i64::from(runs[1].y) * 8 + (runs[1].offset_y * 8.0) as i64 + 98;

        assert_eq!(first_baseline, second_baseline);
        assert_eq!(line.height(), 14);
    }

    #[test]
    fn preview_fractional_baseline_reaches_vector_pixels() {
        use super::{
            PendingPreviewRun, PreviewCommand, PreviewLayout, PreviewLineMetrics,
            finish_preview_line,
        };
        use crate::graphics::font::{TrueTypeFont, normalize_font_container};
        use crate::page_layout::LayoutRect;
        use crate::render::{GlyphRasterCache, paint_preview_scene};

        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/fonts/noto/nko-hmtx.woff2",
        );

        let data =
            normalize_font_container(std::fs::read(path).expect("Nko font fixture must exist"))
                .unwrap();

        let font = TrueTypeFont::parse(&data).unwrap();
        let small_metrics = PreviewLineMetrics::from_font(Some(&font), 24.0);

        // Find a second size that requires a half-pixel adjustment.
        let (large_size, large_metrics) = (25..=64)
            .find_map(|size| {
                let metrics = PreviewLineMetrics::from_font(Some(&font), size as f32);
                let difference = metrics.above_subpixels - small_metrics.above_subpixels;

                (difference > 0 && difference % 8 == 4).then_some((size as f32, metrics))
            })
            .expect("fixture must produce a half-pixel baseline difference");

        let mut line = small_metrics;
        line.include(large_metrics);

        let mut small = TextStyle::initial();
        small.font_size = 24.0;
        small.color = 0x00_FF_00_00;

        let mut large = small.clone();
        large.font_size = large_size;
        large.color = 0x00_00_00_FF;

        let mut pending = vec![
            PendingPreviewRun {
                x: 0,
                offset_x: 0.0,
                above_subpixels: small_metrics.above_subpixels,
                text: "ߊ".into(),
                style: small.clone(),
            },
            PendingPreviewRun {
                x: 100,
                offset_x: 0.0,
                above_subpixels: large_metrics.above_subpixels,
                text: "ߊ".into(),
                style: large,
            },
        ];

        let mut runs = Vec::new();
        finish_preview_line(&mut pending, &mut runs, line, 0);

        assert_eq!(runs[0].offset_y, 0.5);
        assert_eq!(runs[1].offset_y, 0.0);

        let baseline0 = i64::from(runs[0].y) * 8
            + (runs[0].offset_y * 8.0) as i64
            + small_metrics.above_subpixels;

        let baseline1 = i64::from(runs[1].y) * 8
            + (runs[1].offset_y * 8.0) as i64
            + large_metrics.above_subpixels;

        assert_eq!(baseline0, baseline1);

        let page = crate::page::parse_xhtml_for_test(
            "https://example.org/",
            "<html xmlns='http://www.w3.org/1999/xhtml'><head/><body/></html>",
        )
        .unwrap();

        let mut layout = PreviewLayout {
            boxes: vec![],
            display_list: vec![PreviewCommand::Text {
                rect: LayoutRect {
                    x: 16,
                    y: 0,
                    width: 220,
                    height: line.height(),
                },
                text: String::new(),
                style: small,
                runs,
            }],
        };

        let (width, height) = (256, 220);
        let mut aligned = vec![0; width * height];

        paint_preview_scene(
            &mut aligned,
            width,
            height,
            &page,
            &layout,
            Some(&data),
            0,
            &mut GlyphRasterCache::new(),
        );

        assert!(
            aligned.iter().any(|&pixel| {
                let red = (pixel >> 16) & 0xff;
                let green = (pixel >> 8) & 0xff;
                red > green
            }),
            "first vector glyph must be visible"
        );

        assert!(
            aligned.iter().any(|&pixel| {
                let blue = pixel & 0xff;
                let green = (pixel >> 8) & 0xff;
                blue > green
            }),
            "second vector glyph must be visible"
        );

        let PreviewCommand::Text { runs, .. } = &mut layout.display_list[0] else {
            panic!("text command expected");
        };

        runs[0].offset_y = 0.0;

        let mut unaligned = vec![0; width * height];

        paint_preview_scene(
            &mut unaligned,
            width,
            height,
            &page,
            &layout,
            Some(&data),
            0,
            &mut GlyphRasterCache::new(),
        );

        assert_ne!(
            aligned, unaligned,
            "fractional alignment should change rasterization"
        );

        // The second glyph must not change when only the first run moves.
        for y in 90..height {
            assert_eq!(
                &aligned[y * width + 116..y * width + 180],
                &unaligned[y * width + 116..y * width + 180]
            );
        }
    }

    #[test]
    fn word_width_includes_earlier_run_overhang() {
        use super::{InlineWord, InlineWordPart, measure_styled_word};

        let normal = TextStyle::initial();

        let mut bold = normal.clone();
        bold.font_weight = FontWeight::Bold;

        let word = InlineWord {
            space_before: None,
            parts: vec![
                InlineWordPart {
                    text: "A",
                    style: &bold,
                },
                InlineWordPart {
                    text: ".",
                    style: &normal,
                },
            ],
        };

        let measured = measure_styled_word(&word, |_, style| {
            if style.font_weight == FontWeight::Bold {
                (30, 10)
            } else {
                (2, 2)
            }
        });

        assert_eq!((measured.runs[1].x, measured.runs[1].width), (10, 2));
        assert_eq!(measured.width(), 30);
    }
}
