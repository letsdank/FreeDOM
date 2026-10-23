use crate::javascript::{
    DocumentState, JsError, Value,
    dom::{ElementState, parse_simple_selector},
};

use super::Interpreter;

impl Interpreter {
    pub(super) fn current_document(&self) -> Option<&DocumentState> {
        let Value::Document(document) = self.variables.get("document")? else {
            return None;
        };

        Some(document)
    }

    pub(super) fn current_document_element(&self, node_id: u64) -> Option<&ElementState> {
        self.current_document()?.element(node_id)
    }

    pub(super) fn current_document_mut(&mut self) -> Result<&mut DocumentState, JsError> {
        let Some(Value::Document(document)) = self.variables.get_mut("document") else {
            return Err(JsError::runtime(
                "TypeError: document binding is unavailable",
            ));
        };

        Ok(document)
    }

    pub(super) fn set_document_element_text_content(
        &mut self,
        node_id: u64,
        text_content: String,
    ) -> Result<(), JsError> {
        if self
            .current_document_mut()?
            .set_node_text_content(node_id, text_content)
        {
            Ok(())
        } else {
            Err(JsError::runtime(
                "TypeError: element is no longer available",
            ))
        }
    }

    pub(super) fn set_document_element_inner_html(
        &mut self,
        node_id: u64,
        html: String,
    ) -> Result<(), JsError> {
        if self
            .current_document_mut()?
            .set_node_inner_html(node_id, &html)
        {
            Ok(())
        } else {
            Err(JsError::runtime(
                "TypeError: element is no longer available",
            ))
        }
    }

    pub(super) fn set_document_element_outer_html(
        &mut self,
        node_id: u64,
        html: String,
    ) -> Result<(), JsError> {
        if self
            .current_document_mut()?
            .set_node_outer_html(node_id, &html)
        {
            Ok(())
        } else {
            Err(JsError::runtime(
                "TypeError: element is no longer available",
            ))
        }
    }

    pub(super) fn set_document_element_id(
        &mut self,
        node_id: u64,
        id: String,
    ) -> Result<(), JsError> {
        if self
            .current_document_mut()?
            .set_node_attribute(node_id, "id", id)
        {
            Ok(())
        } else {
            Err(JsError::runtime(
                "TypeError: element is no longer available",
            ))
        }
    }

    pub(super) fn get_element_by_id(
        &mut self,
        document: DocumentState,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if arguments.len() != 1 {
            return Err(JsError::runtime(
                "TypeError: document.getElementById expects one argument",
            ));
        }

        let id = arguments[0].to_js_string();

        Ok(document
            .get_element_by_id(&id)
            .map(Value::Element)
            .unwrap_or(Value::Null))
    }

    pub(super) fn get_elements_by_tag_name(
        &mut self,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if arguments.len() != 1 {
            return Err(JsError::runtime(
                "TypeError: document.getElementsByTagName expects one argument",
            ));
        }

        Ok(Value::HtmlCollection(arguments[0].to_js_string()))
    }

    pub(super) fn query_selector(&mut self, arguments: Vec<Value>) -> Result<Value, JsError> {
        if arguments.len() != 1 {
            return Err(JsError::runtime(
                "TypeError: document.querySelector expects one argument",
            ));
        }

        let selector = parse_simple_selector(&arguments[0].to_js_string())?;

        Ok(self
            .current_document()
            .and_then(|document| document.query_selector(&selector))
            .map(Value::Element)
            .unwrap_or(Value::Null))
    }

    pub(super) fn query_selector_within(
        &mut self,
        node_id: u64,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if arguments.len() != 1 {
            return Err(JsError::runtime(
                "TypeError: Element.querySelector expects one argument",
            ));
        }

        let selector = parse_simple_selector(&arguments[0].to_js_string())?;

        Ok(self
            .current_document()
            .and_then(|document| document.query_selector_within(node_id, &selector))
            .map(Value::Element)
            .unwrap_or(Value::Null))
    }

    pub(super) fn query_selector_all(&mut self, arguments: Vec<Value>) -> Result<Value, JsError> {
        if arguments.len() != 1 {
            return Err(JsError::runtime(
                "TypeError: document.querySelectorAll expects one argument",
            ));
        }

        let selector = parse_simple_selector(&arguments[0].to_js_string())?;

        let nodes = self
            .current_document()
            .map(|document| document.query_selector_all(&selector))
            .unwrap_or_default();

        Ok(Value::NodeList(nodes))
    }

