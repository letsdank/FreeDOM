//! Experimental geometry for a small subset of normal-flow block layout.
//!
//! CSS 2.2, sections 9.4.1 and 10.6.3:
//! <https://www.w3.org/TR/CSS22/visuren.html#block-formatting>
//! <https://www.w3.org/TR/CSS22/visudet.html#normal-block>
//!
//! This is an inspector preview, not the page renderer or a conforming layout.

use crate::{
    box_tree::{BlockBox, BlockContent, BoxTree, BoxTreeError, FlowChild, InlineItem},
    css::{
        BackgroundColor, BorderEdge, CssSize, FontStyle, FontWeight, LineHeight, TextStyle,
        WhiteSpace,
    },
    graphics::font::TrueTypeFont,
    inline_layout::{
        InlineFonts, InlineTextFragment, PositionedTextRun, layout_inline_words, same_font,
        same_font_except_weight, split_styled_words,
    },
    page::Page,
    page_layout::{CONTENT_HORIZONTAL_PADDING, LayoutRect, text_metrics},
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
    InlineBackground {
        rect: LayoutRect,
        color: u32,
    },
    InlineText {
        x: i32,
        y: i32,
        text: String,
        style: TextStyle,
        source: Option<SourcePath>,
    },
    Text {
        rect: LayoutRect,
        text: String,
        style: TextStyle,
        runs: Vec<PositionedTextRun>,
    },
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
    Padding(Vec<usize>),
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

            Self::Padding(path) => {
                write!(formatter, "padding is not supported at {path:?}")
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

                    // Padding, like a border, separates adjoining vertical margins.
                    // https://www.w3.org/TR/CSS22/box.html#collapsing-margins
                    let child_start = if is_root
                        || block.style.border_top.width > 0
                        || block.style.padding_top > 0
                    {
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
                            .and_then(|value| value.checked_add(block.style.padding_top))
                            .ok_or(PreviewError::SizeOverflow)?,
                        child_start,
                    )?;

                    // Root, border and padding separate bottom margins from children.
                    // https://www.w3.org/TR/CSS22/box.html#collapsing-margins
                    if (is_root
                        || block.style.border_bottom.width > 0
                        || block.style.padding_bottom > 0)
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
                                .and_then(|value| value.checked_add(block.style.padding_top))
                                .and_then(|value| value.checked_add(height))
                                .ok_or(PreviewError::SizeOverflow)?
                        }
                    };

                    let end = end
                        .checked_add(block.style.padding_bottom)
                        .ok_or(PreviewError::SizeOverflow)?
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

                    let fonts = InlineFonts::new(page, &fragments);
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
                                    && !fonts.can_measure_vector_word(word, fallback)
                            })
                        });

                        if has_vector_font && (unsupported_word || has_bitmap_font) {
                            let path = fragment
                                .style_source
                                .map_or_else(Vec::new, |path| path.indices().to_vec());

                            return Err(PreviewError::InlineFont(path));
                        }
                    }

                    let use_positioned_runs = fragments.len() > 1
                        || inherited.white_space == WhiteSpace::Nowrap
                        || inherited.line_height != LineHeight::Normal
                        || fragments.iter().any(|fragment| {
                            fragment.style.color != inherited.color
                                || (fragment.style_source.is_some()
                                    && fragment.style.background_color
                                        != BackgroundColor::Transparent)
                                || fragment.style.line_height != inherited.line_height
                                || !same_font(inherited, fragment.style)
                        });

                    let (height, runs) = layout_inline_words(
                        &fonts,
                        &words,
                        inherited,
                        width,
                        fallback,
                        use_positioned_runs,
                    );

                    let background_styles: std::collections::HashMap<_, _> = fragments
                        .iter()
                        .filter_map(|fragment| {
                            let source = fragment.style_source?;
                            (fragment.style.background_color != BackgroundColor::Transparent)
                                .then_some((source.indices(), fragment.style))
                        })
                        .collect();

                    let mut backgrounds: Vec<(SourcePath, usize, LayoutRect, u32)> = Vec::new();

                    for run in runs.iter().filter(|_| !background_styles.is_empty()) {
                        let spaces = run
                            .leading_space
                            .iter()
                            .map(|space| (&space.source, Some((space.x, space.width)), None));
                        let spans = run.sources.iter().filter_map(|span| {
                            span.source
                                .as_ref()
                                .map(|source| (source, None, Some(span)))
                        });
                        for (source, space, span) in spaces.chain(spans) {
                            let Some(style) = background_styles.get(source.indices()).copied()
                            else {
                                continue;
                            };
                            let BackgroundColor::Opaque(color) = style.background_color else {
                                continue;
                            };
                            // Restrict this preview to the current unshaped bitmap path.
                            // The CSS inline box is not generally the glyph's ink bounds.
                            if fonts.get(style).or(fallback).is_some()
                                || style.line_height != inherited.line_height
                                || inherited.font_weight != FontWeight::Normal
                                || inherited.font_style != FontStyle::Normal
                                || style.font_weight != FontWeight::Normal
                                || style.font_style != FontStyle::Normal
                                || style.font_size <= 0.0
                            {
                                return Err(PreviewError::InlineStyle(source.indices().to_vec()));
                            }
                            let (relative_x, span_width) = if let Some((x, width)) = space {
                                (x, width)
                            } else {
                                let Some(advance) = span.and_then(|span| span.advance.as_ref())
                                else {
                                    return Err(PreviewError::InlineStyle(
                                        source.indices().to_vec(),
                                    ));
                                };
                                let x_start = advance.start.floor() as i32;
                                let x_end = advance.end.ceil() as i32;
                                let x = run
                                    .x
                                    .checked_add(x_start)
                                    .ok_or(PreviewError::SizeOverflow)?;
                                (x, x_end.saturating_sub(x_start))
                            };
                            let x = left
                                .checked_add(relative_x)
                                .ok_or(PreviewError::SizeOverflow)?;
                            let y = cursor
                                .checked_add(run.y)
                                .and_then(|y| y.checked_sub(style.padding_top))
                                .ok_or(PreviewError::SizeOverflow)?;
                            let height = text_metrics(None, style.font_size)
                                .glyph_height
                                .checked_add(style.padding_top)
                                .and_then(|height| height.checked_add(style.padding_bottom))
                                .ok_or(PreviewError::SizeOverflow)?;
                            let rect = LayoutRect {
                                x,
                                y,
                                width: span_width,
                                height,
                            };
                            if let Some((
                                previous_source,
                                previous_line,
                                previous,
                                previous_color,
                            )) = backgrounds.last_mut()
                            {
                                if previous_source == source
                                    && *previous_line == run.line_index
                                    && previous.y == rect.y
                                    && *previous_color == color
                                {
                                    let right = rect
                                        .x
                                        .checked_add(rect.width)
                                        .ok_or(PreviewError::SizeOverflow)?;
                                    previous.width = right
                                        .checked_sub(previous.x)
                                        .ok_or(PreviewError::SizeOverflow)?;
                                    continue;
                                }
                            }
                            backgrounds.push((source.clone(), run.line_index, rect, color));
                        }
                    }

                    if backgrounds.is_empty() {
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
                    } else {
                        if runs.iter().any(|run| {
                            fonts.get(&run.style).or(fallback).is_some()
                                || run.style.line_height != inherited.line_height
                                || run.offset_x != 0.0
                                || run.offset_y != 0.0
                        }) {
                            return Err(PreviewError::InlineFont(Vec::new()));
                        }
                        append_ordered_bitmap_inline(
                            &mut self.display_list,
                            &runs,
                            &backgrounds,
                            left,
                            cursor,
                        )?;
                    }

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

        let runs = self.display_list.iter().flat_map(|command| match command {
            PreviewCommand::Text { runs, .. } => runs.as_slice(),
            PreviewCommand::Block(_)
            | PreviewCommand::InlineBackground { .. }
            | PreviewCommand::InlineText { .. } => &[],
        });
        for run in runs.filter(|run| !run.sources.is_empty()).take(32) {
            let sources: Vec<_> = run
                .sources
                .iter()
                .map(|span| {
                    (
                        span.range.clone(),
                        span.source.as_ref().map(SourcePath::indices),
                        span.advance.clone(),
                    )
                })
                .collect();
            let _ = writeln!(
                output,
                "  inline sources={sources:?} relative_x={} relative_y={} width={}",
                run.x, run.y, run.width
            );
        }

        for command in self
            .display_list
            .iter()
            .filter_map(|command| match command {
                PreviewCommand::InlineText {
                    x,
                    y,
                    source: Some(source),
                    ..
                } => Some((x, y, source)),
                _ => None,
            })
            .take(32)
        {
            let (x, y, source) = command;
            let _ = writeln!(output, "  inline source={:?} x={x} y={y}", source.indices());
        }

        if self.boxes.len() > 32 {
            output.push_str("  ... output limit reached\n");
        }

        output
    }
}

