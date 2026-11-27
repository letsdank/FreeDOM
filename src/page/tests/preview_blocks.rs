//! Experimental block preview: box geometry, margin collapsing, borders and backgrounds.

use super::*;

#[test]
fn previews_nested_block_geometry_without_changing_page_layout() {
    let page = parse_html(
        "https://example.test/",
        "<div>start<div>child</div><div>child</div>end</div>",
    );

    let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

    let rects: Vec<_> = preview
        .boxes
        .iter()
        .map(|item| (item.source.indices().to_vec(), item.rect))
        .collect();

    assert_eq!(rects.len(), 3);

    assert_eq!(rects[0].1.x, 16);
    assert_eq!(rects[0].1.width, 68);
    assert_eq!(rects[0].1.y, 0);
    assert_eq!(rects[0].1.height, 40);

    assert_eq!(rects[1].0, vec![0, 1]);
    assert_eq!(rects[1].1.y, 10);

    assert_eq!(rects[2].0, vec![0, 2]);
    assert_eq!(rects[2].1.y, 20);
}

#[test]
fn preview_reports_unsupported_content() {
    for html in [
        "<div style='margin-bottom: -5px'>text</div>",
        "<div style='margin-top: 5px'>text</div>",
        "<div><span style='padding-bottom: 5px'>text</span>",
        "<div><img></div>",
        "<div><br></div>",
        "<div><span style='font-size: 32px'>text</span></div>",
        "<div></div>",
        "<div>first</div><div></div>",
        "<div>first</div><div></div>last",
    ] {
        let page = parse_html("https://example.text", html);

        assert!(
            crate::layout_preview::PreviewLayout::build(&page, 100, None).is_err(),
            "{html}"
        );
    }
}

#[test]
fn previews_html_document_and_nonnegative_bottom_spacing() {
    let page = parse_html(
        "https://example.test/",
        concat!(
            "<!doctype html>",
            "<html>",
            "<head><title>Title</title></head>",
            "<body><p>A</p><p>B</p></body>",
            "</html>",
        ),
    );

    let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();
    let rects: Vec<_> = preview.boxes.iter().map(|item| item.rect).collect();

    // html, body, p, p
    assert_eq!(rects.len(), 4);

    assert_eq!((rects[2].y, rects[2].height), (0, 10));
    assert_eq!((rects[3].y, rects[3].height), (20, 10));

    // The final paragraph's margin collapses with body, but not with html.
    assert_eq!(rects[1].height, 30);
    assert_eq!(rects[0].height, 40);

    let text_blocks: Vec<_> = page
        .blocks
        .iter()
        .filter_map(|block| block.text())
        .collect();

    assert_eq!(text_blocks, vec!["A", "B"]);
}

#[test]
fn html_display_can_be_overridden_by_author() {
    let page = parse_html(
        "https://example.test/",
        concat!(
            "<html style='display: inline'>",
            "<body><div>x</div></body>",
            "</html>"
        ),
    );

    let result = crate::layout_preview::PreviewLayout::build(&page, 100, None);

    assert!(matches!(
        result,
        Err(crate::layout_preview::PreviewError::Tree(
            crate::box_tree::BoxTreeError::BlockInsideInline(_)
        ))
    ));
}

#[test]
fn preview_collapses_adjacent_sibling_margins() {
    for (bottom, top, expected_y) in [(8, 12, 22), (8, -3, 15), (-5, -3, 5), (0, -20, -10)] {
        let html = format!(
            "<div style='margin-bottom: {bottom}px'>A</div>\
                 <div style='margin-top: {top}px'>B</div>"
        );

        let page = parse_html("https://example.test/", &html);
        let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

        assert_eq!(preview.boxes.len(), 2);

        let first = preview.boxes[0].rect;
        let second = preview.boxes[1].rect;

        assert_eq!((first.y, first.height), (0, 10));

        assert_eq!(
            (second.y, second.height),
            (expected_y, 10),
            "bottom={bottom}, top={top}"
        );
    }
}

