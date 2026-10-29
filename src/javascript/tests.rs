use super::{DocumentState, TokenKind, Value, evaluate, evaluate_with_document, tokenize};

#[test]
fn tokenizes_url_constructor_syntax() {
    let tokens = tokenize("const url = new URL(test.input, test.base);").unwrap();

    let kinds: Vec<TokenKind> = tokens.into_iter().map(|token| token.kind).collect();

    assert_eq!(
        kinds,
        vec![
            TokenKind::Const,
            TokenKind::Identifier("url".to_owned()),
            TokenKind::Assign,
            TokenKind::New,
            TokenKind::Identifier("URL".to_owned()),
            TokenKind::LeftParen,
            TokenKind::Identifier("test".to_owned()),
            TokenKind::Dot,
            TokenKind::Identifier("input".to_owned()),
            TokenKind::Comma,
            TokenKind::Identifier("test".to_owned()),
            TokenKind::Dot,
            TokenKind::Identifier("base".to_owned()),
            TokenKind::RightParen,
            TokenKind::Semicolon,
            TokenKind::EndOfFile,
        ]
    );
}

#[test]
fn skips_comments_and_decodes_escapes() {
    let tokens = tokenize(
        r#"
// WPT-style input
let base = "https:\/\/example.test\u002Fdocs";
/* ignored */
"#,
    )
    .unwrap();

    let kinds: Vec<TokenKind> = tokens.into_iter().map(|token| token.kind).collect();

    assert_eq!(
        kinds,
        vec![
            TokenKind::Let,
            TokenKind::Identifier("base".to_owned()),
            TokenKind::Assign,
            TokenKind::String("https://example.test/docs".to_owned()),
            TokenKind::Semicolon,
            TokenKind::EndOfFile,
        ]
    );
}

#[test]
fn rejects_unclosed_block_comments() {
    let error = tokenize("/* unfinished").unwrap_err();

    assert_eq!(error.message, "unterminated block comment");
}

#[test]
fn evaluates_url_constructor_and_href() {
    let result = evaluate(
        r##"
                const base = "http://example.org/foo/bar";
                const url = new URL("#", base);
                url.href;
            "##,
    )
    .unwrap();

    assert_eq!(
        result,
        Value::String("http://example.org/foo/bar#".to_owned())
    );
}

#[test]
fn runs_wpt_style_url_assertion() {
    let result = evaluate(
        r##"
                const base = "http://example.org/foo/bar";

                assert_equals(
                    new URL("#", base).href,
                    "http://example.org/foo/bar#",
                    "empty fragment must be preserved",
                );
            "##,
    )
    .unwrap();

    assert_eq!(result, Value::Undefined);
}

#[test]
fn changes_document_title_through_javascript() {
    let mut document = DocumentState::with_title("Before");

    let result = evaluate_with_document(
        r#"
                document.title = "FreeDOM";
                assert_equals(document.title, "FreeDOM");
            "#,
        &mut document,
    )
    .unwrap();

    assert_eq!(result, Value::Undefined);
    assert_eq!(document.title, "FreeDOM");
}

#[test]
fn finds_an_element_by_id_and_reads_its_properties() {
    let mut document = DocumentState::default();
    document.register_element("welcome", "h1", "Welcome home".to_owned());

    evaluate_with_document(
        r#"
                const heading = document.getElementById("welcome");

                assert_equals(heading.id, "welcome");
                assert_equals(heading.tagName, "H1");
                assert_equals(heading.textContent, "Welcome home");
                assert_equals(document.getElementById("missing"), null);
            "#,
        &mut document,
    )
    .unwrap();
}

#[test]
fn evaluates_arithmetic_comparisons_and_string_concatenation() {
    let result = evaluate(
        r#"
                assert_equals(1 + 2 * 3, 7);
                assert_equals("Free" + "DOM", "FreeDOM");
                assert_equals(10 / 2 - 1, 4);
                assert_equals(3 < 5, true);
                assert_equals(3 === 3, true);
                assert_equals(3 !== 4, true);

                2 + 2;
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Number(4.0));
}

#[test]
fn runs_synchronous_wpt_style_test_callbacks() {
    let result = evaluate(
        r#"
                test(() => {
                    assert_equals(1 + 2 * 3, 7);
                }, "arrow callback");

                test(function() {
                    assert_equals("Free" + "DOM", "FreeDOM");
                }, "function callback");
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Undefined);
}

