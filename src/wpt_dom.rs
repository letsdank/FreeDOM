use crate::{
    html::{Document, Element, Node},
    html_attribute,
    javascript::{DocumentState, JsError, evaluate_with_document},
};

pub fn run_document_test(source: &str) -> Result<DocumentState, JsError> {
    let document = Document::parse(source);

    let mut document_state = DocumentState::with_title(find_title(&document).unwrap_or_default());

    register_elements(&document, &mut document_state);
    execute_inline_scripts(&document, &mut document_state)?;

    Ok(document_state)
}

fn find_title(document: &Document) -> Option<String> {
    document.children.iter().find_map(find_title_in_node)
}

fn find_title_in_node(node: &Node) -> Option<String> {
    let Node::Element(element) = node else {
        return None;
    };

    if element.name == "title" {
        return Some(text_content(node).trim().to_owned());
    }

    element.children.iter().find_map(find_title_in_node)
}

fn register_elements(root: &Document, document_state: &mut DocumentState) {
    for child in &root.children {
        register_elements_in_node(child, document_state);
    }
}

fn register_elements_in_node(node: &Node, document_state: &mut DocumentState) {
    let Node::Element(element) = node else {
        return;
    };

    if element.name == "body" {
        document_state.register_body(
            html_attribute(element, "id").unwrap_or(""),
            crate::text_content(node),
        );
    } else if let Some(id) = html_attribute(element, "id") {
        document_state.register_element(id, &element.name, crate::text_content(node));
    }

    for child in &element.children {
        register_elements_in_node(child, document_state);
    }
}

fn execute_inline_scripts(
    root: &Document,
    document_state: &mut DocumentState,
) -> Result<(), JsError> {
    for child in &root.children {
        execute_inline_scripts_in_node(child, document_state)?;
    }

    Ok(())
}

fn execute_inline_scripts_in_node(
    node: &Node,
    document_state: &mut DocumentState,
) -> Result<(), JsError> {
    let Node::Element(element) = node else {
        return Ok(());
    };

    if element.name == "script" {
        if attribute(element, "src").is_none() && is_classic_javascript(element) {
            let source = text_content(node);

            if !source.trim().is_empty() {
                evaluate_with_document(&source, document_state)?;
            }
        }

        return Ok(());
    }

    for child in &element.children {
        execute_inline_scripts_in_node(child, document_state)?;
    }

    Ok(())
}

fn is_classic_javascript(element: &Element) -> bool {
    let Some(kind) = attribute(element, "type") else {
        return true;
    };

    matches!(
        kind.trim().to_ascii_lowercase().as_str(),
        "" | "text/javascript"
            | "application/javascript"
            | "text/ecmascript"
            | "application/ecmascript"
    )
}

fn attribute<'a>(element: &'a Element, name: &str) -> Option<&'a str> {
    element
        .attributes
        .iter()
        .find(|attribute| attribute.name == name)
        .map(|attribute| attribute.value.as_str())
}

fn text_content(node: &Node) -> String {
    match node {
        Node::Text(text) => text.clone(),

        Node::Element(element) => element.children.iter().map(text_content).collect(),

        Node::Doctype(_) | Node::Comment(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::run_document_test;

    #[test]
    fn runs_a_synchronous_wpt_shared_html_test() {
        let state = run_document_test(
            r#"
                <!doctype html>
                <title>Before script execution</title>

                <script src="/resources/testharness.js"></script>
                <script src="/resources/testharnessreport.js"></script>

                <h1 id="message">Before</h1>

                <script>
                    test(function(t) {
                        const message = document.getElementById("message");

                        assert_equals(t === t, true);
                        assert_equals(message.tagName, "H1");
                        assert_equals(message.textContent, "Before");

                        message.textContent = "After";
                        assert_equals(message.textContent, "After");

                        document.title = message.textContent;
                    }, "DOM state is available to a synchronous test");
                </script>
            "#,
        )
        .unwrap();

        assert_eq!(state.title, "After");
        assert_eq!(state.text_content_override("message"), Some("After"));
    }

    #[test]
    fn reports_an_assertion_failure_to_rust_tests() {
        let error = run_document_test(
            r#"
                <script src="/resources/testharness.js"></script>
                <script>
                    test(() => {
                        assert_equals(1 + 1, 3, "arithmetic must be correct");
                    }, "failing case");
                </script>
            "#,
        )
        .unwrap_err();

        assert!(error.message.contains("AssertionError"));
        assert!(error.message.contains("arithmetic must be correct"));
    }

    #[test]
    fn official_wpt_document_get_element_by_id_empty_string() {
        let state = run_document_test(
            r#"
                <!DOCTYPE html>
                <meta charset=utf-8>
                <title>Document.getElementById</title>

                <script src="/resources/testharness.js"></script>
                <script src="/resources/testharnessreport.js"></script>

                <body>
                    <div id="log"></div>

                    <!-- test 0 from dom/nodes/Document-getElementById.html -->
                    <div id=""></div>

                    <script>
                        test(function() {
                            assert_equals(document.getElementById(""), null);
                        }, "Calling document.getElementById with an empty string argument.");
                    </script>
                </body>
            "#,
        )
        .unwrap();

        assert_eq!(state.title, "Document.getElementById");
    }

    #[test]
    fn official_wpt_document_get_element_by_id_static_element() {
        let state = run_document_test(
            r#"
                <!DOCTYPE html>
                <meta charset=utf-8>
                <title>Document.getElementById</title>

                <script src="/resources/testharness.js"></script>
                <script src="/resources/testharnessreport.js"></script>

                <body>
                    <!-- test 1 from dom/nodes/Document-getElementById.html -->
                    <div id="test1"></div>

                    <script>
                        test(function() {
                            var bar = document.getElementById("test1");

                            assert_not_equals(bar, null, "should not be null");
                            assert_equals(
                                bar.tagName,
                                "DIV",
                                "should have expected tag name."
                            );
                            assert_true(
                                bar instanceof HTMLDivElement,
                                "should be a valid Element instance"
                            );
                        }, "on static page");
                    </script>
                </body>
            "#,
        )
        .unwrap();

        assert_eq!(state.title, "Document.getElementById");
    }
}