#[test]
fn preview_collapses_margins_through_empty_siblings() {
    let html = concat!(
        "<div style='margin-bottom: 8px'>A</div>",
        "<div style='margin-top: 12px; margin-bottom: -3px'></div>",
        "<div style='margin-top: 4px; margin-bottom: -7px'></div>",
        "<div style='margin-top: 6px'>B</div>"
    );

    let page = parse_html("https://example.test/", html);
    let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

    let positions: Vec<_> = preview
        .boxes
        .iter()
        .map(|item| (item.rect.y, item.rect.height))
        .collect();

    assert_eq!(positions, vec![(0, 10), (22, 0), (19, 0), (15, 10)]);
}

#[test]
fn preview_collapses_zero_margin_empty_sibling() {
    let page = parse_html(
        "https://example.test/",
        concat!(
            "<div>A</div>",
            "<div></div>",
            "<div style='margin-top: 5px'>B</div>",
        ),
    );

    let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();
    let positions: Vec<_> = preview.boxes.iter().map(|item| item.rect.y).collect();

    assert_eq!(positions, vec![0, 10, 15]);
}

#[test]
fn preview_collapses_parent_and_last_child_bottom_margins() {
    for (parent, child, next, expected_y) in [(4, 8, 3, 18), (8, -3, 12, 19), (-3, 8, 12, 19)] {
        let html = format!(
            "<div style='margin-bottom: {parent}px'>\
                    <div style='margin-bottom: {child}px'>A</div>\
                 </div>\
                 <div style='margin-top: {next}px'>B</div>"
        );

        let page = parse_html("https://example.test/", &html);
        let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

        let rects: Vec<_> = preview.boxes.iter().map(|item| item.rect).collect();

        assert_eq!(rects.len(), 3);

        assert_eq!((rects[0].y, rects[0].height), (0, 10));
        assert_eq!((rects[1].y, rects[1].height), (0, 10));

        assert_eq!(
            rects[2].y, expected_y,
            "parent={parent}, child={child}, next={next}"
        );
    }
}

#[test]
fn preview_keeps_margin_before_following_inline_content_inside_parent() {
    let page = parse_html(
        "https://example.test/",
        concat!(
            "<div>",
            "<div style='margin-bottom: 8px'>A</div>",
            "tail",
            "</div>",
            "<div>B</div>",
        ),
    );

    let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

    assert_eq!(preview.boxes[0].rect.height, 28);
    assert_eq!(preview.boxes[2].rect.y, 28);
}

#[test]
fn preview_collapses_parent_and_first_child_top_margins() {
    for (previous, parent, child, expected_y) in [(6, 4, 8, 18), (6, 8, -3, 15), (-5, -2, -3, 5)] {
        let html = format!(
            "<div style='margin-bottom: {previous}px'>A</div>\
                 <div style='margin-top: {parent}px'>\
                    <div style='margin-top: {child}px'>B</div>\
                 </div>"
        );

        let page = parse_html("https://example.test/", &html);
        let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

        assert_eq!(preview.boxes.len(), 3);

        let parent_rect = preview.boxes[1].rect;
        let child_rect = preview.boxes[2].rect;

        assert_eq!((parent_rect.y, parent_rect.height), (expected_y, 10));
        assert_eq!((child_rect.y, child_rect.height), (expected_y, 10));
    }
}

#[test]
fn preview_root_html_does_not_collapse_with_child_top_margin() {
    let page = parse_html(
        "https://example.test/",
        concat!(
            "<html><body>",
            "<div style='margin-top: 5px'>A</div>",
            "</body></html>",
        ),
    );

    let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

    let rects: Vec<_> = preview
        .boxes
        .iter()
        .map(|item| (item.rect.y, item.rect.height))
        .collect();

    assert_eq!(rects, vec![(0, 15), (5, 10), (5, 10)]);
}

