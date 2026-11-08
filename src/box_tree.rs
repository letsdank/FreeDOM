//! Experimental normal-flow box structure built from a styled snapshot.
//!
//! CSS 2.2, sections 9.2.1.1 and 9.2.2:
//! <https://www.w3.org/TR/CSS22/visuren.html>
//!
//! This does not perform layout or painting. Block-in-inline splitting and
//! CSS white-space modes are not implemented.

use std::fmt::Write;

use crate::{
    css::{Display, TextStyle},
    styled_tree::{SourcePath, StyledNode, StyledTree},
};

const MAX_NODES: usize = 20_000;
const MAX_DEPTH: usize = 128;

pub(crate) struct BoxTree<'a> {
    pub(crate) children: Vec<FlowChild<'a>>,
}

pub(crate) enum FlowChild<'a> {
    Block(BlockBox<'a>),

    InlineRun {
        anonymous: bool,
        items: Vec<InlineItem<'a>>,
    },
}

pub(crate) struct BlockBox<'a> {
    pub(crate) source: &'a SourcePath,
    pub(crate) tag: &'a str,
    pub(crate) style: &'a TextStyle,
    pub(crate) content: BlockContent<'a>,
}

pub(crate) enum BlockContent<'a> {
    Flow(Vec<FlowChild<'a>>),
    Replaced,
}

pub(crate) enum InlineItem<'a> {
    Text {
        source: Option<&'a SourcePath>,
        text: &'a str,
    },

    Element {
        source: &'a SourcePath,
        tag: &'a str,
        style: &'a TextStyle,
        children: Vec<InlineItem<'a>>,
    },

    Replaced {
        source: &'a SourcePath,
        tag: &'a str,
        style: &'a TextStyle,
    },

    Break {
        source: &'a SourcePath,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BoxTreeError {
    BlockInsideInline(Vec<usize>),
    UnsupportedBlockBreak(Vec<usize>),
    DepthLimit,
    NodeLimit,
}

impl std::fmt::Display for BoxTreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

struct Builder {
    remaining: usize,
}

impl Builder {
    fn visit(&mut self, depth: usize) -> Result<(), BoxTreeError> {
        if depth > MAX_DEPTH {
            return Err(BoxTreeError::DepthLimit);
        }

        if self.remaining == 0 {
            return Err(BoxTreeError::NodeLimit);
        }

        self.remaining -= 1;

        Ok(())
    }

    fn flow<'a>(
        &mut self,
        nodes: &'a [StyledNode],
        depth: usize,
    ) -> Result<Vec<FlowChild<'a>>, BoxTreeError> {
        let has_block = nodes.iter().any(|node| {
            matches!(
                node,
                StyledNode::Element { style, .. }
                    if style.display == Display::Block
            )
        });

        let mut output = Vec::new();
        let mut inline = Vec::new();

        for node in nodes {
            match node {
                StyledNode::Element { style, .. } if style.display == Display::None => {}

                StyledNode::Element { style, .. } if style.display == Display::Block => {
                    flush_inline(&mut output, &mut inline, has_block);

                    output.push(FlowChild::Block(self.block(node, depth)?));
                }

                _ => {
                    if let Some(item) = self.inline(node, depth)? {
                        inline.push(item);
                    }
                }
            }
        }

        flush_inline(&mut output, &mut inline, has_block);

        Ok(output)
    }

    fn block<'a>(
        &mut self,
        node: &'a StyledNode,
        depth: usize,
    ) -> Result<BlockBox<'a>, BoxTreeError> {
        self.visit(depth)?;

        let StyledNode::Element {
            source,
            tag,
            style,
            children,
            ..
        } = node
        else {
            unreachable!();
        };

        let content = match tag.as_str() {
            "br" => {
                return Err(BoxTreeError::UnsupportedBlockBreak(
                    source.indices().to_vec(),
                ));
            }

            "img" | "svg" => BlockContent::Replaced,

            _ => BlockContent::Flow(self.flow(children, depth + 1)?),
        };

        Ok(BlockBox {
            source,
            tag,
            style,
            content,
        })
    }

    fn inline<'a>(
        &mut self,
        node: &'a StyledNode,
        depth: usize,
    ) -> Result<Option<InlineItem<'a>>, BoxTreeError> {
        self.visit(depth)?;

        match node {
            StyledNode::Text { source, text } => Ok(Some(InlineItem::Text {
                source: source.as_ref(),
                text,
            })),

            StyledNode::Element {
                source,
                tag,
                style,
                children,
                ..
            } => {
                match style.display {
                    Display::None => return Ok(None),

                    Display::Block => {
                        return Err(BoxTreeError::BlockInsideInline(source.indices().to_vec()));
                    }

                    Display::Inline => {}
                }

                match tag.as_str() {
                    "br" => Ok(Some(InlineItem::Break { source })),

                    "img" | "svg" => Ok(Some(InlineItem::Replaced { source, tag, style })),

                    _ => {
                        let mut result = Vec::new();

                        for child in children {
                            if let Some(item) = self.inline(child, depth + 1)? {
                                result.push(item);
                            }
                        }

                        Ok(Some(InlineItem::Element {
                            source,
                            tag,
                            style,
                            children: result,
                        }))
                    }
                }
            }
        }
    }
}