#[test]
fn passes_test_context_to_wpt_style_callbacks() {
    let result = evaluate(
        r#"
                test(t => {
                    assert_equals(t === t, true);
                    assert_equals(2 + 2, 4);
                }, "single arrow parameter");

                test(function(t) {
                    assert_equals(t === t, true);
                    assert_equals("Free" + "DOM", "FreeDOM");
                }, "single function parameter");
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Undefined);
}

#[test]
fn runs_common_testharness_assertions() {
    let result = evaluate(
        r#"
                test(t => {
                    assert_true(t === t);
                    assert_false(2 < 1);
                    assert_not_equals("FreeDOM", "Chrome");
                }, "common synchronous assertions");
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Undefined);
}

#[test]
fn runs_wpt_cleanup_callbacks_with_this_binding() {
    let mut document = DocumentState::with_title("Before");

    evaluate_with_document(
        r#"
                test(function() {
                    assert_true(this === this);

                    this.add_cleanup(function() {
                        document.title = "After cleanup";
                    });

                    document.title = "During test";
                }, "cleanup callbacks run after the test body");

                assert_equals(document.title, "After cleanup");
            "#,
        &mut document,
    )
    .unwrap();

    assert_eq!(document.title, "After cleanup");
}

#[test]
fn preserves_document_changes_from_cleanups_after_a_failed_test() {
    let mut document = DocumentState::with_title("Before");

    let error = evaluate_with_document(
        r#"
                test(function() {
                    this.add_cleanup(function() {
                        document.title = "Cleaned after failure";
                    });

                    assert_equals(1, 2, "intentional failure");
                }, "cleanup on failure");
            "#,
        &mut document,
    )
    .unwrap_err();

    assert!(error.message.contains("intentional failure"));
    assert_eq!(document.title, "Cleaned after failure");
}

#[test]
fn creates_attaches_and_removes_dynamic_elements() {
    let mut document = DocumentState::default();
    document.register_body("", String::new());

    evaluate_with_document(
        r#"
                const element = document.createElement("div");

                element.setAttribute("id", "dynamic");
                document.body.appendChild(element);

                assert_equals(document.getElementById("dynamic"), element);
                assert_true(element instanceof HTMLDivElement);

                document.body.removeChild(element);

                assert_equals(document.getElementById("dynamic"), null);
            "#,
        &mut document,
    )
    .unwrap();
}

#[test]
fn connects_and_disconnects_an_entire_dynamic_subtree() {
    let mut document = DocumentState::default();
    document.register_body("", String::new());

    evaluate_with_document(
        r#"
                const parent = document.createElement("div");
                const child = document.createElement("div");

                child.setAttribute("id", "nested");
                parent.appendChild(child);

                assert_equals(document.getElementById("nested"), null);

                document.body.appendChild(parent);
                assert_equals(document.getElementById("nested"), child);

                document.body.removeChild(parent);
                assert_equals(document.getElementById("nested"), null);
            "#,
        &mut document,
    )
    .unwrap();
}

#[test]
fn resolves_duplicate_ids_in_actual_tree_order() {
    let mut document = DocumentState::default();
    document.register_body("", String::new());

    evaluate_with_document(
        r#"
                const first_created = document.createElement("div");
                const second_created = document.createElement("div");

                first_created.id = "duplicate";
                second_created.id = "duplicate";

                document.body.appendChild(second_created);
                document.body.appendChild(first_created);

                assert_equals(
                    document.getElementById("duplicate"),
                    second_created
                );
            "#,
        &mut document,
    )
    .unwrap();
}

#[test]
fn exposes_mutable_attributes_in_insertion_order() {
    let mut document = DocumentState::default();
    document.register_body("", String::new());

    evaluate_with_document(
        r#"
                const element = document.createElement("div");

                element.setAttribute("id", "before");
                element.setAttribute("data-kind", "example");
                document.body.appendChild(element);

                assert_equals(element.attributes[0].value, "before");
                assert_equals(element.attributes[1].value, "example");

                element.attributes[0].value = "after";

                assert_equals(element.id, "after");
                assert_equals(element.getAttribute("id"), "after");
                assert_equals(document.getElementById("before"), null);
                assert_equals(document.getElementById("after"), element);
            "#,
        &mut document,
    )
    .unwrap();
}