#[test]
fn preview_does_not_collapse_child_top_past_inline_content() {
    let page = parse_html(
        "https://example.test/",
        "<div>intro<div style='margin-top: 8px'>A</div></div>",
    );

    let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();
    let parent = preview.boxes[0].rect;
    let child = preview.boxes[1].rect;

    assert_eq!((parent.y, parent.height), (0, 28));
    assert_eq!(child.y, 18);
}

#[test]
fn preview_rejects_leading_empty_child() {
    let page = parse_html(
        "https://example.test/",
        "<div>A</div><div><div></div><div>B</div></div>",
    );

    assert!(crate::layout_preview::PreviewLayout::build(&page, 100, None).is_err());
}

#[test]
fn preview_fixed_height_and_margin_cases() {
    // Case name, HTML source, and expected block positions.
    type Case<'a> = (&'a str, &'a str, &'a [(i32, i32)]);
    let cases: &[Case<'_>] = &[
        (
            "collapsed sibling margins",
            concat!(
                "<div style='height: 20px; margin-bottom: 2in'></div>",
                "<div style='height: 20px; margin-top: -2in'></div>",
            ),
            &[(0, 20), (20, 20)],
        ),
        (
            "child margin stays inside fixed height",
            concat!(
                "<div style='height: 20px'>",
                "<div style='height: 5px; margin-bottom: 8px'></div>",
                "</div>",
                "<div style='height: 10px; margin-top: 3px'></div>"
            ),
            &[(0, 20), (0, 5), (23, 10)],
        ),
        (
            "content overflows fixed height",
            concat!(
                "<div style='height: 5px'>",
                "<div style='height: 20px'></div>",
                "</div>",
                "<div style='height: 10px'></div>"
            ),
            &[(0, 5), (0, 20), (5, 10)],
        ),
        (
            "zero height remains collapsible",
            concat!(
                "<div style='height: 10px; margin-bottom: 8px'></div>",
                "<div style='height: 0; margin-top: 12px; ",
                "margin-bottom: -3px'></div>",
                "<div style='height: 10px; margin-top: 6px'></div>"
            ),
            &[(0, 10), (22, 0), (19, 10)],
        ),
    ];

    for &(name, html, expected) in cases {
        let page = parse_html("https://example.test/", html);

        let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None)
            .unwrap_or_else(|error| panic!("{name}: {error}"));

        let rects: Vec<_> = preview
            .boxes
            .iter()
            .map(|item| (item.rect.y, item.rect.height))
            .collect();

        assert_eq!(rects.as_slice(), expected, "{name}");
    }
}

#[test]
fn preview_uses_block_width_for_descendants_and_allows_overflow() {
    let page = parse_html(
        "https://example.test/",
        concat!(
            "<div style='width: 50px'>",
            "<div style='height: 5px'></div>",
            "<div style='width: 90px; height: 5px'></div>",
            "</div>",
            "<div style='height: 5px'></div>"
        ),
    );

    let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

    let rects: Vec<_> = preview
        .boxes
        .iter()
        .map(|item| (item.rect.width, item.rect.height))
        .collect();

    assert_eq!(rects, vec![(50, 10), (50, 5), (90, 5), (68, 5)]);
}

#[test]
fn preview_wraps_text_using_the_parent_width() {
    let page = parse_html(
        "https://example.test/",
        "<div style='width: 16px'><div>a b c</div></div>",
    );

    let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

    let parent = preview.boxes[0].rect;
    let child = preview.boxes[1].rect;

    assert_eq!((parent.width, parent.height), (16, 30));
    assert_eq!((child.width, child.height), (16, 30));
}

#[test]
fn preview_keeps_zero_width_and_positive_height() {
    let page = parse_html(
        "https://example.test/",
        "<div style='width: 0; height: 10px'></div>",
    );

    let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();
    let rect = preview.boxes[0].rect;

    assert_eq!((rect.width, rect.height), (0, 10));
}

