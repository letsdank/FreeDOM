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
    graphics::font::{TrueTypeFont, render::split_subpixel_position},
    page::Page,
    page_layout::{
        CONTENT_HORIZONTAL_PADDING, LayoutRect, TextWrap, text_metrics, text_word_width,
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
    pub(crate) y: i32,
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
                    let font = fonts.get(inherited).or(fallback);
                    let words = split_styled_words(&fragments);

                    let base_metrics = PreviewLineMetrics::from_font(font, inherited.font_size);

                    let mut wrap = TextWrap::new(0, width);
                    let mut line_metrics = base_metrics;
                    let mut line_top = 0_i32;

                    let mut runs = Vec::new();
                    let mut pending_runs = Vec::new();
                    let use_positioned_runs = fragments.len() > 1;

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

                            let run_metrics =
                                PreviewLineMetrics::from_font(run_font, run.style.font_size);

                            line_metrics.include(run_metrics);

                            if use_positioned_runs {
                                pending_runs.push(PendingPreviewRun {
                                    x: placement.x.saturating_add(run.x),
                                    above_baseline: run_metrics.above_baseline,
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
    left.font_size.to_bits() == right.font_size.to_bits()
        && left.font_weight == right.font_weight
        && left.font_style == right.font_style
        && left.font_family == right.font_family
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

// Preview-only integer approximation; this is not CSS baseline alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreviewLineMetrics {
    above_baseline: i32,
    below_baseline: i32,
}

impl PreviewLineMetrics {
    fn from_font(font: Option<&TrueTypeFont<'_>>, font_size: f32) -> Self {
        let metrics = text_metrics(font, font_size);
        let height = metrics.line_height.max(0);

        let above = font
            .and_then(|font| font.line_metrics(font_size))
            .and_then(|metrics| split_subpixel_position(metrics.ascent))
            .map(|(base, _phase)| base)
            .unwrap_or(metrics.glyph_height)
            .clamp(0, height);

        Self {
            above_baseline: above,
            below_baseline: height - above,
        }
    }

    fn include(&mut self, other: Self) {
        self.above_baseline = self.above_baseline.max(other.above_baseline);
        self.below_baseline = self.below_baseline.max(other.below_baseline);
    }

    fn height(self) -> i32 {
        self.above_baseline.saturating_add(self.below_baseline)
    }
}

struct PendingPreviewRun {
    x: i32,
    above_baseline: i32,
    text: String,
    style: TextStyle,
}

fn finish_preview_line(
    pending: &mut Vec<PendingPreviewRun>,
    output: &mut Vec<PreviewTextRun>,
    metrics: PreviewLineMetrics,
    line_top: i32,
) {
    for run in pending.drain(..) {
        let baseline_offset = metrics.above_baseline.saturating_sub(run.above_baseline);

        output.push(PreviewTextRun {
            x: run.x,
            y: line_top.saturating_add(baseline_offset),
            text: run.text,
            style: run.style,
        });
    }
}

pub(crate) struct PreviewFonts<'style, 'font> {
    entries: Vec<(&'style TextStyle, Option<TrueTypeFont<'font>>)>,
}

impl<'style, 'font> PreviewFonts<'style, 'font> {
    fn new(page: &'font Page, fragments: &[InlineTextFragment<'style>]) -> Self {
        Self::from_styles(page, fragments.iter().map(|fragment| fragment.style))
    }

    pub(crate) fn from_styles(
        page: &'font Page,
        styles: impl IntoIterator<Item = &'style TextStyle>,
    ) -> Self {
        let mut entries: Vec<(&TextStyle, Option<TrueTypeFont<'font>>)> = Vec::new();

        for style in styles {
            if entries
                .iter()
                .any(|(existing, _)| same_font(existing, style))
            {
                continue;
            }

            let font = page
                .font_data_for(
                    style.font_family.as_deref(),
                    style.font_weight,
                    style.font_style,
                )
                .and_then(|data| TrueTypeFont::parse(data).ok());

            entries.push((style, font));
        }

        Self { entries }
    }

    pub(crate) fn get(&self, style: &TextStyle) -> Option<&TrueTypeFont<'font>> {
        self.entries
            .iter()
            .find(|(existing, _)| same_font(existing, style))
            .and_then(|(_, font)| font.as_ref())
    }

    fn measure_word<'a>(
        &self,
        word: &InlineWord<'a>,
        fallback: Option<&TrueTypeFont<'_>>,
    ) -> MeasuredWord<'a> {
        measure_styled_word(word, |text, style| {
            let font = self.get(style).or(fallback);

            text_word_width(
                font,
                text,
                style.font_size,
                style.font_weight,
                style.font_style,
            )
        })
    }

    fn space_width(&self, word: &InlineWord<'_>, fallback: Option<&TrueTypeFont<'_>>) -> i32 {
        word.space_before.map_or(0, |style| {
            let font = self.get(style).or(fallback);
            text_metrics(font, style.font_size).character_width
        })
    }
}

struct MeasuredRun<'a> {
    x: i32,
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
            .last()
            .map_or(0, |run| run.x.saturating_add(run.width))
    }
}

// Adjacent fragments with the same font and color are measured together so
// that element boundaries do not change kerning within a word.
fn measure_styled_word<'a>(
    word: &InlineWord<'a>,
    mut measure: impl FnMut(&str, &TextStyle) -> i32,
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

        let width = measure(text.as_ref(), style);

        runs.push(MeasuredRun {
            x,
            width,
            text,
            style,
        });

        x = x.saturating_add(width);
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

    for fragment in &fragments {
        let source_path = || {
            fragment
                .style_source
                .map_or_else(Vec::new, |path| path.indices().to_vec())
        };

        if !same_font(inherited, fragment.style) {
            return Err(PreviewError::InlineFont(source_path()));
        }

        if inherited.color != fragment.style.color {
            return Err(PreviewError::InlineStyle(source_path()));
        }
    }

    for word in split_styled_words(&fragments) {
        if word.space_before.is_some() {
            output.push(' ');
        }

        for part in word.parts {
            debug_assert!(same_font(inherited, part.style));
            output.push_str(part.text);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{PreviewError, collapse_sibling_margins, collect_styled_fragments, collect_text};

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

        assert!(matches!(
            collect_text(&items, &normal, &mut text),
            Err(PreviewError::InlineFont(_))
        ));
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
                20
            } else {
                21
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

        let measured = measure_styled_word(&word, |_, _| i32::MAX);

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
                above_baseline: 8,
                below_baseline: 2
            }
        );

        line.include(PreviewLineMetrics::from_font(None, 32.0));

        assert_eq!(
            line,
            PreviewLineMetrics {
                above_baseline: 16,
                below_baseline: 4,
            }
        );

        line.include(PreviewLineMetrics {
            above_baseline: 5,
            below_baseline: 12,
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
                above_baseline: small_metrics.above_baseline,
                text: "small".into(),
                style: small,
            },
            PendingPreviewRun {
                x: 40,
                above_baseline: large_metrics.above_baseline,
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
            runs[0].y + small_metrics.above_baseline,
            runs[1].y + line_metrics.above_baseline
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

            assert_eq!(preview.above_baseline, base, "size={size}",);

            saw_fractional_phase |= phase != 0;
            saw_round_mismatch |= metrics.ascent.round() as i32 != base;
        }

        assert!(saw_fractional_phase);
        assert!(saw_round_mismatch);
    }
}