#[test]
fn replaces_inner_html_children_and_updates_id_lookup() {
    let mut document = DocumentState::default();
    document.register_body("", String::new());

    evaluate_with_document(
        r#"
                const container = document.createElement("div");
                document.body.appendChild(container);

                container.innerHTML = "<div id='first'><span id='nested'></span></div>";

                assert_not_equals(document.getElementById("first"), null);
                assert_not_equals(document.getElementById("nested"), null);

                container.innerHTML = "<p id='second'></p>";

                assert_equals(document.getElementById("first"), null);
                assert_equals(document.getElementById("nested"), null);
                assert_equals(document.getElementById("second").tagName, "P");

                container.innerHTML = "";

                assert_equals(document.getElementById("second"), null);
            "#,
        &mut document,
    )
    .unwrap();
}

#[test]
fn function_local_declarations_do_not_overwrite_global_bindings() {
    evaluate(
        r#"
                test(function() {
                    var test = "local value";
                    assert_equals(test, "local value");
                });

                test(function() {
                    assert_true(true);
                });
            "#,
    )
    .unwrap();
}

#[test]
fn replaces_outer_html_and_disconnects_the_old_subtree() {
    let mut document = DocumentState::default();
    document.register_body("", String::new());

    evaluate_with_document(
        r#"
                const original = document.createElement("div");
                original.id = "old";
                document.body.appendChild(original);

                original.outerHTML = "<p id='new'></p>";

                assert_equals(document.getElementById("old"), null);
                assert_equals(document.getElementById("new").tagName, "P");

                const replacement = document.getElementById("new");
                replacement.outerHTML = "";

                assert_equals(document.getElementById("new"), null);
            "#,
        &mut document,
    )
    .unwrap();
}

#[test]
fn exposes_live_document_tag_collections() {
    let mut document = DocumentState::default();
    document.register_body("", String::new());

    evaluate_with_document(
        r#"
                const elements = document.getElementsByTagName("div");

                assert_equals(elements.length, 0);

                const first = document.createElement("div");
                const second = document.createElement("div");

                document.body.appendChild(first);

                assert_equals(elements.length, 1);
                assert_equals(elements[0], first);

                document.body.insertBefore(second, first);

                assert_equals(elements.length, 2);
                assert_equals(elements[0], second);
                assert_equals(elements[1], first);

                document.body.removeChild(first);

                assert_equals(elements.length, 1);
                assert_equals(elements[0], second);

                const all = document.getElementsByTagName("*");
                assert_equals(all.length, 2);
            "#,
        &mut document,
    )
    .unwrap();
}

#[test]
fn evaluates_conditionals_and_returns_from_nested_branches() {
    let result = evaluate(
        r#"
                const choose = function(value) {
                    if (!value) {
                        return "missing";
                    }

                    if (value > 3) {
                        return "large";
                    } else {
                        return "small";
                    }
                };

                assert_equals(choose(0), "missing");
                assert_equals(choose(4), "large");
                assert_equals(choose(1), "small");
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Undefined);
}

#[test]
fn short_circuits_logical_operations() {
    let result = evaluate(
        r#"
                const skipped_and = false && missing_value;
                const skipped_or = true || missing_value;

                assert_equals(skipped_and, false);
                assert_equals(skipped_or, true);

                assert_equals("left" && "right", "right");
                assert_equals("" || "fallback", "fallback");
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Undefined);
}

#[test]
fn evaluates_while_loops_and_returns_from_them() {
    let result = evaluate(
        r#"
                let count = 0;

                while (count < 3) {
                    count = count + 1;
                }

                assert_equals(count, 3);

                const find_two = function() {
                    let value = 0;

                    while (true) {
                        value = value + 1;

                        if (value === 2) {
                            return value;
                        }
                    }
                };

                assert_equals(find_two(), 2);
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Undefined);
}

#[test]
fn evaluates_for_loops_updates_and_breaks() {
    let result = evaluate(
        r#"
                const sum_before_four = function() {
                    let sum = 0;

                    for (let index = 0; index < 10; index++) {
                        if (index === 4) {
                            break;
                        }

                        sum = sum + index;
                    }

                    return sum;
                };

                assert_equals(sum_before_four(), 6);

                let value = 0;
                ++value;
                value--;

                assert_equals(value, 0);

                let value = 0;
                assert_equals(++value, 1);
                assert_equals(value--, 1);
                assert_equals(value, 0);
            "#,
    )
    .unwrap();

    assert_eq!(result, Value::Undefined);
}