    pub(super) fn query_selector_all_within(
        &mut self,
        node_id: u64,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if arguments.len() != 1 {
            return Err(JsError::runtime(
                "TypeError: Element.querySelectorAll expects one argument",
            ));
        }

        let selector = parse_simple_selector(&arguments[0].to_js_string())?;

        let nodes = self
            .current_document()
            .map(|document| document.query_selector_all_within(node_id, &selector))
            .unwrap_or_default();

        Ok(Value::NodeList(nodes))
    }

    pub(crate) fn class_list_tokens(&self, node_id: u64) -> Result<Vec<String>, JsError> {
        let Some(element) = self.current_document_element(node_id) else {
            return Err(JsError::runtime(
                "TypeError: element is no longer available",
            ));
        };

        Ok(element
            .attributes
            .iter()
            .find(|attribute| attribute.name == "class")
            .map(|attribute| {
                attribute
                    .value
                    .split_ascii_whitespace()
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default())
    }

    fn set_class_list_tokens(&mut self, node_id: u64, tokens: Vec<String>) -> Result<(), JsError> {
        if self
            .current_document_mut()?
            .set_node_attribute(node_id, "class", tokens.join(" "))
        {
            Ok(())
        } else {
            Err(JsError::runtime(
                "TypeError: element is no longer available",
            ))
        }
    }

    fn class_list_token(value: &Value) -> Result<String, JsError> {
        let token = value.to_js_string();

        if token.is_empty()
            || token
                .chars()
                .any(|character| character.is_ascii_whitespace())
        {
            return Err(JsError::runtime(
                "SyntaxError: class token must not be empty or contain whitespace",
            ));
        }

        Ok(token)
    }

    pub(super) fn class_list_contains(
        &self,
        node_id: u64,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if arguments.len() != 1 {
            return Err(JsError::runtime(
                "TypeError: DOMTokenList.contains expects one argument",
            ));
        }

        let token = Self::class_list_token(&arguments[0])?;
        Ok(Value::Boolean(
            self.class_list_tokens(node_id)?
                .iter()
                .any(|item| item == &token),
        ))
    }

    pub(super) fn class_list_add(
        &mut self,
        node_id: u64,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        let additions = arguments
            .iter()
            .map(Self::class_list_token)
            .collect::<Result<Vec<_>, _>>()?;

        let mut tokens = self.class_list_tokens(node_id)?;

        for token in additions {
            if !tokens.iter().any(|item| item == &token) {
                tokens.push(token);
            }
        }

        self.set_class_list_tokens(node_id, tokens)?;
        Ok(Value::Undefined)
    }

    pub(super) fn class_list_remove(
        &mut self,
        node_id: u64,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        let removals = arguments
            .iter()
            .map(Self::class_list_token)
            .collect::<Result<Vec<_>, _>>()?;

        let mut tokens = self.class_list_tokens(node_id)?;
        tokens.retain(|token| !removals.iter().any(|removal| removal == token));

        self.set_class_list_tokens(node_id, tokens)?;
        Ok(Value::Undefined)
    }

    pub(super) fn class_list_toggle(
        &mut self,
        node_id: u64,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if arguments.is_empty() || arguments.len() > 2 {
            return Err(JsError::runtime(
                "TypeError: DOMTokenList.toggle expects one or two arguments",
            ));
        }

        let token = Self::class_list_token(&arguments[0])?;
        let force = arguments.get(1).map(|value| self.is_truthy(value));

        let mut tokens = self.class_list_tokens(node_id)?;
        let present = tokens.iter().any(|item| item == &token);
        let should_be_present = force.unwrap_or(!present);

        if should_be_present && !present {
            tokens.push(token);
        } else if !should_be_present && present {
            tokens.retain(|item| item != &token);
        }

        self.set_class_list_tokens(node_id, tokens)?;
        Ok(Value::Boolean(should_be_present))
    }

    pub(super) fn create_document_element(
        &mut self,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if arguments.len() != 1 {
            return Err(JsError::runtime(
                "TypeError: document.createElement expects one argument",
            ));
        }

        let tag_name = arguments[0].to_js_string().to_ascii_lowercase();

        if tag_name.is_empty() {
            return Err(JsError::runtime(
                "TypeError: document.createElement requires a tag name",
            ));
        }

        let node_id = self.current_document_mut()?.create_element(&tag_name);

        Ok(Value::Element(node_id))
    }

    pub(super) fn get_document_element_attribute(
        &self,
        node_id: u64,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if arguments.len() != 1 {
            return Err(JsError::runtime(
                "TypeError: Element.getAttribute expects one argument",
            ));
        }

        let name = arguments[0].to_js_string().to_ascii_lowercase();

        let Some(element) = self.current_document_element(node_id) else {
            return Err(JsError::runtime(
                "TypeError: element is no longer available",
            ));
        };

        Ok(element
            .attributes
            .iter()
            .find(|attribute| attribute.name == name)
            .map(|attribute| Value::String(attribute.value.clone()))
            .unwrap_or(Value::Null))
    }

    pub(super) fn set_document_element_attribute(
        &mut self,
        node_id: u64,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if arguments.len() != 2 {
            return Err(JsError::runtime(
                "TypeError: Element.setAttribute expects two arguments",
            ));
        }

        let name = arguments[0].to_js_string().to_ascii_lowercase();
        let value = arguments[1].to_js_string();

        if self
            .current_document_mut()?
            .set_node_attribute(node_id, &name, value)
        {
            Ok(Value::Undefined)
        } else {
            Err(JsError::runtime(
                "TypeError: element is no longer available",
            ))
        }
    }

    pub(super) fn remove_document_element_attribute(
        &mut self,
        node_id: u64,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if arguments.len() != 1 {
            return Err(JsError::runtime(
                "TypeError: Element.removeAttribute expects one argument",
            ));
        }

        let name = arguments[0].to_js_string().to_ascii_lowercase();

        if self
            .current_document_mut()?
            .remove_node_attribute(node_id, &name)
        {
            Ok(Value::Undefined)
        } else {
            Err(JsError::runtime(
                "TypeError: element is no longer available",
            ))
        }
    }

    pub(super) fn append_document_child(
        &mut self,
        parent_node_id: u64,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if arguments.len() != 1 {
            return Err(JsError::runtime(
                "TypeError: Element.appendChild expects one argument",
            ));
        }

        let Value::Element(child_node_id) = arguments[0] else {
            return Err(JsError::runtime(
                "TypeError: Element.appendChild expects an Element",
            ));
        };

        if self
            .current_document_mut()?
            .append_child(parent_node_id, child_node_id)
        {
            Ok(Value::Element(child_node_id))
        } else {
            Err(JsError::runtime("TypeError: cannot append this element"))
        }
    }

    pub(super) fn insert_document_child_before(
        &mut self,
        parent_node_id: u64,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if arguments.len() != 2 {
            return Err(JsError::runtime(
                "TypeError: Element.insertBefore expects two arguments",
            ));
        }

        let Value::Element(child_node_id) = arguments[0] else {
            return Err(JsError::runtime(
                "TypeError: Element.insertBefore expects an Element to insert",
            ));
        };

        let Value::Element(reference_node_id) = arguments[1] else {
            return Err(JsError::runtime(
                "TypeError: Element.insertBefore expects an Element reference",
            ));
        };

        if self.current_document_mut()?.insert_before(
            parent_node_id,
            child_node_id,
            reference_node_id,
        ) {
            Ok(Value::Element(child_node_id))
        } else {
            Err(JsError::runtime(
                "TypeError: cannot insert this element before the reference node",
            ))
        }
    }

    pub(super) fn remove_document_child(
        &mut self,
        parent_node_id: u64,
        arguments: Vec<Value>,
    ) -> Result<Value, JsError> {
        if arguments.len() != 1 {
            return Err(JsError::runtime(
                "TypeError: Element.removeChild expects one argument",
            ));
        }

        let Value::Element(child_node_id) = arguments[0] else {
            return Err(JsError::runtime(
                "TypeError: Element.removeChild expects an Element",
            ));
        };

        if self
            .current_document_mut()?
            .remove_child(parent_node_id, child_node_id)
        {
            Ok(Value::Element(child_node_id))
        } else {
            Err(JsError::runtime("TypeError: cannot remove this element"))
        }
    }
}