fn is_css_whitespace(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\n' | '\r' | '\x0c')
}

fn flush_inline<'a>(
    output: &mut Vec<FlowChild<'a>>,
    inline: &mut Vec<InlineItem<'a>>,
    anonymous: bool,
) {
    // Under normal white-space handling, an all-whitespace run has no box.
    let meaningful = inline.iter().any(|item| match item {
        InlineItem::Text { text, .. } => {
            text.chars().any(|character| !is_css_whitespace(character))
        }

        _ => true,
    });

    if meaningful {
        output.push(FlowChild::InlineRun {
            anonymous,
            items: std::mem::take(inline),
        });
    } else {
        inline.clear();
    }
}

impl<'a> BoxTree<'a> {
    pub(crate) fn build(styled: &'a StyledTree) -> Result<Self, BoxTreeError> {
        let mut builder = Builder {
            remaining: MAX_NODES,
        };

        Ok(Self {
            children: builder.flow(styled.roots(), 0)?,
        })
    }

    pub(crate) fn format_debug(&self) -> String {
        let mut output = String::from(
            "FreeDOM experimental box structure \
             (F7; not used for painting):\n",
        );

        let mut remaining = 300;

        format_flow(&self.children, 0, &mut remaining, &mut output);

        if remaining == 0 {
            output.push_str("... output limit reached\n");
        }

        output
    }
}