#[test]
fn evaluates_continue_in_loops() {
    evaluate(
        r#"
                let sum = 0;

                for (let index = 0; index < 5; index++) {
                    if (index === 2) {
                        continue;
                    }

                    sum = sum + index;
                }

                assert_equals(sum, 8);

                let current = 0;
                let count = 0;

                while (current < 5) {
                    current++;

                    if (current === 3) {
                        continue;
                    }

                    count = count + 1;
                }

                assert_equals(count, 4);
            "#,
    )
    .unwrap();
}

#[test]
fn exposes_live_element_children_and_navigation() {
    let mut document = DocumentState::default();
    document.register_body("", String::new());

    evaluate_with_document(
        r#"
                const parent = document.createElement("div");
                const first = document.createElement("span");
                const second = document.createElement("strong");

                document.body.appendChild(parent);

                assert_equals(parent.children.length, 0);
                assert_equals(parent.firstElementChild, null);

                parent.appendChild(first);
                parent.appendChild(second);

                assert_equals(parent.children.length, 2);
                assert_equals(parent.children[0], first);
                assert_equals(parent.children[1], second);
                assert_equals(parent.firstElementChild, first);
                assert_equals(parent.lastElementChild, second);
                assert_equals(second.parentElement, parent);

                parent.removeChild(first);

                assert_equals(parent.children.length, 1);
                assert_equals(parent.firstElementChild, second);
                assert_equals(first.parentElement, null);

                parent.innerHTML = "<em id='one'></em><b id='two'></b>";

                assert_equals(parent.children.length, 2);
                assert_equals(parent.children[0].tagName, "EM");
                assert_equals(parent.lastElementChild.tagName, "B");
            "#,
        &mut document,
    )
    .unwrap();
}

#[test]
fn exposes_element_siblings_and_connection_state() {
    let mut document = DocumentState::default();
    document.register_body("", String::new());

    evaluate_with_document(
        r#"
                const parent = document.createElement("div")
                const first = document.createElement("span");
                const middle = document.createElement("em");
                const last = document.createElement("strong");

                document.body.appendChild(parent);

                const children = parent.children;
                parent.appendChild(first);
                parent.appendChild(middle);
                parent.appendChild(last);

                assert_equals(children.length, 3);
                assert_equals(parent.childElementCount, 3);

                assert_equals(first.previousElementSibling, null);
                assert_equals(first.nextElementSibling, middle);
                assert_equals(middle.previousElementSibling, first);
                assert_equals(middle.nextElementSibling, last);
                assert_equals(last.previousElementSibling, middle);
                assert_equals(last.nextElementSibling, null);

                assert_equals(parent.isConnected, true);
                assert_equals(middle.isConnected, true);

                parent.removeChild(middle);

                assert_equals(children.length, 2);
                assert_equals(parent.childElementCount, 2);
                assert_equals(first.nextElementSibling, last);
                assert_equals(last.previousElementSibling, first);
                assert_equals(middle.isConnected, false);

                document.body.removeChild(parent);

                assert_equals(parent.isConnected, false);
                assert_equals(first.isConnected, false);
                assert_equals(last.isConnected, false);
            "#,
        &mut document,
    )
    .unwrap();
}

#[test]
fn queries_elements_by_tag_id_and_class() {
    let mut document = DocumentState::default();
    document.register_body("", String::new());

    evaluate_with_document(
            r##"
                document.body.innerHTML =
                    "<section id='catalog' class='panel'><p class='item'>first</p><p id='featured' class='item'>second</p></section>";

                assert_equals(document.querySelector("section").id, "catalog");
                assert_equals(document.querySelector("#featured").textContent, "second");
                assert_equals(document.querySelector(".item").textContent, "first");
                assert_equals(document.querySelector("aside"), null);
            "##,
            &mut document,
        )
            .unwrap();
}

#[test]
fn queries_only_element_descendants() {
    let mut document = DocumentState::default();
    document.register_body("", String::new());

    evaluate_with_document(
            r##"
                document.body.innerHTML =
                    "<main id='root' class='item'><section class='item'><p id='inside'>text</p></section></main>";

                const root = document.querySelector("#root");

                assert_equals(root.querySelector("#root"), null);
                assert_equals(root.querySelector("#inside").textContent, "text");
                assert_equals(root.querySelector(".item").tagName, "SECTION");
                assert_equals(root.querySelector("aside"), null);
            "##,
            &mut document,
        )
            .unwrap();
}

