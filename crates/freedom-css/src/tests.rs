use super::*;

#[test]
fn line_height_cascades_after_font_size_and_inherits_correctly() {
    let rules = build_author_rules(&[
        "div { line-height: 150%; font-size: 20px } span { font-size: 10px }".to_owned(),
    ]);
    let div = Element {
        tag: "div",
        id: None,
        classes: &[],
        inline_style: None,
    };
    let span = Element {
        tag: "span",
        id: None,
        classes: &[],
        inline_style: None,
    };
    let parent = compute_style(&div, TextStyle::initial(), &rules);
    assert_eq!(parent.line_height, LineHeight::Pixels(30.0));
    let child = compute_style(&span, parent.clone(), &rules);
    assert_eq!(child.font_size, 10.0);
    assert_eq!(child.line_height, LineHeight::Pixels(30.0));

    let rules = build_author_rules(&[
        "div { line-height: 1.5; font-size: 20px } span { font-size: 10px }".to_owned(),
    ]);
    let parent = compute_style(&div, TextStyle::initial(), &rules);
    assert_eq!(
        compute_style(&span, parent, &rules).line_height,
        LineHeight::Number(1.5)
    );

    let rules = build_author_rules(&[
        "div { line-height: 2em !important; font-size: 10px; line-height: 4px }".to_owned(),
    ]);
    assert_eq!(
        compute_style(&div, TextStyle::initial(), &rules).line_height,
        LineHeight::Pixels(20.0)
    );
    assert!(parse_line_height("-1px").is_none());
    assert!(parse_line_height("1 px").is_none());
    assert!(parse_line_height("NaN").is_none());
}

#[test]
fn parses_css_font_weights() {
    assert_eq!(parse_font_weight("normal"), Some(FontWeight::Normal));
    assert_eq!(parse_font_weight("400"), Some(FontWeight::Normal));
    assert_eq!(parse_font_weight("bold"), Some(FontWeight::Bold));
    assert_eq!(parse_font_weight("700"), Some(FontWeight::Bold));
    assert_eq!(parse_font_weight("1200"), None);
}

#[test]
fn parses_css_font_styles() {
    assert_eq!(parse_font_style("normal"), Some(FontStyle::Normal));
    assert_eq!(parse_font_style("italic"), Some(FontStyle::Italic));
    assert_eq!(parse_font_style("oblique"), Some(FontStyle::Italic));
    assert_eq!(parse_font_style("oblique 12deg"), None);
}

#[test]
fn parses_compound_css_selectors_with_correct_specificity() {
    let (selector, specificity) = parse_selector("p#lead.notice.wide").unwrap();

    assert_eq!(selector.tag.as_deref(), Some("p"));
    assert_eq!(selector.id.as_deref(), Some("lead"));
    assert_eq!(selector.classes, ["notice", "wide"]);
    assert_eq!(specificity, Specificity::new(1, 2, 1));

    let classes = vec!["notice".to_owned(), "wide".to_owned()];

    let element = Element {
        tag: "p",
        id: Some("lead"),
        classes: &classes,
        inline_style: None,
    };

    assert!(compound_matches(&selector, &element));
}

#[test]
fn exposes_cascaded_declarations_for_other_renderers() {
    let stylesheets =
        vec!["line { stroke: red; } #target { stroke: blue; stroke-width: 4; }".to_owned()];
    let rules = build_cascade_rules(&stylesheets);
    let classes = Vec::new();

    let element = Element {
        tag: "line",
        id: Some("target"),
        classes: &classes,
        inline_style: None,
    };

    let declarations = matching_declarations(&rules, &element)
        .into_iter()
        .map(|declaration| (declaration.name.as_str(), declaration.value.as_str()))
        .collect::<Vec<_>>();

    assert_eq!(
        declarations,
        vec![("stroke", "red"), ("stroke", "blue"), ("stroke-width", "4")]
    );
}

#[test]
fn parses_font_face_woff_sources() {
    let stylesheets = [r#"
            @font-face {
                font-family: "FreeDOM Sans";
                src: local("FreeDOM Sans"),
                     url("fonts/freedom.woff") format("woff"),
                     url('fonts/fallback.ttf') format("truetype");
                font-weight: bold;
                font-style: italic;
            }
        "#
    .to_owned()];

    let faces = parse_font_faces(&stylesheets);

    assert_eq!(faces.len(), 1);
    assert_eq!(faces[0].family, "FreeDOM Sans");
    assert_eq!(
        faces[0].sources,
        vec![
            "fonts/freedom.woff".to_owned(),
            "fonts/fallback.ttf".to_owned(),
        ]
    );
    assert_eq!(faces[0].font_weight, FontWeight::Bold);
    assert_eq!(faces[0].font_style, FontStyle::Italic);
}

