use std::collections::HashMap;

use crate::html::{Document as HtmlDocument, Node as HtmlNode};

use super::JsError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AttributeState {
    pub(crate) name: String,
    pub(crate) value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ElementState {
    pub(crate) node_id: u64,
    pub(crate) id: String,
    pub(crate) tag_name: String,
    pub(crate) text_content: String,
    pub(crate) text_content_was_set: bool,
    pub(crate) attributes: Vec<AttributeState>,
    pub(crate) parent: Option<u64>,
    pub(crate) children: Vec<u64>,
    pub(crate) connected: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocumentState {
    pub title: String,
    root_nodes: Vec<u64>,
    elements_by_node_id: HashMap<u64, ElementState>,
    body: Option<u64>,
    next_node_id: u64,
}

impl DocumentState {
    pub fn with_title(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Self::default()
        }
    }

    pub(crate) fn insert_element(
        &mut self,
        id: &str,
        tag_name: &str,
        text_content: String,
        parent: Option<u64>,
        connected: bool,
        is_document_node: bool,
    ) -> u64 {
        let node_id = self.next_node_id;
        self.next_node_id += 1;

        let parent = parent.filter(|node_id| self.elements_by_node_id.contains_key(node_id));

        let mut attributes = Vec::new();

        if !id.is_empty() {
            attributes.push(AttributeState {
                name: "id".to_owned(),
                value: id.to_owned(),
            });
        }

        self.elements_by_node_id.insert(
            node_id,
            ElementState {
                node_id,
                id: id.to_owned(),
                tag_name: tag_name.to_owned(),
                text_content,
                text_content_was_set: false,
                attributes,
                parent,
                children: Vec::new(),
                connected,
            },
        );

        if let Some(parent_node_id) = parent {
            if let Some(parent) = self.elements_by_node_id.get_mut(&parent_node_id) {
                parent.children.push(node_id);
            }
        }

        if is_document_node && parent.is_none() {
            self.root_nodes.push(node_id);
        }

        node_id
    }

    pub fn register_parsed_element(
        &mut self,
        tag_name: &str,
        id: &str,
        text_content: String,
        parent: Option<u64>,
    ) -> u64 {
        let node_id = self.insert_element(id, tag_name, text_content, parent, true, true);

        if tag_name == "body" && self.body.is_none() {
            self.body = Some(node_id);
        }

        node_id
    }

    pub fn register_element(&mut self, id: &str, tag_name: &str, text_content: String) {
        if !id.is_empty() {
            self.register_parsed_element(tag_name, id, text_content, None);
        }
    }

    pub fn register_body(&mut self, id: &str, text_content: String) {
        if self.body.is_none() {
            self.register_parsed_element("body", id, text_content, None);
        }
    }

    pub fn register_element_attribute(&mut self, node_id: u64, name: &str, value: String) {
        self.set_node_attribute(node_id, name, value);
    }

    pub(crate) fn create_element(&mut self, tag_name: &str) -> u64 {
        self.insert_element("", tag_name, String::new(), None, false, false)
    }

    pub(crate) fn body_node(&self) -> Option<u64> {
        self.body
    }

    pub(crate) fn get_element_by_id(&self, id: &str) -> Option<u64> {
        if id.is_empty() {
            return None;
        }

        for node_id in &self.root_nodes {
            if let Some(found) = self.find_element_by_id(*node_id, id) {
                return Some(found);
            }
        }

        None
    }

    pub(crate) fn find_element_by_id(&self, node_id: u64, id: &str) -> Option<u64> {
        let element = self.elements_by_node_id.get(&node_id)?;

        if !element.connected {
            return None;
        }

        if element.id == id {
            return Some(node_id);
        }

        for child_node_id in &element.children {
            if let Some(found) = self.find_element_by_id(*child_node_id, id) {
                return Some(found);
            }
        }

        None
    }

    pub(crate) fn get_elements_by_tag_name(&self, tag_name: &str) -> Vec<u64> {
        let tag_name = tag_name.to_ascii_lowercase();
        let mut result = Vec::new();

        for node_id in &self.root_nodes {
            self.collect_elements_by_tag_name(*node_id, &tag_name, &mut result);
        }

        result
    }

    pub(crate) fn collect_elements_by_tag_name(
        &self,
        node_id: u64,
        tag_name: &str,
        result: &mut Vec<u64>,
    ) {
        let Some(element) = self.elements_by_node_id.get(&node_id) else {
            return;
        };

        if !element.connected {
            return;
        }

        if tag_name == "*" || element.tag_name == tag_name {
            result.push(node_id);
        }

        for child_node_id in &element.children {
            self.collect_elements_by_tag_name(*child_node_id, tag_name, result);
        }
    }

    pub(crate) fn element(&self, node_id: u64) -> Option<&ElementState> {
        self.elements_by_node_id.get(&node_id)
    }

    pub(crate) fn set_subtree_connected(&mut self, node_id: u64, connected: bool) {
        let Some(element) = self.elements_by_node_id.get_mut(&node_id) else {
            return;
        };

        element.connected = connected;
        let children = element.children.clone();

        for child_node_id in children {
            self.set_subtree_connected(child_node_id, connected);
        }
    }

    pub(crate) fn set_node_text_content(&mut self, node_id: u64, text_content: String) -> bool {
        let Some(element) = self.elements_by_node_id.get_mut(&node_id) else {
            return false;
        };

        element.text_content = text_content;
        element.text_content_was_set = true;
        true
    }

    pub(crate) fn set_node_attribute(&mut self, node_id: u64, name: &str, value: String) -> bool {
        let name = name.to_ascii_lowercase();
        let is_id = name == "id";

        let Some(element) = self.elements_by_node_id.get_mut(&node_id) else {
            return false;
        };

        if let Some(attribute) = element
            .attributes
            .iter_mut()
            .find(|attribute| attribute.name == name)
        {
            attribute.value = value.clone();
        } else {
            element.attributes.push(AttributeState {
                name,
                value: value.clone(),
            });
        }

        if is_id {
            element.id = value.clone();
        }

        true
    }

    pub(crate) fn remove_node_attribute(&mut self, node_id: u64, name: &str) -> bool {
        let name = name.to_ascii_lowercase();

        let Some(element) = self.elements_by_node_id.get_mut(&node_id) else {
            return false;
        };

        element
            .attributes
            .retain(|attribute| attribute.name != name);

        if name == "id" {
            element.id.clear();
        }

        true
    }

    pub(crate) fn append_child(&mut self, parent_node_id: u64, child_node_id: u64) -> bool {
        self.insert_child(parent_node_id, child_node_id, None)
    }

    pub(crate) fn insert_before(
        &mut self,
        parent_node_id: u64,
        child_node_id: u64,
        reference_node_id: u64,
    ) -> bool {
        let Some(parent) = self.elements_by_node_id.get(&parent_node_id) else {
            return false;
        };

        let Some(index) = parent
            .children
            .iter()
            .position(|node_id| *node_id == reference_node_id)
        else {
            return false;
        };

        self.insert_child(parent_node_id, child_node_id, Some(index))
    }

    pub(crate) fn insert_child(
        &mut self,
        parent_node_id: u64,
        child_node_id: u64,
        index: Option<usize>,
    ) -> bool {
        if parent_node_id == child_node_id {
            return false;
        }

        let Some(parent) = self.elements_by_node_id.get(&parent_node_id) else {
            return false;
        };

        let parent_connected = parent.connected;

        let Some(child) = self.elements_by_node_id.get(&child_node_id) else {
            return false;
        };

        let old_parent = child.parent;

        if let Some(old_parent_node_id) = old_parent {
            if let Some(old_parent) = self.elements_by_node_id.get_mut(&old_parent_node_id) {
                old_parent
                    .children
                    .retain(|node_id| *node_id != child_node_id);
            }
        } else {
            self.root_nodes.retain(|node_id| *node_id != child_node_id);
        }

        let Some(child) = self.elements_by_node_id.get_mut(&child_node_id) else {
            return false;
        };

        child.parent = Some(parent_node_id);

        let Some(parent) = self.elements_by_node_id.get_mut(&parent_node_id) else {
            return false;
        };

        let index = index
            .unwrap_or(parent.children.len())
            .min(parent.children.len());
        parent.children.insert(index, child_node_id);

        self.set_subtree_connected(child_node_id, parent_connected);

        true
    }

    pub(crate) fn remove_child(&mut self, parent_node_id: u64, child_node_id: u64) -> bool {
        let Some(child) = self.elements_by_node_id.get(&child_node_id) else {
            return false;
        };

        if child.parent != Some(parent_node_id) {
            return false;
        }

        let Some(parent) = self.elements_by_node_id.get_mut(&parent_node_id) else {
            return false;
        };

        parent.children.retain(|node_id| *node_id != child_node_id);

        let Some(child) = self.elements_by_node_id.get_mut(&child_node_id) else {
            return false;
        };

        child.parent = None;

        self.set_subtree_connected(child_node_id, false);

        true
    }

    pub(crate) fn set_node_inner_html(&mut self, node_id: u64, html: &str) -> bool {
        let fragment = HtmlDocument::parse(html);
        let fragment_text = fragment.text_content();

        let (previous_children, connected) = {
            let Some(element) = self.elements_by_node_id.get_mut(&node_id) else {
                return false;
            };

            element.text_content = fragment_text;
            element.text_content_was_set = false;

            (std::mem::take(&mut element.children), element.connected)
        };

        for child_node_id in previous_children {
            if let Some(child) = self.elements_by_node_id.get_mut(&child_node_id) {
                child.parent = None;
            }

            self.set_subtree_connected(child_node_id, false);
        }

        for node in &fragment.children {
            let _ = self.import_html_node(node, Some(node_id), connected);
        }

        true
    }

    pub(crate) fn set_node_outer_html(&mut self, node_id: u64, html: &str) -> bool {
        let fragment = HtmlDocument::parse(html);

        let Some(element) = self.elements_by_node_id.get(&node_id) else {
            return false;
        };

        let parent = element.parent;
        let connected = element.connected;

        let index = if let Some(parent_node_id) = parent {
            let Some(parent_element) = self.elements_by_node_id.get(&parent_node_id) else {
                return false;
            };

            let Some(index) = parent_element
                .children
                .iter()
                .position(|child_node_id| *child_node_id == node_id)
            else {
                return false;
            };

            index
        } else if let Some(index) = self
            .root_nodes
            .iter()
            .position(|root_node_id| *root_node_id == node_id)
        {
            index
        } else {
            return true;
        };

        if let Some(parent_node_id) = parent {
            let parent_element = self
                .elements_by_node_id
                .get_mut(&parent_node_id)
                .expect("parent was checked above");

            parent_element.children.remove(index);
        } else {
            self.root_nodes.remove(index);
        }

        if let Some(element) = self.elements_by_node_id.get_mut(&node_id) {
            element.parent = None;
        }

        self.set_subtree_connected(node_id, false);

        let mut inserted = 0;

        for node in &fragment.children {
            let Some(imported_node_id) = self.import_html_node(node, parent, connected) else {
                continue;
            };

            if let Some(parent_node_id) = parent {
                let parent_element = self
                    .elements_by_node_id
                    .get_mut(&parent_node_id)
                    .expect("parent was checked above");

                parent_element
                    .children
                    .retain(|child_node_id| *child_node_id != imported_node_id);

                parent_element
                    .children
                    .insert(index + inserted, imported_node_id);
            } else {
                self.root_nodes.insert(index + inserted, imported_node_id);
            }

            inserted += 1;
        }

        true
    }

    pub(crate) fn import_html_node(
        &mut self,
        node: &HtmlNode,
        parent: Option<u64>,
        connected: bool,
    ) -> Option<u64> {
        let HtmlNode::Element(element) = node else {
            return None;
        };

        let id = element
            .attributes
            .iter()
            .find(|attribute| attribute.name == "id")
            .map(|attribute| attribute.value.as_str())
            .unwrap_or("");

        let node_id = self.insert_element(
            id,
            &element.name,
            html_node_text_content(node),
            parent,
            connected,
            false,
        );

        for attribute in &element.attributes {
            self.set_node_attribute(node_id, &attribute.name, attribute.value.clone());
        }

        for child in &element.children {
            let _ = self.import_html_node(child, Some(node_id), connected);
        }

        Some(node_id)
    }

    pub(crate) fn previous_element_sibling(&self, node_id: u64) -> Option<u64> {
        let parent_node_id = self.element(node_id)?.parent?;
        let siblings = &self.element(parent_node_id)?.children;
        let index = siblings.iter().position(|child| *child == node_id)?;

        index
            .checked_sub(1)
            .and_then(|previous| siblings.get(previous))
            .copied()
    }

    pub(crate) fn next_element_sibling(&self, node_id: u64) -> Option<u64> {
        let parent_node_id = self.element(node_id)?.parent?;
        let siblings = &self.element(parent_node_id)?.children;
        let index = siblings.iter().position(|child| *child == node_id)?;

        siblings.get(index + 1).copied()
    }

    pub(crate) fn query_selector(&self, selector: &[SimpleSelector]) -> Option<u64> {
        for node_id in &self.root_nodes {
            if let Some(found) = self.find_matching_element(*node_id, selector) {
                return Some(found);
            }
        }

        None
    }

    pub(crate) fn query_selector_within(
        &self,
        node_id: u64,
        selector: &[SimpleSelector],
    ) -> Option<u64> {
        let element = self.element(node_id)?;

        if !element.connected {
            return None;
        }

        for child_node_id in &element.children {
            if let Some(found) = self.find_matching_element(*child_node_id, selector) {
                return Some(found);
            }
        }

        None
    }

    pub(crate) fn query_selector_all(&self, selector: &[SimpleSelector]) -> Vec<u64> {
        let mut result = Vec::new();

        for node_id in &self.root_nodes {
            self.collect_matching_elements(*node_id, selector, &mut result);
        }

        result
    }

    pub(crate) fn query_selector_all_within(
        &self,
        node_id: u64,
        selector: &[SimpleSelector],
    ) -> Vec<u64> {
        let Some(element) = self.element(node_id) else {
            return Vec::new();
        };

        let mut result = Vec::new();

        for child_node_id in &element.children {
            self.collect_matching_elements(*child_node_id, selector, &mut result);
        }

        result
    }

    pub(crate) fn collect_matching_elements(
        &self,
        node_id: u64,
        selectors: &[SimpleSelector],
        result: &mut Vec<u64>,
    ) {
        let Some(element) = self.element(node_id) else {
            return;
        };

        if !element.connected {
            return;
        }

        if self.node_matches_selector(node_id, selectors) {
            result.push(node_id);
        }

        for child_node_id in &element.children {
            self.collect_matching_elements(*child_node_id, selectors, result);
        }
    }

    pub(crate) fn find_matching_element(
        &self,
        node_id: u64,
        selectors: &[SimpleSelector],
    ) -> Option<u64> {
        let element = self.elements_by_node_id.get(&node_id)?;

        if !element.connected {
            return None;
        }

        if self.node_matches_selector(node_id, selectors) {
            return Some(node_id);
        }

        for child_node_id in &element.children {
            if let Some(found) = self.find_matching_element(*child_node_id, selectors) {
                return Some(found);
            }
        }

        None
    }

    pub(crate) fn node_matches_selector(&self, node_id: u64, selectors: &[SimpleSelector]) -> bool {
        let Some(last_selector) = selectors.last() else {
            return false;
        };

        let Some(element) = self.element(node_id) else {
            return false;
        };

        if !element.connected || !element_matches_selector(element, last_selector) {
            return false;
        }

        let mut ancestor = element.parent;

        for required_selector in selectors[..selectors.len() - 1].iter().rev() {
            let mut found = false;

            while let Some(ancestor_node_id) = ancestor {
                let Some(ancestor_element) = self.element(ancestor_node_id) else {
                    return false;
                };

                ancestor = ancestor_element.parent;

                if ancestor_element.connected
                    && element_matches_selector(ancestor_element, required_selector)
                {
                    found = true;
                    break;
                }
            }

            if !found {
                return false;
            }
        }

        true
    }

    pub fn set_element_text_content(&mut self, id: &str, text_content: String) -> bool {
        let Some(node_id) = self.get_element_by_id(id) else {
            return false;
        };

        self.set_node_text_content(node_id, text_content)
    }

    pub fn text_content_override(&self, id: &str) -> Option<&str> {
        let node_id = self.get_element_by_id(id)?;
        let element = self.element(node_id)?;

        if element.text_content_was_set {
            Some(element.text_content.as_str())
        } else {
            None
        }
    }
}

pub(crate) fn parse_simple_selector(input: &str) -> Result<Vec<SimpleSelector>, JsError> {
    let selectors = input
        .split_ascii_whitespace()
        .map(parse_compound_selector)
        .collect::<Result<Vec<_>, _>>()?;

    if selectors.is_empty() {
        return Err(JsError::runtime("SyntaxError: selector must not be empty"));
    }

    Ok(selectors)
}

fn parse_compound_selector(input: &str) -> Result<SimpleSelector, JsError> {
    let bytes = input.as_bytes();

    if bytes.is_empty() {
        return Err(JsError::runtime("SyntaxError: unsupported selector"));
    }

    let mut cursor = 0;
    let mut selector = SimpleSelector {
        tag_name: None,
        id: None,
        classes: Vec::new(),
    };

    if bytes[cursor] == b'*' {
        cursor += 1;
    } else if bytes[cursor] != b'#' && bytes[cursor] != b'.' {
        let name = take_selector_identifier(input, &mut cursor)?;
        selector.tag_name = Some(name.to_ascii_lowercase());
    }

    while cursor < input.len() {
        let marker = bytes[cursor];

        if marker != b'#' && marker != b'.' {
            return Err(JsError::runtime("SyntaxError: unsupported selector"));
        }

        cursor += 1;
        let name = take_selector_identifier(input, &mut cursor)?;

        match marker {
            b'#' if selector.id.is_none() => selector.id = Some(name.to_owned()),
            b'.' => selector.classes.push(name.to_owned()),
            _ => return Err(JsError::runtime("SyntaxError: unsupported selector")),
        }
    }

    if selector.tag_name.is_none() && selector.id.is_none() && selector.classes.is_empty() {
        return Err(JsError::runtime("SyntaxError: unsupported selector"));
    }

    Ok(selector)
}

fn take_selector_identifier<'a>(input: &'a str, cursor: &mut usize) -> Result<&'a str, JsError> {
    let start = *cursor;
    let bytes = input.as_bytes();

    while *cursor < input.len() {
        let byte = bytes[*cursor];

        if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' {
            *cursor += 1;
        } else {
            break;
        }
    }

    if start == *cursor {
        return Err(JsError::runtime("SyntaxError: unsupported selector"));
    }

    Ok(&input[start..*cursor])
}

fn element_matches_selector(element: &ElementState, selector: &SimpleSelector) -> bool {
    if let Some(tag_name) = &selector.tag_name {
        if element.tag_name != *tag_name {
            return false;
        }
    }

    if let Some(id) = &selector.id {
        if element.id != *id {
            return false;
        }
    }

    let class_attribute = element
        .attributes
        .iter()
        .find(|attribute| attribute.name == "class");

    selector.classes.iter().all(|required_class| {
        class_attribute.is_some_and(|attribute| {
            attribute
                .value
                .split_ascii_whitespace()
                .any(|class| class == required_class)
        })
    })
}

fn html_node_text_content(node: &HtmlNode) -> String {
    match node {
        HtmlNode::Text(text) => text.clone(),

        HtmlNode::Element(element) => element
            .children
            .iter()
            .map(html_node_text_content)
            .collect(),

        HtmlNode::Doctype(_) | HtmlNode::Comment(_) => String::new(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SimpleSelector {
    tag_name: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
}
