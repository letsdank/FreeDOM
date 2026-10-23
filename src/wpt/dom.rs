use crate::{
    html::{Document, Element, Node},
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
        register_elements_in_node(child, None, document_state);
    }
}

fn register_elements_in_node(
    node: &Node,
    parent_node_id: Option<u64>,
    document_state: &mut DocumentState,
) {
    let Node::Element(element) = node else {
        return;
    };

    let node_id = document_state.register_parsed_element(
        &element.name,
        attribute(element, "id").unwrap_or(""),
        text_content(node),
        parent_node_id,
    );

    for attribute in &element.attributes {
        document_state.register_element_attribute(
            node_id,
            &attribute.name,
            attribute.value.clone(),
        );
    }

    for child in &element.children {
        register_elements_in_node(child, Some(node_id), document_state);
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

    #[test]
    fn official_wpt_document_get_element_by_id_null_and_undefined() {
        let state = run_document_test(
            r#"
                <!DOCTYPE html>
                <meta charset=utf-8>
                <title>Document.getElementById</title>

                <script src="/resources/testharness.js"></script>
                <script src="/resources/testharnessreport.js"></script>

                <body>
                    <div id="log"></div>

                    <script>
                        test(function() {
                            var foo = document.createElement("div");
                            foo.setAttribute("id", "null");
                            document.body.appendChild(foo);

                            this.add_cleanup(function() {
                                document.body.removeChild(foo);
                            });

                            var bar = document.getElementById(null);
                            assert_equals(bar, foo);
                        }, "Calling document.getElementById with a null argument.");
                    </script>

                    <script>
                        test(function() {
                            var foo = document.createElement("div");
                            foo.setAttribute("id", "undefined");
                            document.body.appendChild(foo);

                            this.add_cleanup(function() {
                                document.body.removeChild(foo);
                            });

                            var bar = document.getElementById(undefined);
                            assert_equals(bar, foo);
                        }, "Calling document.getElementById with an undefined argument.");
                    </script>
                </body>
            "#,
        )
        .unwrap();

        assert_eq!(state.title, "Document.getElementById");
    }

    #[test]
    fn official_wpt_document_get_element_by_id_script_inserted_element() {
        let state = run_document_test(
            r#"
                <!DOCTYPE html>
                <meta charset=utf-8>
                <title>Document.getElementById</title>

                <script src="/resources/testharness.js"></script>
                <script src="/resources/testharnessreport.js"></script>

                <body>
                    <script>
                        var gBody = document.getElementsByTagName("body")[0];

                        test(function() {
                            var TEST_ID = "test2";

                            var test = document.createElement("div");
                            test.setAttribute("id", TEST_ID);
                            gBody.appendChild(test);

                            var result = document.getElementById(TEST_ID);
                            assert_not_equals(result, null, "should not be null.");
                            assert_equals(
                                result.tagName,
                                "DIV",
                                "should have appended element's tag name"
                            );
                            assert_true(
                                result instanceof HTMLDivElement,
                                "should be a valid Element instance"
                            );

                            gBody.removeChild(test);

                            var removed = document.getElementById(TEST_ID);
                            assert_equals(
                                removed,
                                null,
                                "should not get removed element."
                            );
                        }, "Document.getElementById with a script-inserted element");
                    </script>
                </body>
            "#,
        )
        .unwrap();

        assert_eq!(state.title, "Document.getElementById");
    }

    #[test]
    fn official_wpt_document_get_element_by_id_after_id_changes() {
        let state = run_document_test(
            r#"
                <!DOCTYPE html>
                <meta charset=utf-8>
                <title>Document.getElementById</title>

                <script src="/resources/testharness.js"></script>
                <script src="/resources/testharnessreport.js"></script>

                <body>
                    <script>
                        var gBody = document.getElementsByTagName("body")[0];

                        test(function() {
                            var TEST_ID = "test3";
                            var test = document.createElement("div");

                            test.setAttribute("id", TEST_ID);
                            gBody.appendChild(test);

                            var UPDATED_ID = "test3-updated";
                            test.setAttribute("id", UPDATED_ID);

                            var e = document.getElementById(UPDATED_ID);
                            assert_equals(e, test, "should get the element with id.");

                            var old = document.getElementById(TEST_ID);
                            assert_equals(
                                old,
                                null,
                                "shouldn't get the element by the old id."
                            );

                            test.removeAttribute("id");

                            var e2 = document.getElementById(UPDATED_ID);
                            assert_equals(
                                e2,
                                null,
                                "should return null when the passed id is none in document."
                            );
                        }, "update `id` attribute via setAttribute/removeAttribute");
                    </script>
                </body>
            "#,
        )
        .unwrap();

        assert_eq!(state.title, "Document.getElementById");
    }

    #[test]
    fn official_wpt_document_get_element_by_id_ignores_detached_elements() {
        let state = run_document_test(
            r#"
                <!DOCTYPE html>
                <meta charset=utf-8>
                <title>Document.getElementById</title>

                <script src="/resources/testharness.js"></script>
                <script src="/resources/testharnessreport.js"></script>

                <body>
                    <script>
                        test(function() {
                            var TEST_ID = "test4-should-not-exist";

                            var e = document.createElement("div");
                            e.setAttribute("id", TEST_ID);

                            assert_equals(
                                document.getElementById(TEST_ID),
                                null,
                                "should be null"
                            );

                            document.body.appendChild(e);

                            assert_equals(
                                document.getElementById(TEST_ID),
                                e,
                                "should be the appended element"
                            );
                        }, "Ensure that the id attribute only affects elements present in a document");
                    </script>
                </body>
            "#,
        )
            .unwrap();

        assert_eq!(state.title, "Document.getElementById");
    }

    #[test]
    fn official_wpt_document_get_element_by_id_tree_order() {
        let _state = run_document_test(
            r#"
                <!DOCTYPE html>
                <meta charset=utf-8>
                <title>Document.getElementById</title>

                <script src="/resources/testharness.js"></script>
                <script src="/resources/testharnessreport.js"></script>

                <body>
                    <div id="test5" data-name="1st">
                        <p id="test5" data-name="2nd">P</p>
                        <input id="test5" type="submit" value="Submit" data-name="3rd">
                    </div>

                    <script>
                        var gBody = document.getElementsByTagName("body")[0];

                        test(function() {
                            var TEST_ID = "test5";

                            var target = document.getElementById(TEST_ID);
                            assert_not_equals(target, null, "should not be null");
                            assert_equals(
                                target.getAttribute("data-name"),
                                "1st",
                                "should return the 1st"
                            );

                            var element4 = document.createElement("div");
                            element4.setAttribute("id", TEST_ID);
                            element4.setAttribute("data-name", "4th");
                            gBody.appendChild(element4);

                            var target2 = document.getElementById(TEST_ID);
                            assert_not_equals(target2, null, "should not be null");
                            assert_equals(
                                target2.getAttribute("data-name"),
                                "1st",
                                "should be the 1st"
                            );

                            target2.parentNode.removeChild(target2);

                            var target3 = document.getElementById(TEST_ID);
                            assert_not_equals(target3, null, "should not be null");
                            assert_equals(
                                target3.getAttribute("data-name"),
                                "4th",
                                "should be the 4th"
                            );
                        }, "in tree order, within the context object's tree");
                    </script>
            "#,
        )
        .unwrap();
    }

    #[test]
    fn official_wpt_document_get_element_by_id_ignores_detached_subtrees() {
        let state = run_document_test(
            r#"
                <!DOCTYPE html>
                <meta charset=utf-8>
                <title>Document.getElementById</title>

                <script src="/resources/testharness.js"></script>
                <script src="/resources/testharnessreport.js"></script>

                <body>
                    <script>
                        test(function() {
                            var TEST_ID = "test6";
                            var s = document.createElement("div");

                            s.setAttribute("id", TEST_ID);

                            document.createElement("div").appendChild(s);

                            assert_equals(
                                document.getElementById(TEST_ID),
                                null,
                                "should be null"
                            );
                        }, "Appending to a detached node must not expose its descendants");
                    </script>
                </body>
            "#,
        )
        .unwrap();

        assert_eq!(state.title, "Document.getElementById");
    }

    #[test]
    fn official_wpt_document_get_element_by_id_after_id_property_changes() {
        let state = run_document_test(
            r#"
                <!DOCTYPE html>
                <meta charset=utf-8>
                <title>Document.getElementById</title>

                <script src="/resources/testharness.js"></script>
                <script src="/resources/testharnessreport.js"></script>

                <body>
                    <script>
                        var gBody = document.getElementsByTagName("body")[0];

                        test(function() {
                            var TEST_ID = "test12";
                            var test = document.createElement("div");

                            test.id = TEST_ID;
                            gBody.appendChild(test);

                            var UPDATED_ID = TEST_ID + "-updated";
                            test.id = UPDATED_ID;

                            var e = document.getElementById(UPDATED_ID);
                            assert_equals(e, test, "should get the element with id.");

                            var old = document.getElementById(TEST_ID);
                            assert_equals(
                                old,
                                null,
                                "shouldn't get the element by the old id."
                            );

                            test.id = "";

                            var e2 = document.getElementById(TEST_ID);
                            assert_equals(
                                e2,
                                null,
                                "should return null when the passed id is none in document."
                            );
                        }, "update `id` attribute via element.id");
                    </script>
                </body>
            "#,
        )
        .unwrap();

        assert_eq!(state.title, "Document.getElementById");
    }

    #[test]
    fn official_wpt_document_get_element_by_id_nested_connectivity() {
        let state = run_document_test(
            r#"
                <!DOCTYPE html>
                <meta charset=utf-8>
                <title>Document.getElementById</title>

                <script src="/resources/testharness.js"></script>
                <script src="/resources/testharnessreport.js"></script>

                <body>
                    <div id="outer">
                        <div id="middle">
                            <div id="inner"></div>
                        </div>
                    </div>

                    <script>
                        var gBody = document.getElementsByTagName("body")[0];

                        test(function() {
                            var TEST_ID = "test14";
                            var a = document.createElement("a");
                            var b = document.createElement("b");

                            a.appendChild(b);
                            b.id = TEST_ID;

                            assert_equals(document.getElementById(TEST_ID), null);

                            gBody.appendChild(a);

                            assert_equals(document.getElementById(TEST_ID), b);
                        }, "Inserting an id by inserting its parent node");

                        test(function() {
                            var outer = document.getElementById("outer");
                            var middle = document.getElementById("middle");
                            var inner = document.getElementById("inner");

                            outer.removeChild(middle);

                            var new_el = document.createElement("h1");
                            new_el.id = "heading";
                            inner.appendChild(new_el);

                            assert_equals(
                                document.getElementById("heading"),
                                null
                            );
                        }, "Document.getElementById must not return nodes not present in document");
                    </script>
                </body>
            "#,
        )
        .unwrap();

        assert_eq!(state.title, "Document.getElementById");
    }

    #[test]
    fn official_wpt_document_get_element_by_id_tree_order_after_insert_before() {
        let state = run_document_test(
            r#"
                <!DOCTYPE html>
                <meta charset=utf-8>
                <title>Document.getElementById</title>

                <script src="/resources/testharness.js"></script>
                <script src="/resources/testharnessreport.js"></script>

                <body>
                    <script>
                        var gBody = document.getElementsByTagName("body")[0];

                        test(function() {
                            var TEST_ID = "test13";

                            var create_same_id_element = function(order) {
                                var element = document.createElement("div");
                                element.setAttribute("id", TEST_ID);
                                element.setAttribute("data-order", order);
                                return element;
                            };

                            var container = document.createElement("div");
                            container.setAttribute("id", TEST_ID + "-fixture");
                            gBody.appendChild(container);

                            var element1 = create_same_id_element("1");
                            var element2 = create_same_id_element("2");
                            var element3 = create_same_id_element("3");
                            var element4 = create_same_id_element("4");

                            container.appendChild(element2);
                            container.appendChild(element4);
                            container.insertBefore(element3, element4);
                            container.insertBefore(element1, element2);

                            var test = document.getElementById(TEST_ID);
                            assert_equals(test, element1, "should return 1st element");

                            container.removeChild(element1);
                            test = document.getElementById(TEST_ID);
                            assert_equals(test, element2, "should return 2nd element");

                            container.removeChild(element2);
                            test = document.getElementById(TEST_ID);
                            assert_equals(test, element3, "should return 3rd element");

                            container.removeChild(element3);
                            test = document.getElementById(TEST_ID);
                            assert_equals(test, element4, "should return 4th element");

                            container.removeChild(element4);
                        }, "where insertion order and tree order don't match");
                    </script>
                </body>
            "#,
        )
        .unwrap();

        assert_eq!(state.title, "Document.getElementById");
    }

    #[test]
    fn official_wpt_document_get_element_by_id_after_inner_html_changes() {
        let state = run_document_test(
            r#"
                <!DOCTYPE html>
                <title>Document.getElementById</title>

                <body>
                    <script>
                        var gBody = document.getElementsByTagName("body")[0];

                        test(function() {
                            var TEST_ID = "test8";
                            var test = document.createElement("div");

                            gBody.appendChild(test);
                            test.innerHTML = '<div id="' + TEST_ID + '"></div>';

                            assert_not_equals(
                                document.getElementById(TEST_ID),
                                null
                            );
                        }, "addition through innerHTML");

                        test(function() {
                            var TEST_ID = "test9";
                            var test = document.createElement("div");

                            test.innerHTML = '<div id="' + TEST_ID + '"></div>';
                            gBody.appendChild(test);

                            assert_not_equals(
                                document.getElementById(TEST_ID),
                                null
                            );

                            test.innerHTML = "";

                            assert_equals(
                                document.getElementById(TEST_ID),
                                null
                            );
                        }, "removal through innerHTML");
                    </script>
                </body>
            "#,
        )
        .unwrap();

        assert_eq!(state.title, "Document.getElementById");
    }
}