#[test]
fn preview_wpt_inspired_two_fixed_stripes() {
    let page = parse_html(
        "https://example.test/",
        concat!(
            "<div>",
            "<div style='width: 50px; height: 20px; ",
            "margin-bottom: 2in'></div>",
            "<div style='width: 50px; height: 20px; ",
            "margin-top: -2in'></div>",
            "</div>",
        ),
    );

    let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

    let rects: Vec<_> = preview
        .boxes
        .iter()
        .map(|item| (item.rect.y, item.rect.width, item.rect.height))
        .collect();

    assert_eq!(rects, vec![(0, 68, 40), (0, 50, 20), (20, 50, 20)]);
}

#[test]
fn preview_paints_backgrounds_from_cascaded_page_styles() {
    let page = parse_html(
        "https://example.test/",
        concat!(
            "<style>",
            "#a { background: blue }",
            "#b { background-color: orange }",
            "</style>",
            "<div>",
            "<div id='a' style='width: 50px; height: 20px; ",
            "margin-bottom: 2in'></div>",
            "<div id='b' style='width: 50px; height: 20px; ",
            "margin-top: -2in'></div>",
            "</div>"
        ),
    );

    let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();
    let mut pixels = vec![0; 100 * 140];

    crate::layout_debug::paint_preview_backgrounds(
        &mut XrgbFrame::new(&mut pixels, 100, 140).unwrap(),
        &preview,
        0,
    );

    assert_eq!(pixels[90 * 100 + 16], 0x00_00_00_FF);
    assert_eq!(pixels[109 * 100 + 65], 0x00_00_00_FF);

    assert_eq!(pixels[110 * 100 + 16], 0x00_FF_A5_00);
    assert_eq!(pixels[129 * 100 + 65], 0x00_FF_A5_00);

    assert_eq!(pixels[130 * 100 + 16], crate::render::BACKGROUND);
}

#[test]
fn descendant_rules_reach_old_render_and_new_background_preview() {
    let page = parse_html(
        "https://example.test/",
        concat!(
            "<style>",
            "div div { color: red; background: blue; ",
            "width: 50px; height: 20px }",
            "</style>",
            "<div><section><div>Inside</div></section></div>",
            "<div>Outside</div>"
        ),
    );

    let inside = page
        .blocks
        .iter()
        .find(|block| block.text() == Some("Inside"))
        .unwrap();

    let outside = page
        .blocks
        .iter()
        .find(|block| block.text() == Some("Outside"))
        .unwrap();

    assert_eq!(inside.style.color, 0x00_FF_00_00);
    assert_ne!(outside.style.color, 0x00_FF_00_00);

    let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

    let rect = preview.boxes[2].rect;

    assert_eq!((rect.width, rect.height), (50, 20));
    assert_eq!(
        preview.boxes[2].background_color,
        freedom_css::BackgroundColor::Opaque(0x00_00_00_FF)
    );

    let mut pixels = vec![0; 100 * 140];

    crate::layout_debug::paint_preview_backgrounds(
        &mut XrgbFrame::new(&mut pixels, 100, 140).unwrap(),
        &preview,
        0,
    );

    assert_eq!(pixels[90 * 100 + 16], 0x00_00_00_FF);
    assert_eq!(pixels[110 * 100 + 16], crate::render::BACKGROUND);
}