fn format_flow(nodes: &[FlowChild<'_>], depth: usize, remaining: &mut usize, output: &mut String) {
    for node in nodes {
        if *remaining == 0 {
            return;
        }

        *remaining -= 1;

        match node {
            FlowChild::Block(block) => {
                let _ = writeln!(
                    output,
                    "{:indent$}block <{}> path={:?} font-size={:.1}px",
                    "",
                    block.tag,
                    block.source.indices(),
                    block.style.font_size,
                    indent = depth * 2,
                );

                match &block.content {
                    BlockContent::Flow(children) => {
                        format_flow(children, depth + 1, remaining, output);
                    }

                    BlockContent::Replaced => {
                        let _ = writeln!(
                            output,
                            "{:indent$}replaced content",
                            "",
                            indent = (depth + 1) * 2,
                        );
                    }
                }
            }

            FlowChild::InlineRun { anonymous, items } => {
                let kind = if *anonymous {
                    "anonymous block:"
                } else {
                    "inline context:"
                };

                let _ = writeln!(
                    output,
                    "{:indent$}{kind} inline run",
                    "",
                    indent = depth * 2
                );

                format_inline(items, depth + 1, remaining, output);
            }
        }
    }
}

fn format_inline(
    nodes: &[InlineItem<'_>],
    depth: usize,
    remaining: &mut usize,
    output: &mut String,
) {
    for node in nodes {
        if *remaining == 0 {
            return;
        }

        *remaining -= 1;

        match node {
            InlineItem::Text { source, text } => {
                let preview: String = text.chars().take(60).collect();

                let _ = writeln!(
                    output,
                    "{:indent$}text {preview:?} path={:?}",
                    "",
                    source.map(|path| path.indices()),
                    indent = depth * 2
                );
            }

            InlineItem::Element {
                source,
                tag,
                style,
                children,
            } => {
                let _ = writeln!(
                    output,
                    "{:indent$}inline <{tag}> path={:?} \
                     font-size:{:.1}px",
                    "",
                    source.indices(),
                    style.font_size,
                    indent = depth * 2
                );

                format_inline(children, depth + 1, remaining, output);
            }

            InlineItem::Replaced { source, tag, style } => {
                let _ = writeln!(
                    output,
                    "{:indent$}inline replaced <{tag}> path={:?} \
                     font-size:{:.1}px",
                    "",
                    source.indices(),
                    style.font_size,
                    indent = depth * 2
                );
            }

            InlineItem::Break { source } => {
                let _ = writeln!(
                    output,
                    "{:indent$}<br> path={:?}",
                    "",
                    source.indices(),
                    indent = depth * 2
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{css::build_author_rules, html::Document, javascript::DocumentState};

    fn make_styled(input: &str) -> StyledTree {
        let html = Document::parse(input);

        StyledTree::build(&html, &build_author_rules(&[]), &DocumentState::default())
    }

    #[test]
    fn groups_inline_runs_around_nested_blocks() {
        let styled = make_styled(r#"<div>before <span>inside</span><div>child</div> after</div>"#);
        let tree = BoxTree::build(&styled).unwrap();

        let [FlowChild::Block(root)] = tree.children.as_slice() else {
            panic!("expected div");
        };

        assert_eq!(root.tag, "div");
        assert_eq!(root.source.indices(), &[0]);

        let BlockContent::Flow(children) = &root.content else {
            panic!("expected flow");
        };

        let [
            FlowChild::InlineRun {
                anonymous: first_anonymous,
                items: first,
            },
            FlowChild::Block(child),
            FlowChild::InlineRun {
                anonymous: last_anonymous,
                items: last,
            },
        ] = children.as_slice()
        else {
            panic!("expected inline/block/inline");
        };

        assert!(*first_anonymous);
        assert!(*last_anonymous);

        assert_eq!(child.tag, "div");
        assert_eq!(child.source.indices(), &[0, 2]);

        assert!(matches!(
            first.as_slice(),
            [
                InlineItem::Text {
                    text: "before ",
                    ..
                },
                InlineItem::Element { tag: "span", .. }
            ]
        ));

        assert!(matches!(
            last.as_slice(),
            [InlineItem::Text { text: " after", .. }]
        ));

        assert!(matches!(
            styled.source_node(child.source),
            Some(crate::html::Node::Element(element))
                if element.name == "div"
        ));
    }

    #[test]
    fn omits_whitespace_only_between_blocks_but_keeps_nbsp() {
        let styled = make_styled("<div><p>A</p> \n <p>B</p></div>");
        let tree = BoxTree::build(&styled).unwrap();

        let FlowChild::Block(root) = &tree.children[0] else {
            panic!("expected div");
        };

        let BlockContent::Flow(children) = &root.content else {
            panic!("expected flow");
        };

        assert_eq!(children.len(), 2);

        let styled = make_styled("<div><p>A</p>\u{00a0}<p>B</p></div>");
        let tree = BoxTree::build(&styled).unwrap();

        let FlowChild::Block(root) = &tree.children[0] else {
            panic!("expected div");
        };

        let BlockContent::Flow(children) = &root.content else {
            panic!("expected flow");
        };

        assert_eq!(children.len(), 3);
    }

    #[test]
    fn treats_inline_images_and_breaks_as_distinct_items() {
        let styled = make_styled("<p>A<img><br>B</p>");
        let tree = BoxTree::build(&styled).unwrap();

        let FlowChild::Block(root) = &tree.children[0] else {
            panic!("expected p");
        };

        let BlockContent::Flow(children) = &root.content else {
            panic!("expected flow");
        };

        let [FlowChild::InlineRun { anonymous, items }] = children.as_slice() else {
            panic!("expected inline run");
        };

        assert!(!anonymous);

        assert!(matches!(
            items.as_slice(),
            [
                InlineItem::Text { .. },
                InlineItem::Replaced { tag: "img", .. },
                InlineItem::Break { .. },
                InlineItem::Text { .. }
            ]
        ));

        assert!(tree.format_debug().contains("inline replaced <img"));
    }

    #[test]
    fn refuses_block_inside_inline_instead_of_silently_misplacing_it() {
        let style = make_styled("<span><div>block</div><span>");

        assert!(matches!(
            BoxTree::build(&style),
            Err(BoxTreeError::BlockInsideInline(path))
                if path == [0, 0]
        ));
    }
}