#[test]
fn queries_static_node_lists_in_document_and_element_scopes() {
    let mut document = DocumentState::default();
    document.register_body("", String::new());

    evaluate_with_document(
            r##"
                document.body.innerHTML =
                    "<section id='catalog'><p id='first' class='item'>one</p><p id='second' class='item'>two</p></section>";

                const all_items = document.querySelectorAll(".item");
                const catalog = document.querySelector("#catalog");
                const scoped_items = catalog.querySelectorAll(".item");

                assert_equals(all_items.length, 2);
                assert_equals(all_items[0].id, "first");
                assert_equals(all_items[1].id, "second");

                assert_equals(scoped_items.length, 2);
                assert_equals(scoped_items[0], all_items[0]);

                const extra = document.createElement("p");
                extra.setAttribute("class", "item");
                catalog.appendChild(extra);

                assert_equals(all_items.length, 2);
                assert_equals(document.querySelectorAll(".item").length, 3);
            "##,
            &mut document,
        )
            .unwrap();
}

#[test]
fn supports_compound_and_descendant_selectors() {
    let mut document = DocumentState::default();
    document.register_body("", String::new());

    evaluate_with_document(
            r##"
                document.body.innerHTML =
                    "<main id='root'><section id='panel' class='panel featured'><p id='first' class='item featured'>first</p><p id='second' class='item'>second</p></section></main>";

                assert_equals(document.querySelector("section.panel#panel").id, "panel");
                assert_equals(document.querySelector("#root .item.featured").id, "first");
                assert_equals(document.querySelector("main section .item").id, "first");
                assert_equals(document.querySelectorAll("#root .item").length, 2);
                assert_equals(document.querySelector("main .missing"), null);
            "##,
            &mut document,
        )
            .unwrap();
}

#[test]
fn reflects_class_name_to_the_class_attribute() {
    let mut document = DocumentState::default();
    document.register_body("", String::new());

    evaluate_with_document(
        r#"
                const card = document.createElement("article");
                document.body.appendChild(card);

                assert_equals(card.className, "");

                card.className = "card featured";

                assert_equals(card.className, "card featured");
                assert_equals(card.getAttribute("class"), "card featured");
                assert_equals(document.querySelector(".featured"), card);

                card.className = "card";

                assert_equals(card.getAttribute("class"), "card");
                assert_equals(document.querySelector(".featured"), null);
                assert_equals(document.querySelector(".card"), card);
            "#,
        &mut document,
    )
    .unwrap();
}

#[test]
fn mutates_classes_through_class_list() {
    let mut document = DocumentState::default();
    document.register_body("", String::new());

    evaluate_with_document(
        r#"
                const card = document.createElement("article");
                document.body.appendChild(card);

                card.classList.add("card", "featured", "card");

                assert_equals(card.className, "card featured");
                assert_equals(card.classList.length, 2);
                assert_equals(card.classList.contains("featured"), true);
                assert_equals(document.querySelector(".featured"), card);

                assert_equals(card.classList.toggle("featured"), false);
                assert_equals(card.classList.contains("featured"), false);

                assert_equals(card.classList.toggle("visible", true), true);
                assert_equals(card.className, "card visible");

                card.classList.remove("card", "visible");

                assert_equals(card.className, "");
                assert_equals(card.classList.length, 0);
            "#,
        &mut document,
    )
    .unwrap();
}

#[test]
fn evaluates_array_literals_indexing_and_length() {
    evaluate(
        r#"
            const values = [1, "two", 3,];

            assert_equals(values.length, 3);
            assert_equals(values[0], 1);
            assert_equals(values[1], "two");
            assert_equals(values[2], 3);
            assert_equals(values[3], undefined);
        "#,
    )
    .unwrap();
}

#[test]
fn evaluates_for_of_over_array_literals() {
    evaluate(
        r#"
            const of = 7;
            let sum = 0;
            let visits = 0;
            
            for (const value of [1, 2, 3, 4]) {
                if (value === 2) continue;
                if (value === 4) break;
                
                sum = sum + value;
                visits = visits + 1;
            }
            
            assert_equals(of, 7);
            assert_equals(sum, 4);
            assert_equals(visits, 2);
        "#,
    )
    .unwrap();
}

#[test]
fn arrays_preserve_identity_and_support_push_and_pop() {
    evaluate(
        r#"
            const values = [1];
            const alias = values;
            
            assert_equals(alias.push(2, 3), 3);
            assert_equals(values.length, 3);
            assert_equals(values[1], 2);
            
            assert_equals(values.pop(), 3);
            assert_equals(alias.length, 2);
        "#,
    )
    .unwrap();
}

