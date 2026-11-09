//! Experimental geometry for a small subset of normal-flow block layout.
//!
//! CSS 2.2, sections 9.4.1 and 10.6.3:
//! <https://www.w3.org/TR/CSS22/visuren.html#block-formatting>
//! <https://www.w3.org/TR/CSS22/visudet.html#normal-block>
//!
//! This is an inspector preview, not the page renderer or a conforming layout.

use std::fmt::Write;

use crate::{
    box_tree::{BlockBox, BlockContent, BoxTree, BoxTreeError, FlowChild, InlineItem},
    css::TextStyle,
    graphics::font::TrueTypeFont,
    page::Page,
    page_layout::{CONTENT_HORIZONTAL_PADDING, LayoutRect, measure_wrapped_text},
    styled_tree::SourcePath,
};

pub(crate) struct PreviewBox {
    pub(crate) source: SourcePath,
    pub(crate) rect: LayoutRect,
}

pub(crate) struct PreviewLayout {
    pub(crate) boxes: Vec<PreviewBox>,
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

        let mut output = Self { boxes: Vec::new() };
        let root_style = TextStyle::initial();

        let flow = output.flow(
            &tree.children,
            &root_style,
            page,
            fallback.as_ref(),
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

                    self.boxes.push(PreviewBox {
                        source: block.source.clone(),
                        rect: LayoutRect {
                            x: CONTENT_HORIZONTAL_PADDING as i32,
                            y: start,
                            width,
                            height: 0,
                        },
                    });

                    let child_start = if is_root {
                        FlowStart::RootContent
                    } else {
                        FlowStart::Collapsed
                    };

                    let mut child_flow = self.flow(
                        children,
                        block.style,
                        page,
                        fallback,
                        width,
                        start,
                        child_start,
                    )?;

                    // The root element's margins do not collapse with its children.
                    // https://www.w3.org/TR/CSS22/box.html#collapsing-margins
                    if is_root {
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

                    let end = child_flow.end;
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

                    let page_font = page
                        .font_data_for(
                            inherited.font_family.as_deref(),
                            inherited.font_weight,
                            inherited.font_style,
                        )
                        .and_then(|data| TrueTypeFont::parse(data).ok());

                    let font = page_font.as_ref().or(fallback);

                    let height = measure_wrapped_text(
                        &text,
                        width,
                        inherited.font_size,
                        inherited.font_weight,
                        inherited.font_style,
                        font,
                    );

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

        if matches!(&child.content, BlockContent::Flow(children) if children.is_empty()) {
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

fn collect_text(
    items: &[InlineItem<'_>],
    inherited: &TextStyle,
    output: &mut String,
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

                output.push_str(text);
            }

            InlineItem::Element {
                source,
                style,
                children,
                ..
            } => {
                if !same_font(inherited, style) {
                    return Err(PreviewError::InlineFont(source.indices().to_vec()));
                }

                collect_text(children, style, output)?;
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

#[cfg(test)]
mod tests {
    use super::collapse_sibling_margins;

    #[test]
    fn collapses_signed_margins_without_overflow() {
        assert_eq!(collapse_sibling_margins(8, 12), 12);
        assert_eq!(collapse_sibling_margins(8, -3), 5);
        assert_eq!(collapse_sibling_margins(-5, -3), -5);
        assert_eq!(collapse_sibling_margins(i32::MAX, i32::MIN), -1);
    }
}