#[test]
fn cascades_the_first_font_family_name() {
    let stylesheets = vec![r#"p { font-family: "FreeDOM Sans", sans-serif; }"#.to_owned()];
    let rules = build_author_rules(&stylesheets);
    let classes = Vec::new();

    let element = Element {
        tag: "p",
        id: None,
        classes: &classes,
        inline_style: None,
    };

    let style = compute_style(&element, TextStyle::initial(), &rules);

    assert_eq!(
        style.font_families.first().map(String::as_str),
        Some("FreeDOM Sans")
    );
}

#[test]
fn quoted_family_name_can_contain_a_comma() {
    assert_eq!(
        parse_text_font_family(r#""Nko, Test", sans-serif"#),
        Some("Nko, Test".to_owned())
    );

    assert_eq!(
        parse_font_face_family(r#""Nko, Test""#),
        Some("Nko, Test".to_owned())
    );

    assert_eq!(parse_text_font_family(r#""Nko, Test"#), None);

    assert_eq!(
        parse_text_font_families(r#""Nko, Test", sans-serif"#),
        Some(vec!["Nko, Test".to_owned(), "sans-serif".to_owned()])
    );

    assert_eq!(parse_text_font_families("Test, "), None);
    assert_eq!(parse_text_font_families("Test, 'broken"), None);
}

#[test]
fn invalid_declarations_do_not_erase_earlier_valid_values() {
    let declarations = parse_declaration_list(
        "color: green; color: not-a-color; \
            font-weight: bold; font-weight: nope;\
            display: block; display: nope",
    );

    let parsed = parse_text_declarations(&declarations);

    assert_eq!(parsed.color, Some(TextColor::Value(0x00_00_80_00)));
    assert_eq!(parsed.font_weight, Some(FontWeight::Bold));
    assert!(matches!(parsed.display, Some(Display::Block)));
}

#[test]
fn stylesheet_preserves_comments_in_quoted_font_family() {
    let stylesheets = ["/* note */ p { font-family: 'A/*B*/C'; color: green; }".to_owned()];

    let rules = build_author_rules(&stylesheets);

    let element = Element {
        tag: "p",
        id: None,
        classes: &[],
        inline_style: None,
    };

    let style = compute_style(&element, TextStyle::initial(), &rules);

    assert_eq!(
        style.font_families.first().map(String::as_str),
        Some("A/*B*/C")
    );

    assert_eq!(style.color, 0x00_00_80_00);
}

#[test]
fn selector_matches_non_ascii_class_and_rejects_digit_starts() {
    let stylesheets = [".💡 { color: green } .123 { color: red }".to_owned()];

    let rules = build_author_rules(&stylesheets);

    assert_eq!(rules.len(), 1);

    let classes = vec!["💡".to_owned()];

    let element = Element {
        tag: "p",
        id: None,
        classes: &classes,
        inline_style: None,
    };

    let style = compute_style(&element, TextStyle::initial(), &rules);

    assert_eq!(style.color, 0x00_00_80_00);
}

#[test]
fn selector_decodes_escaped_class_name() {
    let stylesheets = [r".\31 23 { color: green }".to_owned()];
    let rules = build_author_rules(&stylesheets);

    assert_eq!(rules.len(), 1);

    let classes = vec!["123".to_owned()];

    let element = Element {
        tag: "p",
        id: None,
        classes: &classes,
        inline_style: None,
    };

    let style = compute_style(&element, TextStyle::initial(), &rules);

    assert_eq!(style.color, 0x00_00_80_00);
}

#[test]
fn selector_list_with_escaped_comma_matches_both_classes() {
    let stylesheets = [r".hello\,world, .other { color: green }".to_owned()];

    let rules = build_author_rules(&stylesheets);

    assert_eq!(rules.len(), 2);

    for class in ["hello,world", "other"] {
        let classes = vec![class.to_owned()];

        let element = Element {
            tag: "p",
            id: None,
            classes: &classes,
            inline_style: None,
        };

        let style = compute_style(&element, TextStyle::initial(), &rules);

        assert_eq!(style.color, 0x00_00_80_00);
    }
}

#[test]
fn ignores_entire_rule_with_invalid_selector_list() {
    for source in [
        "p,,div { color: red }",
        "p, .123 { color: red }",
        "p, { color: red }",
    ] {
        let rules = build_author_rules(&[source.to_owned()]);

        assert!(rules.is_empty(), "source: {source}");
    }

    let rules = build_author_rules(&["p, div { color: green }".to_owned()]);

    assert_eq!(rules.len(), 2);
}

#[test]
fn id_specificity_exceeds_many_classes() {
    let stylesheets = ["#target { color: green } \
             .x.x.x.x.x.x.x.x.x.x.x { color: red }"
        .to_owned()];

    let rules = build_author_rules(&stylesheets);
    let classes = vec!["x".to_owned()];

    let element = Element {
        tag: "p",
        id: Some("target"),
        classes: &classes,
        inline_style: None,
    };

    let style = compute_style(&element, TextStyle::initial(), &rules);

    assert_eq!(style.color, 0x00_00_80_00);
}

#[test]
fn equal_specificity_respects_source_order() {
    let stylesheets = [".a { color: red } .b { color: green }".to_owned()];

    let rules = build_author_rules(&stylesheets);
    let classes = vec!["a".to_owned(), "b".to_owned()];

    let element = Element {
        tag: "p",
        id: None,
        classes: &classes,
        inline_style: None,
    };

    let style = compute_style(&element, TextStyle::initial(), &rules);

    assert_eq!(style.color, 0x00_00_80_00);
}

#[test]
fn relative_font_sizes_use_parent_not_previous_cascade_values() {
    let stylesheets = ["h1 { font-size: 2em } .lead { font-size: 3em }".to_owned()];

    let rules = build_author_rules(&stylesheets);
    let classes = vec!["lead".to_owned()];

    let element = Element {
        tag: "h1",
        id: None,
        classes: &classes,
        inline_style: Some("font-size: 1.5em"),
    };

    let mut parent_style = TextStyle::initial();
    parent_style.font_size = 20.0;

    let style = compute_style(&element, parent_style.clone(), &rules);

    assert_eq!(style.font_size, 30.0);

    let element_without_inline = Element {
        tag: "h1",
        id: None,
        classes: &classes,
        inline_style: None,
    };

    let style = compute_style(&element_without_inline, parent_style, &rules);

    assert_eq!(style.font_size, 60.0);
}

#[test]
fn important_overrides_normal_rules_and_inline_style() {
    let stylesheets = [
        "p { color: green !important; font-size: 2em !IMPORTANT; color: red; } \
             #target { color: red; font-size: 30px }"
            .to_owned(),
    ];

    let rules = build_author_rules(&stylesheets);

    let element = Element {
        tag: "p",
        id: Some("target"),
        classes: &[],
        inline_style: Some("color: blue; font-size: 40px"),
    };

    let mut parent = TextStyle::initial();
    parent.font_size = 12.0;

    let style = compute_style(&element, parent.clone(), &rules);

    assert_eq!(style.color, 0x00_00_80_00);
    assert_eq!(style.font_size, 24.0);

    let important_inline = Element {
        inline_style: Some("color: blue !important"),
        ..element
    };

    let style = compute_style(&important_inline, parent.clone(), &rules);

    assert_eq!(style.color, 0x00_00_00_FF);
}

#[test]
fn invalid_unicode_hex_color_preserves_previous_value() {
    let element = Element {
        tag: "p",
        id: None,
        classes: &[],
        inline_style: Some("color: green; color: #éa"),
    };

    let style = compute_style(&element, TextStyle::initial(), &[]);

    assert_eq!(style.color, 0x00_00_80_00);
}

#[test]
fn cascades_clamped_rgb_colors() {
    let stylesheets = ["p { color: rgb(-500, 64, 128) }".to_owned()];

    let rules = build_author_rules(&stylesheets);

    let element = Element {
        tag: "p",
        id: None,
        classes: &[],
        inline_style: None,
    };

    let style = compute_style(&element, TextStyle::initial(), &rules);

    assert_eq!(style.color, 0x00_00_40_80);
}

#[test]
fn zero_font_size_is_accepted_and_negative_is_rejected() {
    for value in ["0", "0px", "0em", "-0px"] {
        let inline_style = format!("font-size: 18px; font-size: {value}");

        let element = Element {
            tag: "p",
            id: None,
            classes: &[],
            inline_style: Some(&inline_style),
        };

        let style = compute_style(&element, TextStyle::initial(), &[]);

        assert_eq!(style.font_size, 0.0, "value: {value}");
    }

    let element = Element {
        tag: "p",
        id: None,
        classes: &[],
        inline_style: Some("font-size: 18px; font-size: -1px"),
    };

    let style = compute_style(&element, TextStyle::initial(), &[]);

    assert_eq!(style.font_size, 18.0);
}

#[test]
fn percentage_font_size_uses_parent_and_respects_cascade() {
    let mut parent = TextStyle::initial();
    parent.font_size = 20.0;

    for (value, expected) in [("150%", 30.0), ("25%", 5.0), ("0%", 0.0), ("-0%", 0.0)] {
        let inline_style = format!("font-size: 7px; font-size: {value}");

        let element = Element {
            tag: "p",
            id: None,
            classes: &[],
            inline_style: Some(&inline_style),
        };

        let style = compute_style(&element, parent.clone(), &[]);

        assert_eq!(style.font_size, expected, "value: {value}");
    }

    for value in ["-1%", "nan%", "inf%", "20 %"] {
        let inline_style = format!("font-size: 18px; font-size: {value}");

        let element = Element {
            tag: "p",
            id: None,
            classes: &[],
            inline_style: Some(&inline_style),
        };

        let style = compute_style(&element, parent.clone(), &[]);

        assert_eq!(style.font_size, 18.0, "value: {value}");
    }
}

#[test]
fn font_size_defaulting_keywords_follow_cascade() {
    let mut parent = TextStyle::initial();
    parent.font_size = 24.0;

    for (value, expected) in [("inherit", 24.0), ("unset", 24.0), ("INITIAL", 16.0)] {
        let inline_style = format!("font-size: 30px; font-size: {value}");

        let element = Element {
            tag: "h1",
            id: None,
            classes: &[],
            inline_style: Some(&inline_style),
        };

        let style = compute_style(&element, parent.clone(), &[]);

        assert_eq!(style.font_size, expected, "value: {value}");
    }

    let important = Element {
        tag: "p",
        id: None,
        classes: &[],
        inline_style: Some("font-size: inherit !important; font-size: 30px"),
    };

    let style = compute_style(&important, parent, &[]);

    assert_eq!(style.font_size, 24.0);
}

#[test]
fn absolute_font_size_keywords_do_not_depend_on_parent() {
    let mut parent = TextStyle::initial();
    parent.font_size = 25.0;

    for (keyword, expected) in [
        ("xx-small", 9.0),
        ("x-small", 10.0),
        ("small", 13.0),
        ("medium", 16.0),
        ("large", 18.0),
        ("x-large", 24.0),
        ("xx-large", 32.0),
        ("XXX-LARGE", 48.0),
    ] {
        let inline_style = format!("font-size: 7px; font-size: {keyword}");

        let element = Element {
            tag: "h1",
            id: None,
            classes: &[],
            inline_style: Some(&inline_style),
        };

        let style = compute_style(&element, parent.clone(), &[]);

        assert_eq!(style.font_size, expected, "keyword: {keyword}");
    }
}

#[test]
fn relative_font_size_keywords_use_parent_size() {
    for (parent_size, keyword, expected) in [
        (20.0, "larger", 24.0),
        (24.0, "smaller", 20.0),
        (0.0, "larger", 0.0),
        (20.0, "LARGER", 24.0),
    ] {
        let mut parent = TextStyle::initial();
        parent.font_size = parent_size;

        let inline_style = format!("font-size: 100px; font-size: {keyword}");

        let element = Element {
            tag: "h1",
            id: None,
            classes: &[],
            inline_style: Some(&inline_style),
        };

        let style = compute_style(&element, parent, &[]);

        assert!(
            (style.font_size - expected).abs() < 0.001,
            "keyword: {keyword}, actual: {}",
            style.font_size
        );
    }
}

#[test]
fn color_defaulting_keywords_use_parent_or_initial() {
    let mut parent = TextStyle::initial();
    parent.color = 0x00_FF_00_00;

    for (keyword, expected) in [
        ("currentColor", 0x00_FF_00_00),
        ("inherit", 0x00_FF_00_00),
        ("unset", 0x00_FF_00_00),
        ("initial", TextStyle::initial().color),
    ] {
        let inline_style = format!("color: green; color: {keyword}");

        let element = Element {
            tag: "a",
            id: None,
            classes: &[],
            inline_style: Some(&inline_style),
        };

        let style = compute_style(&element, parent.clone(), &[]);

        assert_eq!(style.color, expected, "keyword: {keyword}");
    }

    let element = Element {
        tag: "p",
        id: None,
        classes: &[],
        inline_style: Some("color: currentcolor !important; color: green"),
    };

    let style = compute_style(&element, parent, &[]);

    assert_eq!(style.color, 0x00_FF_00_00);
}

#[test]
fn vertical_padding_cascades_and_does_not_inherit() {
    for input in ["-0.1px", "-1px", "2", "auto", "1em", "NaNpx"] {
        assert_eq!(parse_padding_length(input), None, "{input}");
    }
    assert_eq!(parse_padding_length(".5in"), Some(48));

    let element = Element {
        tag: "div",
        id: None,
        classes: &[],
        inline_style: Some("padding-top: 8px; padding-top: -0.1px; padding-bottom: 5px"),
    };
    let style = compute_style(&element, TextStyle::initial(), &[]);
    assert_eq!((style.padding_top, style.padding_bottom), (8, 5));
    let child = Element {
        tag: "span",
        id: None,
        classes: &[],
        inline_style: None,
    };
    let style = compute_style(&child, style, &[]);
    assert_eq!((style.padding_top, style.padding_bottom), (0, 0));
}

#[test]
fn parses_margin_shorthand() {
    assert_eq!(parse_margin_shorthand("1px"), Some((1, 1, 1, 1)));
    assert_eq!(parse_margin_shorthand("1px 2px"), Some((1, 2, 1, 2)));
    assert_eq!(parse_margin_shorthand("1px 2px 3px"), Some((1, 2, 3, 2)));

    assert_eq!(
        parse_margin_shorthand("-1px 2px 3px 4px"),
        Some((-1, 2, 3, 4))
    );

    assert_eq!(parse_margin_shorthand("0 2PX"), Some((0, 2, 0, 2)));

    for input in [
        "",
        "1px 2em",
        "1px 2px 3px 4px 5px",
        "3",
        "NaNpx",
        "1px 2px junk",
    ] {
        assert_eq!(parse_margin_shorthand(input), None, "{input}");
    }
}

#[test]
fn cascades_vertical_margins_and_does_not_inherit_them() {
    let stylesheets = vec![
        "div { margin: 1px 2px 3px 4px; \
             margin-top: 5px !important }"
            .to_owned(),
    ];

    let rules = build_author_rules(&stylesheets);

    let element = Element {
        tag: "div",
        id: None,
        classes: &[],
        inline_style: Some("margin: 6px 7px 8px; margin-bottom: 9px"),
    };

    let style = compute_style(&element, TextStyle::initial(), &rules);

    assert_eq!((style.margin_before, style.margin_after), (5, 9));
    assert_eq!((style.margin_left, style.margin_right), (7, 7));

    let child = Element {
        tag: "span",
        id: None,
        classes: &[],
        inline_style: None,
    };

    let child_style = compute_style(&child, style, &[]);

    assert_eq!(
        (child_style.margin_before, child_style.margin_after),
        (0, 0)
    );

    assert_eq!((child_style.margin_left, child_style.margin_right), (0, 0));
}

#[test]
fn invalid_margin_shorthand_does_not_override_previous_values() {
    let element = Element {
        tag: "div",
        id: None,
        classes: &[],
        inline_style: Some(
            "margin: 2px 3px 4px; \
                 margin: 8px nope; \
                 margin-top: 1px 2px",
        ),
    };

    let style = compute_style(&element, TextStyle::initial(), &[]);

    assert_eq!((style.margin_before, style.margin_after), (2, 4));
    assert_eq!((style.margin_left, style.margin_right), (3, 3));
}

#[test]
fn parses_absolute_inches_in_vertical_margins() {
    for (input, expected) in [
        ("2in", Some(192)),
        ("-2in", Some(-192)),
        ("0.5IN", Some(48)),
        ("0", Some(0)),
        ("2px", Some(2)),
        ("2em", None),
        ("2 in", None),
        ("1e308in", None),
    ] {
        assert_eq!(parse_margin_length(input), expected, "{input}");
    }

    assert_eq!(
        parse_margin_shorthand("1in 0 -0.5in"),
        Some((96, 0, -48, 0))
    );
}

#[test]
fn cascades_inches_in_vertical_margin_declarations() {
    let rules = build_author_rules(&[concat!(
        "#div1 { margin-bottom: 2in } ",
        "#div2 { margin-top: -2in }"
    )
    .to_owned()]);

    let first = Element {
        tag: "div",
        id: Some("div1"),
        classes: &[],
        inline_style: None,
    };

    let second = Element {
        tag: "div",
        id: Some("div2"),
        classes: &[],
        inline_style: None,
    };

    let first_style = compute_style(&first, TextStyle::initial(), &rules);
    let second_style = compute_style(&second, TextStyle::initial(), &rules);

    assert_eq!(first_style.margin_after, 192);
    assert_eq!(second_style.margin_before, -192);
}

#[test]
fn parses_fixed_height_subset() {
    for (value, expected) in [
        ("20px", Some(CssSize::Pixels(20))),
        (" 0PX ", Some(CssSize::Pixels(0))),
        ("0", Some(CssSize::Pixels(0))),
        ("auto", Some(CssSize::Auto)),
        ("-1px", None),
        ("-0.5px", None),
        ("20", None),
        ("20%", None),
        ("0.5px", None),
        ("2147483648px", None),
        ("1in", Some(CssSize::Pixels(96))),
        ("2IN", Some(CssSize::Pixels(192))),
        ("0in", Some(CssSize::Pixels(0))),
        ("-1in", None),
        ("0.5in", None),
        ("22369622in", None),
    ] {
        assert_eq!(parse_size(value), expected, "{value}");
    }
}

#[test]
fn cascades_height_and_resets_it_for_children() {
    let rules = build_author_rules(&["div { height: 20px !important }".to_owned()]);

    let element = Element {
        tag: "div",
        id: None,
        classes: &[],
        inline_style: Some("height: 10px"),
    };

    let style = compute_style(&element, TextStyle::initial(), &rules);

    assert_eq!(style.height, CssSize::Pixels(20));

    let child = Element {
        tag: "span",
        id: None,
        classes: &[],
        inline_style: None,
    };

    let child_style = compute_style(&child, style, &[]);

    assert_eq!(child_style.height, CssSize::Auto);

    let reset = Element {
        tag: "div",
        id: None,
        classes: &[],
        inline_style: Some("height: 20px; height: auto"),
    };

    let reset_style = compute_style(&reset, TextStyle::initial(), &[]);

    assert_eq!(reset_style.height, CssSize::Auto);
}

#[test]
fn cascades_width_and_resets_it_for_children() {
    let rules = build_author_rules(&["div { width: 50px !important }".to_owned()]);

    let element = Element {
        tag: "div",
        id: None,
        classes: &[],
        inline_style: Some("width: 10px"),
    };

    let style = compute_style(&element, TextStyle::initial(), &rules);

    assert_eq!(style.width, CssSize::Pixels(50));

    let child = Element {
        tag: "span",
        id: None,
        classes: &[],
        inline_style: None,
    };

    let child_style = compute_style(&child, style, &[]);

    assert_eq!(child_style.width, CssSize::Auto);

    let reset = Element {
        tag: "div",
        id: None,
        classes: &[],
        inline_style: Some("width: 20px; width: auto"),
    };

    let reset_style = compute_style(&reset, TextStyle::initial(), &[]);

    assert_eq!(reset_style.width, CssSize::Auto);

    for (input, expected) in [
        ("0", Some(CssSize::Pixels(0))),
        ("20PX", Some(CssSize::Pixels(20))),
        ("-1px", None),
        ("20%", None),
    ] {
        assert_eq!(parse_size(input), expected, "{input}");
    }
}

#[test]
fn cascades_background_color_without_inheriting_it() {
    let rules = build_author_rules(&["div { background: orange !important }".to_owned()]);

    let element = Element {
        tag: "div",
        id: None,
        classes: &[],
        inline_style: Some("background-color: blue"),
    };

    let style = compute_style(&element, TextStyle::initial(), &rules);

    assert_eq!(
        style.background_color,
        BackgroundColor::Opaque(0x00_FF_A5_00)
    );

    let child = Element {
        tag: "span",
        id: None,
        classes: &[],
        inline_style: None,
    };

    let child_style = compute_style(&child, style, &[]);

    assert_eq!(child_style.background_color, BackgroundColor::Transparent);

    let clear = Element {
        tag: "div",
        id: None,
        classes: &[],
        inline_style: Some("background: blue; background-color: transparent"),
    };

    let clear_style = compute_style(&clear, TextStyle::initial(), &[]);

    assert_eq!(clear_style.background_color, BackgroundColor::Transparent);
}

#[test]
fn background_preview_supports_only_solid_colors_and_transparent() {
    assert_eq!(
        parse_background_color("#aBc"),
        Some(BackgroundColor::Opaque(0x00_AA_BB_CC))
    );

    assert_eq!(
        parse_background_color("rgb(0, 128, 0)"),
        Some(BackgroundColor::Opaque(0x00_00_80_00))
    );

    assert_eq!(
        parse_background_color("Transparent"),
        Some(BackgroundColor::Transparent)
    );

    assert_eq!(parse_background_color("rgba(0, 0, 0, 0.5)"), None);
    assert_eq!(parse_background_color("url(example.png)"), None);
}

#[test]
fn descendant_selector_matches_ancestors_in_order() {
    let (selector, specificity) = parse_selector("#root .box span").unwrap();

    assert_eq!(specificity, Specificity::new(1, 1, 1));

    let classes = vec!["box".to_owned()];

    let root = Element {
        tag: "main",
        id: Some("root"),
        classes: &[],
        inline_style: None,
    };

    let middle = Element {
        tag: "div",
        id: None,
        classes: &classes,
        inline_style: None,
    };

    let target = Element {
        tag: "span",
        id: None,
        classes: &[],
        inline_style: None,
    };

    assert!(selector_matches_with_ancestors(
        &selector,
        &target,
        &[root, middle]
    ));

    let root = Element {
        tag: "main",
        id: Some("root"),
        classes: &[],
        inline_style: None,
    };

    let middle = Element {
        tag: "div",
        id: None,
        classes: &classes,
        inline_style: None,
    };

    assert!(!selector_matches_with_ancestors(
        &selector,
        &target,
        &[middle, root]
    ));

    let (repeat, _) = parse_selector(".box .box span").unwrap();

    let middle = Element {
        tag: "div",
        id: None,
        classes: &classes,
        inline_style: None,
    };

    assert!(!selector_matches_with_ancestors(
        &repeat,
        &target,
        &[middle]
    ));
}

#[test]
fn descendant_selector_preserves_escapes_and_rejects_other_combinators() {
    let (selector, specificity) = parse_selector(r".\31 23 span").unwrap();

    assert_eq!(specificity, Specificity::new(0, 1, 1));
    assert_eq!(selector.ancestors[0].classes, ["123"]);
    assert_eq!(selector.tag.as_deref(), Some("span"));

    let (selector, _) = parse_selector("#a\u{00a0}").unwrap();

    assert_eq!(selector.id.as_deref(), Some("a\u{00a0}"));

    for invalid in [
        "div > span",
        "div + span",
        "div ~ span",
        "div >",
        "div span::before",
    ] {
        assert!(parse_selector(invalid).is_none(), "{invalid}");
    }
}

#[test]
fn descendant_specificity_participates_in_the_cascade() {
    let rules = build_author_rules(&["#root span { color: blue } \
             .box span { color: red }"
        .to_owned()]);

    let classes = vec!["box".to_owned()];

    let ancestors = [Element {
        tag: "div",
        id: Some("root"),
        classes: &classes,
        inline_style: None,
    }];

    let target = Element {
        tag: "span",
        id: None,
        classes: &[],
        inline_style: None,
    };

    let style = compute_style_with_ancestors(&target, TextStyle::initial(), &rules, &ancestors);

    assert_eq!(style.color, 0x00_00_00_FF);
}

#[test]
fn parses_solid_browser_subset() {
    let expected = Some(BorderEdge {
        width: 20,
        color: 0x00_00_00_FF,
    });

    assert_eq!(parse_solid_border("blue solid 20px"), expected);
    assert_eq!(parse_solid_border("20px SOLID blue"), expected);
    assert_eq!(parse_solid_border("none"), Some(BorderEdge::default()));

    for value in [
        "solid blue",
        "20px blue",
        "20px dashed blue",
        "20px solid blue orange",
        "-1px solid red",
        "20px solid rgb(0, 0, 255)",
    ] {
        assert_eq!(parse_solid_border(value), None, "{value}");
    }
}

#[test]
fn cascades_borders_without_inheriting_them() {
    let rules = build_author_rules(&["div { border-top: blue solid 20px !important }".to_owned()]);

    let parent = Element {
        tag: "div",
        id: None,
        classes: &[],
        inline_style: Some(
            "border-top: orange solid 10px; \
             border-bottom: orange solid 20px",
        ),
    };

    let style = compute_style(&parent, TextStyle::initial(), &rules);

    assert_eq!(
        style.border_top,
        BorderEdge {
            width: 20,
            color: 0x00_00_00_FF
        }
    );

    assert_eq!(
        style.border_bottom,
        BorderEdge {
            width: 20,
            color: 0x00_FF_A5_00
        }
    );

    let child = Element {
        tag: "span",
        id: None,
        classes: &[],
        inline_style: None,
    };

    let child_style = compute_style(&child, style, &[]);

    assert_eq!(child_style.border_top, BorderEdge::default());
    assert_eq!(child_style.border_bottom, BorderEdge::default());
}

#[test]
fn cascade_without_ancestors_does_not_match_descendant_selectors() {
    let rules = build_cascade_rules(&["g rect { fill: red } rect { fill: blue }".to_owned()]);

    let element = Element {
        tag: "rect",
        id: None,
        classes: &[],
        inline_style: None,
    };

    let values: Vec<_> = matching_declarations(&rules, &element)
        .iter()
        .map(|declaration| declaration.value.as_str())
        .collect();

    assert_eq!(values, ["blue"]);
}

#[test]
fn light_theme_default_text_color_is_black() {
    assert_eq!(TextStyle::initial().color, 0x00_00_00_00);
}

#[test]
fn type_selector_beats_later_universal_selector() {
    let rules =
        build_author_rules(
            &["div { color: green } * { color: red } p { color: black }".to_owned()],
        );

    for (tag, expected) in [
        ("div", 0x00_00_80_00),
        ("p", 0x00_00_00_00),
        ("span", 0x00_FF_00_00),
    ] {
        let element = Element {
            tag,
            id: None,
            classes: &[],
            inline_style: None,
        };

        let style = compute_style(&element, TextStyle::initial(), &rules);

        assert_eq!(style.color, expected, "{tag}");
    }
}

#[test]
fn audit_ignores_rules_with_empty_selector_list_items() {
    for source in [
        "body,,div { color: red }",
        ",div { color: red }",
        "div, { padding: 10px } p { color: green }",
        "div, /* comment */ ,p { color: red }",
    ] {
        assert_eq!(audit_preview_stylesheet(source), Ok(()), "{source}");
    }

    // Valid selector syntax that the current matcher cannot handle.
    assert!(audit_preview_stylesheet("div:hover, p { color: red }").is_err());

    // An unsupported declaration in a valid rule must not be ignored.
    assert!(audit_preview_stylesheet("div, p { padding: 10px }").is_err());

    // A non-breaking space is not CSS whitespace.
    assert!(audit_preview_stylesheet("div,\u{00a0},p { padding: 10px }").is_err());
}

#[test]
fn preview_audits_horizontal_margin_shorthand() {
    for value in [
        "0",
        "-0px",
        "0e2px",
        "5px",
        "0 20px",
        "5px -1px 8px",
        "0 0 0 1px",
        "5px 0 8px -0in",
    ] {
        assert!(
            audit_preview_declarations(&format!("margin: {value}")).is_ok(),
            "{value}"
        );
    }

    for value in ["0 0.001px", "0 1e-9999px", "0 0.5in", "0 auto", "0 50%"] {
        assert!(
            audit_preview_declarations(&format!("margin: {value}")).is_err(),
            "{value}"
        );
    }

    for property in ["margin-left", "margin-right"] {
        assert!(audit_preview_declarations(&format!("{property}: -20px")).is_ok());
        assert!(audit_preview_declarations(&format!("{property}: 0.1px")).is_err());
        assert!(audit_preview_declarations(&format!("{property}: auto")).is_err());
    }
}

#[test]
fn cascades_horizontal_longhands_and_shorthand() {
    let rules = build_author_rules(&[concat!(
        "div { ",
        "margin: 1px 2px 3px 4px; ",
        "margin-right: 12px !important",
        " }",
    )
    .to_owned()]);

    let element = Element {
        tag: "div",
        id: None,
        classes: &[],
        inline_style: Some(concat!(
            "margin: 5px 6px; ",
            "margin-left: -7px; ",
            "margin-left: auto; ",
            "margin-right: 8px"
        )),
    };

    let style = compute_style(&element, TextStyle::initial(), &rules);

    assert_eq!(
        (
            style.margin_before,
            style.margin_right,
            style.margin_after,
            style.margin_left
        ),
        (5, 12, 5, -7)
    );
}

#[test]
fn audit_accepts_integer_inches_and_rejects_unsupported_values() {
    assert!(audit_preview_declarations("width: 2in; height: 1in").is_ok());
    assert!(audit_preview_declarations("height: 0.5in").is_err());
    assert!(audit_preview_declarations("padding-top: 10px; padding-bottom: 1in").is_ok());
    assert!(audit_preview_declarations("padding-top: 0.1px").is_err());
    assert!(audit_preview_declarations("padding-bottom: -1px").is_err());
    assert!(audit_preview_declarations("border-left: 2px solid black").is_ok());
    assert!(audit_preview_declarations("border-right: 2px solid blue").is_ok());
    assert!(audit_preview_declarations("border-left: 2px dashed black").is_err());

    assert_eq!(
        parse_solid_border("1in solid black"),
        Some(BorderEdge {
            width: 96,
            color: 0
        })
    );
}

#[test]
fn cascades_side_borders_without_inheriting_them() {
    let rules = build_author_rules(&[concat!(
        "div { ",
        "border-left: 2px solid red !important; ",
        "border-right: 2px solid orange",
        " }",
    )
    .to_owned()]);

    let parent = Element {
        tag: "div",
        id: None,
        classes: &[],
        inline_style: Some(concat!(
            "border-left: 3px solid blue; ",
            "border-right: none; ",
            "border-right: 9px dashed red",
        )),
    };

    let style = compute_style(&parent, TextStyle::initial(), &rules);

    assert_eq!(
        style.border_left,
        BorderEdge {
            width: 2,
            color: 0x00_FF_00_00
        }
    );

    assert_eq!(style.border_right, BorderEdge::default());

    let child = Element {
        tag: "span",
        id: None,
        classes: &[],
        inline_style: None,
    };

    let child_style = compute_style(&child, style, &[]);

    assert_eq!(child_style.border_left, BorderEdge::default());
    assert_eq!(child_style.border_right, BorderEdge::default());

    assert!(audit_preview_declarations("border-left: 2px solid red").is_ok());
}

#[test]
fn strong_and_b_have_bold_default_that_author_css_can_override() {
    for tag in ["strong", "b"] {
        let element = Element {
            tag,
            id: None,
            classes: &[],
            inline_style: None,
        };

        let default_style = compute_style(&element, TextStyle::initial(), &[]);

        assert_eq!(default_style.font_weight, FontWeight::Bold);

        let rules = build_author_rules(&[format!("{tag} {{ font-weight: normal }}")]);
        let overridden = compute_style(&element, TextStyle::initial(), &rules);

        assert_eq!(overridden.font_weight, FontWeight::Normal);
    }
}