// Bitmap-only preview: preserve inline/line painting order without remeasuring
// source fragments or losing the positioning calculated for the whole word.
// https://www.w3.org/TR/CSS22/zindex.html#painting-order
fn append_ordered_bitmap_inline(
    output: &mut Vec<PreviewCommand>,
    runs: &[PositionedTextRun],
    backgrounds: &[(SourcePath, usize, LayoutRect, u32)],
    left: i32,
    top: i32,
) -> Result<(), PreviewError> {
    let indices: std::collections::HashMap<_, _> = backgrounds
        .iter()
        .enumerate()
        .map(|(index, (source, y, _, _))| ((source.indices(), *y), index))
        .collect();
    let mut painted = vec![false; backgrounds.len()];
    let mut emit_background =
        |source: &SourcePath, line: usize, output: &mut Vec<PreviewCommand>| {
            if let Some(&index) = indices.get(&(source.indices(), line)) {
                if !painted[index] {
                    let (_, _, rect, color) = &backgrounds[index];
                    output.push(PreviewCommand::InlineBackground {
                        rect: *rect,
                        color: *color,
                    });
                    painted[index] = true;
                }
            }
        };

    for run in runs {
        if let Some(space) = &run.leading_space {
            emit_background(&space.source, run.line_index, output);
        }
        let y = top.checked_add(run.y).ok_or(PreviewError::SizeOverflow)?;
        let x = left.checked_add(run.x).ok_or(PreviewError::SizeOverflow)?;
        if run.sources.is_empty() {
            output.push(PreviewCommand::InlineText {
                x,
                y,
                text: run.text.clone(),
                style: run.style.clone(),
                source: None,
            });
            continue;
        }
        for span in &run.sources {
            if let Some(source) = &span.source {
                emit_background(source, run.line_index, output);
            }
            let Some(advance) = &span.advance else {
                return Err(PreviewError::InlineStyle(Vec::new()));
            };
            let Some(text) = run.text.get(span.range.clone()) else {
                return Err(PreviewError::InlineStyle(Vec::new()));
            };
            let x = x
                .checked_add(advance.start as i32)
                .ok_or(PreviewError::SizeOverflow)?;
            output.push(PreviewCommand::InlineText {
                x,
                y,
                text: text.to_owned(),
                style: run.style.clone(),
                source: span.source.clone(),
            });
        }
    }
    if painted.iter().any(|painted| !painted) {
        return Err(PreviewError::InlineStyle(Vec::new()));
    }
    Ok(())
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
        if current.style.border_top.width > 0 || current.style.padding_top > 0 {
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
            && child.style.padding_top == 0
            && child.style.padding_bottom == 0
        {
            return Err(PreviewError::EmptyBlock(child.source.indices().to_vec()));
        }

        group.add(child.style.margin_before);
        current = child;
    }

    Ok(group)
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
                        link: None,
                    });
                }
            }

            InlineItem::Element {
                source,
                style,
                children,
                ..
            } => {
                if (style.padding_top != 0 || style.padding_bottom != 0)
                    && style.background_color == BackgroundColor::Transparent
                {
                    return Err(PreviewError::Padding(source.indices().to_vec()));
                }
                if style.background_color != BackgroundColor::Transparent {
                    let [InlineItem::Text { text, .. }] = children.as_slice() else {
                        return Err(PreviewError::InlineStyle(source.indices().to_vec()));
                    };
                    // Segment-break transformation is not implemented here.
                    if text.is_empty()
                        || text
                            .chars()
                            .any(|ch| ch.is_whitespace() && !matches!(ch, ' ' | '\t'))
                    {
                        return Err(PreviewError::InlineStyle(source.indices().to_vec()));
                    }
                }
                if style.border_top.width != 0
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

        if inherited.white_space != fragment.style.white_space {
            return Err(PreviewError::InlineStyle(source_path()));
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
    fn collapses_whitespace_across_fragments() {
        use super::{InlineTextFragment, split_styled_words};

        let normal = TextStyle::initial();

        let fragments = vec![
            InlineTextFragment {
                text: " \t",
                style: &normal,
                style_source: None,
                link: None,
            },
            InlineTextFragment {
                text: "one",
                style: &normal,
                style_source: None,
                link: None,
            },
            InlineTextFragment {
                text: "\n",
                style: &normal,
                style_source: None,
                link: None,
            },
            InlineTextFragment {
                text: "\t two \r",
                style: &normal,
                style_source: None,
                link: None,
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
            link: None,
        }];

        assert!(split_styled_words(&whitespace_only).is_empty());
    }
}