#[test]
fn preview_paints_vertical_border_reference_like_stripes() {
    let target = parse_html(
        "https://example.test/",
        concat!(
            "<div style='width: 50px; ",
            "border-top: blue solid 20px; ",
            "border-bottom: orange solid 20px'></div>"
        ),
    );

    let reference = parse_html(
        "https://example.test/",
        concat!(
            "<div style='width: 50px; height: 20px; ",
            "background: blue'></div>",
            "<div style='width: 50px; height: 20px; ",
            "background: orange'></div>"
        ),
    );

    let target_layout = crate::layout_preview::PreviewLayout::build(&target, 100, None).unwrap();

    let reference_layout =
        crate::layout_preview::PreviewLayout::build(&reference, 100, None).unwrap();

    assert_eq!(
        (
            target_layout.boxes[0].rect.width,
            target_layout.boxes[0].rect.height
        ),
        (50, 40)
    );

    let mut actual = vec![0; 100 * 140];
    let mut expected = vec![0; 100 * 140];

    crate::layout_debug::paint_preview_backgrounds(
        &mut XrgbFrame::new(&mut actual, 100, 140).unwrap(),
        &target_layout,
        0,
    );

    crate::layout_debug::paint_preview_backgrounds(
        &mut XrgbFrame::new(&mut expected, 100, 140).unwrap(),
        &reference_layout,
        0,
    );

    assert_eq!(actual, expected);
    assert_eq!(actual[90 * 100 + 16], 0x00_00_00_FF);
    assert_eq!(actual[110 * 100 + 16], 0x00_FF_A5_00);
}

#[test]
fn preview_borders_stop_parent_child_margin_collapsing() {
    let page = parse_html(
        "https://example.test/",
        concat!(
            "<div style='border-top: red solid 2px; ",
            "border-bottom: blue solid 3px'>",
            "<div style='height: 5px; margin-top: 8px; ",
            "margin-bottom: 6px'></div>",
            "</div>",
            "<div style='height: 10px'></div>"
        ),
    );

    let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

    let rects: Vec<_> = preview
        .boxes
        .iter()
        .map(|item| (item.rect.y, item.rect.height))
        .collect();

    assert_eq!(rects, vec![(0, 24), (10, 5), (24, 10)]);
}

#[test]
fn preview_padding_contains_nested_margins_and_extends_background() {
    let page = parse_html(
        "https://example.test/",
        concat!(
            "<div style='padding-top:10px; padding-bottom:15px; background:blue'>",
            "<div style='height:5px; margin-top:8px; margin-bottom:6px; background:orange'></div>",
            "</div><div style='height:10px'></div>"
        ),
    );
    let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();
    let rects: Vec<_> = preview.boxes.iter().map(|item| item.rect).collect();
    assert_eq!((rects[0].y, rects[0].height), (0, 44));
    assert_eq!((rects[1].y, rects[1].height), (18, 5));
    assert_eq!((rects[2].y, rects[2].height), (44, 10));

    let mut pixels = vec![0; 100 * 150];
    crate::layout_debug::paint_preview_backgrounds(
        &mut XrgbFrame::new(&mut pixels, 100, 150).unwrap(),
        &preview,
        0,
    );
    assert_eq!(pixels[90 * 100 + 16], 0x00_00_00_FF);
    assert_eq!(pixels[108 * 100 + 16], 0x00_FF_A5_00);
    assert_eq!(pixels[133 * 100 + 16], 0x00_00_00_FF);

    let fixed = parse_html(
        "https://example.test/",
        "<div style='height:7px; padding-top:3px; padding-bottom:4px'></div>",
    );
    let fixed = crate::layout_preview::PreviewLayout::build(&fixed, 100, None).unwrap();
    assert_eq!(fixed.boxes[0].rect.height, 14);
}

#[test]
fn preview_rejects_border_size_overflow() {
    let page = parse_html(
        "https://example.test/",
        concat!(
            "<div style='height: 2147483647px; ",
            "border-top: red solid 1px'></div>"
        ),
    );

    assert!(matches!(
        crate::layout_preview::PreviewLayout::build(&page, 100, None),
        Err(crate::layout_preview::PreviewError::SizeOverflow)
    ));
}

