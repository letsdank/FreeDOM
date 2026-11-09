//! Experimental geometry for a small subset of normal-flow block layout.
//!
//! CSS 2.2, sections 9.4.1 and 10.6.3:
//! <https://www.w3.org/TR/CSS22/visuren.html#block-formatting>
//! <https://www.w3.org/TR/CSS22/visudet.html#normal-block>
//!
//! This is an inspector preview, not the page renderer or a conforming layout.

use std::fmt::Write;

use crate::{
    box_tree::{BlockContent, BoxTree, BoxTreeError, FlowChild, InlineItem},
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

#[derive(Debug)]
pub(crate) enum PreviewError {
    Tree(BoxTreeError),
    Margin(Vec<usize>),
    Replaced(Vec<usize>),
    LineBreak(Vec<usize>),
    InlineFont(Vec<usize>),
    WhiteSpace,
    SizeOverflow,
}

impl std::fmt::Display for PreviewError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tree(error) => {
                write!(formatter, "box tree: {error}")
            }

            Self::Margin(path) => {
                write!(formatter, "margins are not supported at {path:?}")
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

        output.flow(
            &tree.children,
            &root_style,
            page,
            fallback.as_ref(),
            width,
            0,
        )?;

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
    ) -> Result<i32, PreviewError> {
        let mut cursor = top;

        for child in children {
            match child {
                FlowChild::Block(block) => {
                    if block.style.margin_after != 0 {
                        return Err(PreviewError::Margin(block.source.indices().to_vec()));
                    }

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
                            y: cursor,
                            width,
                            height: 0,
                        },
                    });

                    let end = self.flow(children, block.style, page, fallback, width, cursor)?;

                    self.boxes[index].rect.height =
                        end.checked_sub(cursor).ok_or(PreviewError::SizeOverflow)?;

                    cursor = end;
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

        Ok(cursor)
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
