use super::*;

#[test]
fn moving_and_removing_children_updates_connection_and_lookup() {
    let mut document = DocumentState::default();
    let body = document.register_parsed_element("body", "", String::new(), None);
    let parent = document.register_parsed_element("div", "parent", String::new(), Some(body));
    let child = document.create_element("span");

    assert!(document.set_node_attribute(child, "ID", "child".to_owned()));
    assert_eq!(document.get_element_by_id("child"), None);

    assert!(document.append_child(parent, child));
    assert!(document.element(child).unwrap().is_connected());
    assert_eq!(document.get_element_by_id("child"), Some(child));
    assert_eq!(document.element(child).unwrap().parent(), Some(parent));

    assert!(!document.remove_child(body, child));
    assert!(document.remove_child(parent, child));
    assert!(!document.element(child).unwrap().is_connected());
    assert_eq!(document.get_element_by_id("child"), None);
    assert_eq!(document.element(parent).unwrap().children(), &[] as &[u64]);
}

#[test]
fn selectors_match_descendants_and_reject_unsupported_syntax() {
    let mut document = DocumentState::default();
    let list = document.register_parsed_element("ul", "list", String::new(), None);
    let first = document.register_parsed_element("li", "", String::new(), Some(list));
    let second = document.register_parsed_element("li", "", String::new(), Some(list));
    document.register_element_attribute(second, "class", "item selected".to_owned());

    let all_items = parse_simple_selector("#list li").unwrap();
    assert_eq!(document.query_selector_all(&all_items), vec![first, second]);

    let selected = parse_simple_selector("UL li.item.selected").unwrap();
    assert_eq!(document.query_selector(&selected), Some(second));
    assert_eq!(document.query_selector_within(second, &selected), None);

    assert_eq!(
        parse_simple_selector(" ").unwrap_err().to_string(),
        "selector must not be empty"
    );
    assert_eq!(
        parse_simple_selector("li > a").unwrap_err().to_string(),
        "unsupported selector"
    );
}

#[test]
fn outer_html_replaces_an_element_in_place() {
    let mut document = DocumentState::default();
    let parent = document.register_parsed_element("div", "", String::new(), None);
    let before = document.register_parsed_element("p", "", String::new(), Some(parent));
    let target = document.register_parsed_element("p", "old", String::new(), Some(parent));
    let after = document.register_parsed_element("p", "", String::new(), Some(parent));

    assert!(document.set_node_outer_html(target, "<b id=a>A</b><i id=b>B</i>"));

    let a = document.get_element_by_id("a").unwrap();
    let b = document.get_element_by_id("b").unwrap();

    assert_eq!(
        document.element(parent).unwrap().children(),
        &[before, a, b, after]
    );
    assert_eq!(document.element(a).unwrap().text_content(), "A");
    assert!(!document.element(target).unwrap().is_connected());
    assert_eq!(document.get_element_by_id("old"), None);
}