#[test]
fn saved_xhtml_pair_has_matching_background_only_preview_not_a_wpt_result() {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/wpt/css/CSS2/margin-padding-clear");

    let read =
        |name: &str| std::fs::read_to_string(base.join(name)).expect("WPT fixture must exist");

    let target = parse_xhtml_for_test(
        "https://web-platform.test/css/CSS2/margin-padding-clear/margin-collapse-003.xht",
        &read("margin-collapse-003.xht"),
    )
    .unwrap();

    let reference = parse_xhtml_for_test(
        "https://web-platform.test/css/CSS2/margin-padding-clear/margin-collapse-003-ref.xht",
        &read("margin-collapse-003-ref.xht"),
    )
    .unwrap();

    let (width, height) = (320, 200);

    let target_layout = crate::layout_preview::PreviewLayout::build(&target, width, None).unwrap();

    let reference_layout =
        crate::layout_preview::PreviewLayout::build(&reference, width, None).unwrap();

    let mut actual = vec![0; width * height];
    let mut expected = vec![0; width * height];

    crate::layout_debug::paint_preview_backgrounds(
        &mut XrgbFrame::new(&mut actual, width, height).unwrap(),
        &target_layout,
        0,
    );

    crate::layout_debug::paint_preview_backgrounds(
        &mut XrgbFrame::new(&mut expected, width, height).unwrap(),
        &reference_layout,
        0,
    );

    let blue_pixels = actual
        .iter()
        .filter(|&&pixel| pixel == 0x00_00_00_FF)
        .count();

    let orange_pixels = actual
        .iter()
        .filter(|&&pixel| pixel == 0x00_FF_A5_00)
        .count();

    assert_eq!(blue_pixels, 50 * 20);
    assert_eq!(orange_pixels, 50 * 20);

    assert_eq!(
        actual, expected,
        "background-only diagnostic does not render text or run WPT"
    );
}

#[test]
fn saved_xhtml_pair_has_matching_experimental_scene_not_a_wpt_result() {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/wpt/css/CSS2/margin-padding-clear");

    let read =
        |name: &str| std::fs::read_to_string(base.join(name)).expect("WPT fixture must exist");

    let target = parse_xhtml_for_test(
        "https://web-platform.test/css/CSS2/margin-padding-clear/margin-collapse-003.xht",
        &read("margin-collapse-003.xht"),
    )
    .unwrap();

    let reference = parse_xhtml_for_test(
        "https://web-platform.test/css/CSS2/margin-padding-clear/margin-collapse-003-ref.xht",
        &read("margin-collapse-003-ref.xht"),
    )
    .unwrap();

    let (width, height) = (320, 200);

    let target_layout = crate::layout_preview::PreviewLayout::build(&target, width, None).unwrap();

    let reference_layout =
        crate::layout_preview::PreviewLayout::build(&reference, width, None).unwrap();

    let mut actual = vec![0; width * height];
    let mut expected = vec![0; width * height];
    let mut cache = crate::render::GlyphRasterCache::new();

    crate::render::paint_preview_scene(
        &mut XrgbFrame::new(&mut actual, width, height).unwrap(),
        &target,
        &target_layout,
        None,
        0,
        &mut cache,
    );

    crate::render::paint_preview_scene(
        &mut XrgbFrame::new(&mut expected, width, height).unwrap(),
        &reference,
        &reference_layout,
        None,
        0,
        &mut cache,
    );

    assert!(
        actual[crate::page_layout::CONTENT_TOP as usize * width..].contains(&0x00_00_00_00),
        "expected test pixels"
    );

    assert_eq!(
        actual
            .iter()
            .filter(|&&pixel| pixel == 0x00_00_00_FF)
            .count(),
        50 * 20
    );

    assert_eq!(
        actual
            .iter()
            .filter(|&&pixel| pixel == 0x00_FF_A5_00)
            .count(),
        50 * 20
    );

    assert_eq!(actual, expected, "experimental preview is not a WPT result");
}