#[test]
fn evaluates_object_literals_and_shared_property_updates() {
    evaluate(
        r#"
            const options = {
                enabled: true,
                label: "FreeDOM",
                count: 1,
            };
            
            const alias = options;
            alias.count = 2;
            alias.values = [options.count, 3];
            
            assert_true(options.enabled);
            assert_equals(options.label, "FreeDOM");
            assert_equals(options.count, 2);
            assert_equals(options.values.length, 2);
            assert_equals(options.values[0], 2);
            assert_equals(options.missing, undefined);
        "#,
    )
    .unwrap();
}

#[test]
fn assigns_object_properties_and_array_indices() {
    evaluate(
        r#"
            const settings = {};
            const property = "title";
            
            settings[property] = "FreeDOM";
            assert_equals(settings.title, "FreeDOM");
            
            const values = [1];
            values[2] = 3;
            values[0]++;
            
            assert_equals(values.length, 3);
            assert_equals(values[0], 2);
            assert_equals(values[1], undefined);
            assert_equals(values[2], 3);
        "#,
    )
    .unwrap();
}

#[test]
fn preserves_object_property_order_and_joins_array_values() {
    evaluate(
        r#"
            const details = { first: 1, second: 2 };
            details.first = 3;
            details.third = 4;
            
            const keys = Object.keys(details);
            
            assert_equals(keys.join(","), "first,second,third");
            assert_equals([1, null, undefined, 4].join("|"), "1|||4");
        "#,
    )
    .unwrap();
}

#[test]
fn evaluates_conditional_expressions_lazily() {
    evaluate(
        r#"
            let evaluated = 0;

            assert_equals(true ? "yes" : "no", "yes");
            assert_equals(false ? "yes" : "no", "no");

            const selected = false ? (evaluated = 1) : 42;
            assert_equals(selected, 42);
            assert_equals(evaluated, 0);

            assert_equals(false ? 1 : true ? 2 : 3, 2);
        "#,
    )
    .unwrap();
}

#[test]
fn evaluates_typeof_expressions() {
    evaluate(
        r#"
            assert_equals(typeof undefined, "undefined");
            assert_equals(typeof null, "object");
            assert_equals(typeof true, "boolean");
            assert_equals(typeof 42, "number");
            assert_equals(typeof "FreeDOM", "string");
            assert_equals(typeof [], "object");
            assert_equals(typeof {}, "object");
            assert_equals(typeof Object.keys, "function");
            assert_equals(typeof function() {}, "function");
        "#,
    )
    .unwrap();
}

#[test]
fn tokenizes_equality_operators_without_consuming_following_tokens() {
    let tokens = tokenize("= == === => ! != !== ==x !=y").unwrap();

    let kind: Vec<_> = tokens.into_iter().map(|token| token.kind).collect();

    assert_eq!(
        kind,
        vec![
            TokenKind::Assign,
            TokenKind::Equal,
            TokenKind::StrictEqual,
            TokenKind::Arrow,
            TokenKind::Not,
            TokenKind::NotEqual,
            TokenKind::StrictNotEqual,
            TokenKind::Equal,
            TokenKind::Identifier("x".to_owned()),
            TokenKind::NotEqual,
            TokenKind::Identifier("y".to_owned()),
            TokenKind::EndOfFile,
        ]
    );
}

#[test]
fn evaluates_equality_operators() {
    for (source, expected) in [
        ("1 == '1'", true),
        ("1 != '1'", false),
        ("1 === '1'", false),
        ("1 !== '1'", true),
        ("2 == 3", false),
        ("2 != 3", true),
    ] {
        let result = evaluate(source).unwrap();

        assert_eq!(result, Value::Boolean(expected), "source: {source}");
    }
}

#[test]
fn constructs_and_uses_regexp_objects() {
    evaluate(
        r#"
            const re = new RegExp("a", "g");
            const alias = re;

            assert_equals(typeof re, "object");
            assert_equals(typeof re.test, "function");

            assert_true(re === alias);
            assert_false(re === new RegExp("a", "g"));

            assert_true(re.test("aba"));
            assert_equals(alias.lastIndex, 1);

            assert_true(alias.test("aba"));
            assert_equals(re.lastIndex, 3);

            assert_false(re.test("aba"));
            assert_equals(re.lastIndex, 0);
        "#,
    )
    .unwrap();
}

