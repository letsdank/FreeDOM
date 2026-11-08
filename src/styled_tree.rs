//! A styled snapshot of the parsed HTML tree.
//!
//! CSS Display: <https://www.w3.org/TR/css-display-3/#box-generation>
//!
//! This is not yet the CSS box tree or a live DOM. The existing block renderer
//! still performs layout and painting separately.

use std::fmt::Write;

use crate::{
    css::{CssRule, Display, TextStyle, compute_style},
    html::{Document, Node},
    javascript::DocumentState,
    page::element_info,
};

/// Child indexes from the document root, valid only for this source snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourcePath(Vec<usize>);

impl SourcePath {
    pub(crate) fn indices(&self) -> &[usize] {
        &self.0
    }

    pub(crate) fn from_indices(indices: &[usize]) -> Self {
        Self(indices.to_vec())
    }
}

#[derive(Default)]
pub(crate) struct StyledTree {
    source: Document,
    roots: Vec<StyledNode>,
}

pub(crate) enum StyledNode {
    Element {
        source: SourcePath,
        tag: String,
        id: Option<String>,
        style: TextStyle,
        children: Vec<StyledNode>,
    },

    Text {
        // A script-provided text override has no corresponding HTML text node.
        source: Option<SourcePath>,
        text: String,
    },
}

impl StyledTree {
    pub(crate) fn build(source: &Document, rules: &[CssRule], js: &DocumentState) -> Self {
        let mut path = Vec::new();

        let roots = build_children(
            &source.children,
            &TextStyle::initial(),
            rules,
            js,
            &mut path,
        );

        Self {
            source: source.clone(),
            roots,
        }
    }

    pub(crate) fn source_node(&self, path: &SourcePath) -> Option<&Node> {
        let mut children = self.source.children.as_slice();
        let (last, parents) = path.0.split_last()?;

        for &index in parents {
            let Node::Element(element) = children.get(index)? else {
                return None;
            };

            children = &element.children;
        }

        children.get(*last)
    }

    pub(crate) fn roots(&self) -> &[StyledNode] {
        &self.roots
    }

    pub(crate) fn format_debug(&self) -> String {
        let mut out = String::from("FreeDOM styled snapshot (F4; not a CSS box tree):\n");

        let mut remaining = 300usize;

        format_nodes(self, self.roots(), 0, &mut remaining, &mut out);

        if remaining == 0 {
            out.push_str("... output limit reached\n");
        }

        out
    }
}

fn build_children(
    nodes: &[Node],
    inherited: &TextStyle,
    rules: &[CssRule],
    js: &DocumentState,
    path: &mut Vec<usize>,
) -> Vec<StyledNode> {
    let mut result = Vec::new();

    for (index, node) in nodes.iter().enumerate() {
        path.push(index);

        if let Some(styled) = build_node(node, inherited, rules, js, path) {
            result.push(styled);
        }

        path.pop();
    }

    result
}

fn build_node(
    node: &Node,
    inherited: &TextStyle,
    rules: &[CssRule],
    js: &DocumentState,
    path: &mut Vec<usize>,
) -> Option<StyledNode> {
    if let Node::Text(text) = node {
        return Some(StyledNode::Text {
            source: Some(SourcePath(path.clone())),
            text: text.clone(),
        });
    }

    let Node::Element(element) = node else {
        return None;
    };

    let info = element_info(node)?;

    if matches!(
        info.tag.as_str(),
        "script" | "style" | "template" | "head" | "title" | "meta" | "link"
    ) {
        return None;
    }

    let style = compute_style(&info.css_element(), inherited.clone(), rules);

    if style.display == Display::None {
        return None;
    }

    let children = if matches!(info.tag.as_str(), "img" | "svg" | "br") {
        Vec::new()
    } else if let Some(text) = info
        .id
        .as_deref()
        .and_then(|id| js.text_content_override(id))
    {
        vec![StyledNode::Text {
            source: None,
            text: text.to_owned(),
        }]
    } else {
        build_children(&element.children, &style, rules, js, path)
    };

    Some(StyledNode::Element {
        source: SourcePath(path.clone()),
        tag: info.tag,
        id: info.id,
        style,
        children,
    })
}

fn format_nodes(
    tree: &StyledTree,
    nodes: &[StyledNode],
    depth: usize,
    remaining: &mut usize,
    out: &mut String,
) {
    if depth > 40 {
        let _ = writeln!(out, "{:indent$}... depth limit", "", indent = 80);
        return;
    }

    for node in nodes {
        if *remaining == 0 {
            return;
        }

        match node {
            StyledNode::Element {
                source,
                tag,
                id,
                style,
                children,
            } => {
                *remaining -= 1;

                let display = match style.display {
                    Display::Block => "block",
                    Display::Inline => "inline",
                    Display::None => "none",
                };

                let is_valid = matches!(tree.source_node(source), Some(Node::Element(_)));

                let _ = writeln!(
                    out,
                    "{:indent$}<{tag}> id={id:?} path={:?} \
                     display={display} color=#{:06x} font-size={:.1}px\
                     source={}",
                    "",
                    source.indices(),
                    style.color & 0xffffff,
                    style.font_size,
                    if is_valid { "html" } else { "invalid" },
                    indent = depth * 2
                );

                format_nodes(tree, children, depth + 1, remaining, out);
            }

            StyledNode::Text { source, text } => {
                if text.trim().is_empty() {
                    continue;
                }

                *remaining -= 1;

                let preview: String = text.chars().take(80).collect();

                let source_label = if source.is_some() {
                    "html"
                } else {
                    "script override"
                };

                let _ = writeln!(
                    out,
                    "{:indent$}text {preview:?} source={source_label}",
                    "",
                    indent = depth * 2
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::build_author_rules;

    #[test]
    fn preserves_nesting_styles_and_source_paths() {
        let html = Document::parse(
            r#"<main id="outer"><p id="same">Hello <span id="same">world</span></p><p class="hidden">no</p></main>"#,
        );

        let rules = build_author_rules(&[String::from(
            ".hidden { display: none } #outer { color: red }",
        )]);

        let tree = StyledTree::build(&html, &rules, &DocumentState::default());

        let [
            StyledNode::Element {
                source,
                children,
                style,
                ..
            },
        ] = tree.roots.as_slice()
        else {
            panic!("main missing");
        };

        assert_eq!(source.0, [0]);
        assert_eq!(style.color, 0x00_ff_00_00);

        // The hidden paragraph is not present.
        assert_eq!(children.len(), 1);

        let StyledNode::Element {
            source: paragraph,
            children,
            ..
        } = &children[0]
        else {
            panic!("p missing");
        };

        assert_eq!(paragraph.0, [0, 0]);

        let StyledNode::Element {
            source: span,
            style,
            ..
        } = &children[1]
        else {
            panic!("span missing");
        };

        // Duplicate HTML ids still have different source paths.
        assert_eq!(span.0, [0, 0, 1]);
        assert_ne!(paragraph, span);
        assert_eq!(style.color, 0x00_ff_00_00);

        assert!(matches!(
            tree.source_node(span),
            Some(Node::Element(element)) if element.name == "span",
        ));

        let formatted = tree.format_debug();

        assert!(formatted.contains("<main>"));
        assert!(formatted.contains("<span>"));
        assert!(!formatted.contains("text \"no\""));
    }
}