#[test]
fn preview_positions_nested_horizontal_margins_and_text() {
    let page = parse_html(
        "https://example.test/",
        concat!(
            "<div style='width:60px;margin-left:10px'>",
            "<div style='margin-left:8px;margin-right:12px'>",
            "text",
            "</div>",
            "</div>",
        ),
    );

    let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

    assert_eq!(
        (preview.boxes[0].rect.x, preview.boxes[0].rect.width),
        (26, 60)
    );

    assert_eq!(
        (preview.boxes[1].rect.x, preview.boxes[1].rect.width),
        (34, 40)
    );

    let rect = preview
        .display_list
        .iter()
        .find_map(|command| match command {
            crate::layout_preview::PreviewCommand::Text { rect, .. } => Some(rect),
            _ => None,
        })
        .unwrap();

    assert_eq!((rect.x, rect.width), (34, 40));
}

#[test]
fn preview_paints_background_at_nested_horizontal_position() {
    let page = parse_html(
        "https://example.test/",
        concat!(
            "<div style='width:60px;margin-left:10px'>",
            "<div style='height:10px;margin-left:8px;",
            "margin-right:12px;background:blue'></div>",
            "</div>"
        ),
    );

    let preview = crate::layout_preview::PreviewLayout::build(&page, 132, None).unwrap();

    let mut pixels = vec![0; 132 * 110];
    let mut cache = crate::render::GlyphRasterCache::new();

    crate::render::paint_preview_scene(
        &mut XrgbFrame::new(&mut pixels, 132, 110).unwrap(),
        &page,
        &preview,
        None,
        0,
        &mut cache,
    );

    assert_eq!(pixels[90 * 132 + 33], crate::render::BACKGROUND);
    assert_eq!(pixels[90 * 132 + 34], 0x00_00_00_FF);
    assert_eq!(pixels[90 * 132 + 73], 0x00_00_00_FF);
    assert_eq!(pixels[90 * 132 + 74], crate::render::BACKGROUND);
}

#[test]
fn preview_handles_negative_and_overconstrained_horizontal_margins() {
    let page = parse_html(
        "https://example.test/",
        concat!(
            "<div style='height:10px;margin-left:-10px;",
            "margin-right:-5px'></div>",
            "<div style='height:10px;margin-left:50px;",
            "margin-right:50px'></div>"
        ),
    );

    let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

    assert_eq!(
        (preview.boxes[0].rect.x, preview.boxes[0].rect.width),
        (6, 83)
    );

    assert_eq!(
        (preview.boxes[1].rect.x, preview.boxes[1].rect.width),
        (66, 0)
    );
}

#[test]
fn preview_rejects_horizontal_overflow_and_inline_margins() {
    let overflow = parse_html(
        "https://example.test/",
        "<div style='height:10px;margin-left:2147483647px'></div>",
    );

    assert!(matches!(
        crate::layout_preview::PreviewLayout::build(&overflow, 100, None),
        Err(crate::layout_preview::PreviewError::SizeOverflow)
    ));

    let inline = parse_html(
        "https://example.test/",
        "<div>A<span style='margin-left:5px'>B</span></div>",
    );

    assert!(matches!(
        crate::layout_preview::PreviewLayout::build(&inline, 100, None),
        Err(crate::layout_preview::PreviewError::InlineStyle(_))
    ));
}

#[test]
fn preview_renders_one_inch_height_as_96_pixels() {
    let page = parse_html(
        "https://example.test/",
        "<div style='height:1in;width:20px;background:blue'></div>",
    );

    let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

    assert_eq!(preview.boxes[0].rect.height, 96);

    let mut pixels = vec![0; 100 * 200];
    let mut cache = crate::render::GlyphRasterCache::new();

    crate::render::paint_preview_scene(
        &mut XrgbFrame::new(&mut pixels, 100, 200).unwrap(),
        &page,
        &preview,
        None,
        0,
        &mut cache,
    );

    assert_eq!(pixels[185 * 100 + 16], 0x00_00_00_FF);
    assert_eq!(pixels[186 * 100 + 16], crate::render::BACKGROUND);
}