#[test]
fn regexp_constructor_defaults_and_test_default_argument() {
    evaluate(
        r#"
            const empty = new RegExp();
            assert_true(empty.test("text"));

            const re = new RegExp("undefined");
            assert_true(re.test());

            const implicit = new RegExp(undefined, undefined);
            assert_true(implicit.test("x"));
        "#,
    )
    .unwrap();
}

#[test]
fn regexp_constructor_evaluate_extra_arguments() {
    evaluate(
        r#"
            let count = 0;
            const re = new RegExp("a", "", count = 1);

            assert_equals(count, 1);
            assert_true(re.test("a"));
        "#,
    )
    .unwrap();
}

#[test]
fn regexp_last_index_preserves_assigned_values_until_matching() {
    evaluate(
        r#"
            const re = new RegExp("a", "g");
            const alias = re;

            re.lastIndex = 1.9;
            assert_equals(alias.lastIndex, 1.9);

            assert_true(alias.test("aba"));
            assert_equals(re.lastIndex, 3);

            re.lastIndex = 0 - 4;
            assert_true(re.test("aba"));
            assert_equals(alias.lastIndex, 1);

            re.lastIndex = true;
            assert_true(re.test("aba"));
            assert_equals(re.lastIndex, 3);

            re.lastIndex = null;
            assert_true(re.test("aba"));
            assert_equals(re.lastIndex, 1);

            re.lastIndex = undefined;
            assert_true(re.test("aba"));
            assert_equals(re.lastIndex, 1);
        "#,
    )
    .unwrap();
}

#[test]
fn regexp_stateless_test_preserves_original_last_index() {
    evaluate(
        r#"
            const re = new RegExp("a");
            re.lastIndex = 4.5;

            assert_true(re.test("a"));
            assert_equals(re.lastIndex, 4.5);

            assert_false(re.test("b"));
            assert_equals(re.lastIndex, 4.5);
        "#,
    )
    .unwrap();
}

#[test]
fn regexp_last_index_coerces_strings_at_match_time() {
    evaluate(
        r#"
            const re = new RegExp("a", "g");

            re.lastIndex = "2";
            assert_equals(re.lastIndex, "2");

            assert_true(re.test("aba"));
            assert_equals(re.lastIndex, 3);
        "#,
    )
    .unwrap();
}

#[test]
fn indexed_regexp_properties_share_state() {
    evaluate(
        r#"
            const re = new RegExp("a", "g");
            const key = "lastIndex";

            re[key] = "0x2";
            assert_equals(re.lastIndex, "0x2");

            assert_true(re["test"]("aba"));
            assert_equals(re[key], 3);

            re.lastIndex = "not-a-number";
            assert_true(re.test("aba"));
            assert_equals(re[key], 1);
        "#,
    )
    .unwrap();
}

#[test]
fn arrays_support_named_properties_and_indexed_access() {
    evaluate(
        r#"
            const values = [1, 2];
            const alias = values;

            values.index = 7;
            values["input"] = "abc";

            assert_equals(alias["index"], 7);
            assert_equals(alias.input, "abc");
            assert_equals(values.length, 2);
            assert_equals(values["length"], 2);
            assert_equals(values.missing, undefined);

            values["01"] = 99;
            values["4294967295"] = 88;

            assert_equals(values["01"], 99);
            assert_equals(values[1], 2);
            assert_equals(values["4294967295"], 88);
            assert_equals(values.length, 2);

            values[4] = 5;
            assert_equals(values.length, 5);
            assert_equals(values[4], 5);
            assert_equals(values[3], undefined);
        "#,
    )
    .unwrap();
}

#[test]
fn array_named_properties_coexist_with_methods() {
    evaluate(
        r#"
            const values = [1, 2];
            values.index = 7;

            assert_equals(values["push"](3), 3);
            assert_equals(values.join("-"), "1-2-3");
            assert_equals(values.pop(), 3);
            assert_equals(values.length, 2);
            assert_equals(values.index, 7);

            values.push = "overridden";
            assert_equals(values.push, "overridden");
            assert_equals(values["push"], "overridden");
        "#,
    )
    .unwrap();
}

#[test]
fn array_length_assignment_is_explicitly_unsupported() {
    let error = evaluate("const values = [1]; values.length = 0;").unwrap_err();

    assert!(
        error
            .message
            .contains("Array.length assignment is not implemented")
    );
}

#[test]
fn regexp_exec_returns_captures_properties_and_named_indices() {
    evaluate(
        r#"
            const re = new RegExp("(?<letter>a)(b)?", "dg");

            assert_equals(typeof re.exec, "function");

            const result = re.exec("zab");

            assert_equals(result.length, 3);
            assert_equals(result[0], "ab");
            assert_equals(result[1], "a");
            assert_equals(result[2], "b");

            assert_equals(result.index, 1);
            assert_equals(result.input, "zab");
            assert_equals(result.groups.letter, "a");

            assert_equals(result.indices[0][0], 1);
            assert_equals(result.indices[0][1], 3);
            assert_equals(result.indices[1][0], 1);
            assert_equals(result.indices[1][1], 2);

            assert_equals(result.indices.groups.letter[0], 1);
            assert_equals(re.lastIndex, 3);

            assert_equals(re["exec"]("zab"), null);
            assert_equals(re.lastIndex, 0);
        "#,
    )
    .unwrap();
}

#[test]
fn regexp_exec_keeps_unmatched_groups_and_absent_indices_undefined() {
    evaluate(
        r#"
            const re = new RegExp("(?<x>a)?b", "d");
            const result = re.exec("b");

            assert_equals(result[1], undefined);
            assert_equals(result.groups.x, undefined);
            assert_equals(result.indices[1], undefined);
            assert_equals(result.indices.groups.x, undefined);

            const plain = new RegExp("(a)").exec("a");

            assert_equals(plain.groups, undefined);
            assert_equals(plain.indices, undefined);

            const indexed = new RegExp("(a)", "d").exec("😀a");

            assert_equals(indexed.index, 2);
            assert_equals(indexed.indices[1][0], 2);
            assert_equals(indexed.indices[1][1], 3);
            assert_equals(indexed.indices.groups, undefined);
        "#,
    )
    .unwrap();
}

#[test]
fn regexp_exec_rejects_unpaired_surrogates_without_lossy_conversion() {
    let error = evaluate(r#"new RegExp(".", "g").exec("😀");"#).unwrap_err();

    assert!(error.message.contains("unpaired UTF-16 surrogate"))
}

#[test]
fn compares_arrays_and_objects_by_identity() {
    evaluate(
        r#"
            const first = [1];
            const alias = first;
            const second = [1];

            assert_true(first === alias);
            assert_false(first === second);
            assert_true(first !== second);
            assert_false(first == second);

            assert_equals(first, alias);
            assert_not_equals(first, second);

            const object = {value: 1};
            const same = object;
            const different = {value: 1};

            assert_true(object === same);
            assert_false(object === different);
            assert_not_equals(object, different);
        "#,
    )
    .unwrap();
}

#[test]
fn regexp_indices_named_group_reuses_the_same_array() {
    evaluate(
        r#"
            const result = new RegExp("(?<x>a)", "d").exec("a");

            assert_true(
                result.indices.groups.x === result.indices[1]
            );

            assert_false(
                result.indices.groups.x === result.indices[0]
            );

            assert_not_equals(
                result.indices[0],
                result.indices[1]
            );
        "#,
    )
    .unwrap();
}

#[test]
fn wpt_assertions_use_same_value_for_nan_and_signed_zero() {
    evaluate(
        r#"
            const nan = 0 / 0;
            const negativeZero = 0 / (0 - 1);

            assert_equals(nan, nan);
            assert_not_equals(0, negativeZero);

            assert_true(0 === negativeZero);
            assert_false(nan === nan);
        "#,
    )
    .unwrap();
}

#[test]
fn assert_equals_rejects_distinct_arrays_with_equal_contents() {
    let error = evaluate("assert_equals([1], [1]);").unwrap_err();

    assert!(error.message.contains("AssertionError"));
}

#[test]
fn uses_string_numeric_grammar_for_arithmetic_and_regexp() {
    evaluate(
        r#"
            assert_equals(1 * "0x10", 16);
            assert_equals(1 * "0b101", 5);
            assert_equals(1 * "0o10", 8);
            assert_equals(1 * " 1.5 ", 1.5);

            assert_equals(1 * "invalid", 0 / 0);
            assert_equals(1 * undefined, 0 / 0);

            const re = new RegExp("a", "g");
            re.lastIndex = "0x2";

            assert_true(re.test("aba"));
            assert_equals(re.lastIndex, 3);
        "#,
    )
    .unwrap();
}