#[test]
fn preview_rejects_mixed_side_and_vertical_borders() {
    let page = parse_html(
        "https://example.test/",
        concat!(
            "<div style='height:10px;",
            "border-left:2px solid red;",
            "border-top:2px solid blue'></div>",
        ),
    );

    assert!(matches!(
        crate::layout_preview::PreviewLayout::build(&page, 100, None),
        Err(crate::layout_preview::PreviewError::SideBorder(_))
    ));

    let inline = parse_html(
        "https://example.test/",
        "<div>A<span style='border-left:2px solid red'>B</span></div>",
    );

    assert!(matches!(
        crate::layout_preview::PreviewLayout::build(&inline, 100, None),
        Err(crate::layout_preview::PreviewError::InlineStyle(_))
    ));

    let no_border = parse_html(
        "https://example.test/",
        "<div style='height:10px;border-left:none'></div>",
    );

    assert!(crate::layout_preview::PreviewLayout::build(&no_border, 100, None).is_ok());
}

#[test]
fn preview_positions_and_paints_side_borders() {
    let page = parse_html(
        "https://example.test/",
        concat!(
            "<div style='width:20px;margin-left:5px;",
            "border-left:2px solid red;",
            "border-right:3px solid blue'>",
            "<div style='height:10px;background:green'></div>",
            "</div>"
        ),
    );

    let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

    assert_eq!(
        (preview.boxes[0].rect.x, preview.boxes[0].rect.width),
        (21, 25)
    );

    assert_eq!(
        (preview.boxes[1].rect.x, preview.boxes[1].rect.width),
        (23, 20)
    );

    let mut pixels = vec![0; 100 * 110];
    let mut cache = crate::render::GlyphRasterCache::new();

    crate::render::paint_preview_scene(
        &mut XrgbFrame::new(&mut pixels, 100, 110).unwrap(),
        &page,
        &preview,
        None,
        0,
        &mut cache,
    );

    let row = 90 * 100;

    assert_eq!(pixels[row + 20], crate::render::BACKGROUND);
    assert_eq!(pixels[row + 21], 0x00_FF_00_00);
    assert_eq!(pixels[row + 22], 0x00_FF_00_00);
    assert_eq!(pixels[row + 23], 0x00_00_80_00);
    assert_eq!(pixels[row + 42], 0x00_00_80_00);
    assert_eq!(pixels[row + 43], 0x00_00_00_FF);
    assert_eq!(pixels[row + 45], 0x00_00_00_FF);
    assert_eq!(pixels[row + 46], crate::render::BACKGROUND);
}

#[test]
fn preview_subtracts_side_borders_from_auto_content_width() {
    let page = parse_html(
        "https://example.test/",
        concat!(
            "<div style='margin-left:3px;margin-right:5px;",
            "border-left:2px solid red;",
            "border-right:4px solid blue'>",
            "<div style='height:10px'></div>",
            "</div>"
        ),
    );

    let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

    assert_eq!(
        (preview.boxes[0].rect.x, preview.boxes[0].rect.width),
        (19, 60)
    );

    assert_eq!(
        (preview.boxes[1].rect.x, preview.boxes[1].rect.width),
        (21, 54)
    );
}

#[test]
fn preview_rejects_side_border_geometry_overflow() {
    let page = parse_html(
        "https://example.test/",
        concat!(
            "<div style='height:10px;",
            "border-left:2147483647px solid red'></div>",
        ),
    );

    assert!(matches!(
        crate::layout_preview::PreviewLayout::build(&page, 100, None),
        Err(crate::layout_preview::PreviewError::SizeOverflow)
    ));
}
