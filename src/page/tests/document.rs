//! Block layout, the style cascade, text blocks and inline scripts.

use super::*;

#[test]
fn line_height_reaches_the_main_page_layout() {
    use crate::{
        page_layout::{CONTENT_TOP, PageLayout},
        render::{BACKGROUND, BlockRasterCache, GlyphRasterCache, paint},
    };

    let page = parse_html(
        "https://example.test",
        "<div style='font-size:16px;line-height:20px'>one one</div>",
    );
    let layout = PageLayout::build(&page, 80, None);
    let runs = layout
        .block_text_runs(0)
        .expect("explicit line-height uses inline layout");
    assert_eq!(layout.block_height(0), 40);
    assert_eq!(runs.len(), 2);
    assert_eq!((runs[0].y, runs[1].y), (6, 26));
    let (width, height) = (80, 160);
    let mut pixels = vec![0; width * height];
    paint(
        &mut XrgbFrame::new(&mut pixels, width, height).unwrap(),
        &page,
        &layout,
        PaintOptions::default(),
        &mut BlockRasterCache::new(),
        &mut GlyphRasterCache::new(),
    );
    let first_y = CONTENT_TOP as usize + 6;
    let mut ink = false;
    for row in 0..8 {
        let first = &pixels[(first_y + row) * width + 16..(first_y + row) * width + 40];
        let second = &pixels[(first_y + 20 + row) * width + 16..(first_y + 20 + row) * width + 40];
        assert_eq!(first, second);
        ink |= first.iter().any(|&pixel| pixel != BACKGROUND);
    }
    assert!(ink);

    let page = parse_html(
        "https://example.test",
        "<div style='font-size:16px;line-height:20px'>one <span style='font-size:24px;line-height:40px'>two</span></div>",
    );
    let layout = PageLayout::build(&page, 300, None);
    let runs = layout.block_text_runs(0).unwrap();
    assert_eq!(layout.block_height(0), 40);
    assert_eq!(runs.len(), 2);
    // Fallback bitmap glyphs use baselines at 8px and 16px respectively.
    assert_eq!(runs[0].y + 8, runs[1].y + 16);

    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/fonts/noto/nko-hmtx.woff2");
    let font_bytes = normalize_font_container(std::fs::read(path).unwrap()).unwrap();
    let font = crate::graphics::font::TrueTypeFont::parse(&font_bytes).unwrap();
    let layout = PageLayout::build(&page, 300, Some(&font_bytes));
    let runs = layout.block_text_runs(0).unwrap();
    assert_eq!(runs.len(), 2);
    let baseline = |run: &crate::inline_layout::PositionedTextRun| {
        run.y as f32 + run.offset_y + font.line_metrics(run.style.font_size).unwrap().ascent
    };
    assert!((baseline(&runs[0]) - baseline(&runs[1])).abs() <= 0.125);

    let page = parse_html(
        "https://example.test",
        "<div style='line-height:0'>A B</div>",
    );
    let layout = PageLayout::build(&page, 48, None);
    let runs = layout.block_text_runs(0).unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].y, runs[1].y);
    assert_eq!((runs[0].line_index, runs[1].line_index), (0, 1));
}

#[test]
fn vertical_padding_moves_text_and_the_next_block() {
    use crate::{
        page_layout::{CONTENT_TOP, PageLayout},
        render::{BlockRasterCache, GlyphRasterCache, paint},
    };

    for line_height in ["", "line-height:20px;"] {
        let html = |padding| format!("<div style='{line_height}{padding}'>A</div><div>B</div>");
        let plain = parse_html("https://example.test", &html(""));
        let padded = parse_html(
            "https://example.test",
            &html("padding-top:10px;padding-bottom:5px"),
        );
        let plain_layout = PageLayout::build(&plain, 300, None);
        let padded_layout = PageLayout::build(&padded, 300, None);
        assert_eq!(
            padded_layout.block_height(0),
            plain_layout.block_height(0) + 15
        );
        assert_eq!(padded_layout.block_top(1), plain_layout.block_top(1) + 15);

        let (width, height) = (300, 180);
        let render = |page: &Page, layout: &PageLayout| {
            let mut pixels = vec![0; width * height];
            paint(
                &mut XrgbFrame::new(&mut pixels, width, height).unwrap(),
                page,
                layout,
                PaintOptions::default(),
                &mut BlockRasterCache::new(),
                &mut GlyphRasterCache::new(),
            );
            pixels
        };
        let plain_pixels = render(&plain, &plain_layout);
        let padded_pixels = render(&padded, &padded_layout);
        let first = CONTENT_TOP as usize;
        let text_height = plain_layout.block_height(0) as usize;
        assert_eq!(
            &plain_pixels[first * width..(first + text_height) * width],
            &padded_pixels[(first + 10) * width..(first + 10 + text_height) * width]
        );
    }
}

#[test]
fn negative_margin_does_not_hide_an_earlier_line_from_scroll_extent() {
    use crate::{
        page_layout::{CONTENT_TOP, PageLayout},
        render::{BlockRasterCache, GlyphRasterCache, paint},
    };

    let page = parse_html(
        "https://example.test",
        "<div style='line-height:20px;margin-bottom:-15px'>A</div>\
            <div style='line-height:0'>B</div>",
    );
    let layout = PageLayout::build(&page, 300, None);
    assert_eq!(layout.block_height(0), 20);
    assert_eq!(layout.block_top(1), 5);
    assert_eq!(layout.content_height(), 20);

    let (width, height) = (300, CONTENT_TOP as usize + 10);
    let result = paint(
        &mut XrgbFrame::new(&mut vec![0; width * height], width, height).unwrap(),
        &page,
        &layout,
        PaintOptions::default(),
        &mut BlockRasterCache::new(),
        &mut GlyphRasterCache::new(),
    );
    assert_eq!(result.max_scroll_y, 10);
}

#[test]
fn inline_style_overrides_author_rules() {
    let page = parse_html(
        "https://example.test",
        r#"
                <style>
                    p { color: #ff0000; }
                    .note { color: blue; font-size: 24px; }
                    #first { color: #00ff00; }
                </style>
                <p id="first" class="note" style="color: #123456">Hello</p>
            "#,
    );

    assert_eq!(page.blocks.len(), 1);
    assert_eq!(page.blocks[0].text().unwrap(), "Hello");
    assert_eq!(page.blocks[0].style.color, 0x00_12_34_56);
    assert!((page.blocks[0].style.font_size - 24.0).abs() < f32::EPSILON);
}

#[test]
fn display_none_removes_content_from_layout() {
    let page = parse_html(
        "https://example.test",
        r#"
                <style>.hidden { display: none; }</style>
                <p>Visible text</p>
                <p class="hidden">Invisible text</p>
            "#,
    );

    assert_eq!(page.blocks.len(), 1);
    assert_eq!(page.blocks[0].text().unwrap(), "Visible text");
}

#[test]
fn inline_javascript_updates_the_page_title() {
    let page = parse_html(
        "https://example.test/",
        r#"
                <title>Before</title>
                <script>
                    document.title = "FreeDOM script test";
                    assert_equals(document.title, "FreeDOM script test");
                </script>
                <p>Visible content</p>
            "#,
    );

    assert_eq!(page.title, "FreeDOM script test");
    assert_eq!(page.blocks.len(), 1);
    assert_eq!(page.blocks[0].text().unwrap(), "Visible content");
}

#[test]
fn inline_javascript_can_read_an_element_from_the_document() {
    let page = parse_html(
        "https://example.test/",
        r#"
                <title>Before</title>
                <h1 id="welcome">Welcome <em>home</em></h1>
                <script>
                    const heading = document.getElementById("welcome");

                    assert_equals(heading.tagName, "H1");
                    assert_equals(heading.textContent, "Welcome home");

                    document.title = heading.textContent;
                </script>
            "#,
    );

    assert_eq!(page.title, "Welcome home");
}

#[test]
fn javascript_text_content_updates_layout() {
    let page = parse_html(
        "https://example.test/",
        r#"
                <h1 id="welcome">Before</h1>
                <script>
                    const heading = document.getElementById("welcome");

                    heading.textContent = "After";
                    assert_equals(heading.textContent, "After");

                    document.title = heading.textContent;
                </script>
            "#,
    );

    assert_eq!(page.title, "After");
    assert_eq!(page.blocks.len(), 1);
    assert_eq!(page.blocks[0].text().unwrap(), "After");
}

#[test]
fn keeps_inline_svg_in_document_flow() {
    let page = parse_html(
        "https://example.test",
        r##"
                <p>Before</p>
                <svg viewBox="0 0 10 10">
                    <path fill="#ff0000" d="M 0 0 H 10 V 10 H 0 Z"/>
                </svg>
                <p>After</p>
            "##,
    );

    assert_eq!(page.blocks.len(), 3);
    assert_eq!(page.blocks[0].text().unwrap(), "Before");
    assert!(page.blocks[1].svg().is_some());
    assert_eq!(page.blocks[2].text().unwrap(), "After");
}

#[test]
fn compound_css_selectors_participate_in_the_cascade() {
    let page = parse_html(
        "https://example.test/",
        r#"
                <style>
                    p { color: red; font-size: 12px; }
                    .notice { color: green; }
                    p.notice { color: #123456; font-size: 20px; }
                    p#lead.notice { font-weight: 700; font-style: italic; }
                </style>

                <p id="lead" class="notice">FreeDOM</p>
            "#,
    );

    let style = page.blocks[0].style.clone();

    assert_eq!(page.blocks[0].text(), Some("FreeDOM"));
    assert_eq!(style.color, 0x00_12_34_56);
    assert_eq!(style.font_size, 20.0);
    assert_eq!(style.font_weight, FontWeight::Bold);
    assert_eq!(style.font_style, FontStyle::Italic);
}

#[test]
fn inline_style_keeps_semicolon_inside_font_family() {
    let page = parse_html(
        "https://example.test",
        r#"<p style="font-family: 'Nko; Test'; color: green">Hello</p>"#,
    );

    assert_eq!(
        page.blocks[0]
            .style
            .font_families
            .first()
            .map(String::as_str),
        Some("Nko; Test")
    );

    assert_eq!(page.blocks[0].style.color, 0x00_00_80_00);
}

#[test]
fn preprocesses_css_in_stylesheet_and_inline_style() {
    let stylesheet = parse_html(
        "https://example.test",
        "<style>p { font-family: 'A\0B' }</style><p>Hello</p>",
    );

    let inline = parse_html(
        "https://example.test",
        "<p style=\"font-family: 'A\0B'\">Hello</p>",
    );

    assert_eq!(
        stylesheet.blocks[0]
            .style
            .font_families
            .first()
            .map(String::as_str),
        Some("A\u{FFFD}B")
    );

    assert_eq!(
        inline.blocks[0]
            .style
            .font_families
            .first()
            .map(String::as_str),
        Some("A\u{FFFD}B")
    );
}

#[test]
fn currentcolor_flows_through_page_styles() {
    let page = parse_html(
        "https://example.test",
        r#"
                <style>
                    div { color: red }
                    p { color: green; color: currentcolor }
                </style>
                <div><p>Hello</p></div>
            "#,
    );

    assert_eq!(page.blocks[0].style.color, 0x00_FF_00_00);
}

#[test]
fn inline_scripts_share_global_bindings_and_continue_after_errors() {
    let page = parse_html(
        "https://example.test/",
        r#"
                <title>Before</title>
                <script>
                    let prefix = 'Hello';
                    throw 'failure';
                </script>
                <script>
                    document.title = prefix + ' world';
                </script>
            "#,
    );

    assert_eq!(page.title, "Hello world");
}

#[test]
fn collapses_whitespace_across_inline_text_nodes() {
    for (html, expected) in [
        ("<div>Free<strong>DOM</strong>!</div>", "FreeDOM!"),
        ("<div>one <span>  two</span> three</div>", "one two three"),
        ("<div>one<span> </span><em></em>two</div>", "one two"),
        ("<div>  a\n<span>\tb</span>  </div>", "a b"),
        ("<div>\u{a0}a\u{a0}b\u{a0}</div>", "\u{a0}a\u{a0}b\u{a0}"),
    ] {
        let page = parse_html("https://example.test/", html);

        assert_eq!(page.blocks.len(), 1, "{html}");
        assert_eq!(page.blocks[0].text(), Some(expected), "{html}");
    }

    let page = parse_html(
        "https://example.test/",
        "<div>A <span> </span></div><div>B</div>",
    );

    let blocks: Vec<_> = page.blocks.iter().filter_map(Block::text).collect();

    assert_eq!(blocks, ["A", "B"]);
}

#[test]
fn preserves_inline_styles_and_utf8_ranges_in_text_blocks() {
    let page = parse_html(
        "https://example.test/",
        "<p>A<strong>Б</strong><span style='color:red'> C</span></p>",
    );

    let BlockContent::Text(text) = &page.blocks[0].content else {
        panic!("expected text block");
    };

    assert_eq!(text.text, "AБ C");

    let pieces: Vec<_> = text
        .runs
        .iter()
        .map(|run| &text.text[run.range.clone()])
        .collect();

    assert_eq!(pieces, ["A", "Б", " C"]);
    assert_eq!(text.runs[0].range.start, 0);

    for pair in text.runs.windows(2) {
        assert_eq!(pair[0].range.end, pair[1].range.start);
    }

    assert_eq!(text.runs.last().unwrap().range.end, text.text.len());
    assert!(text.runs.iter().all(|run| run.link.is_none()));
    assert_eq!(text.runs[1].style.font_weight, FontWeight::Bold);
    assert_eq!(text.runs[2].style.color, 0x00_ff_00_00);

    let description = page.describe_block_sources(0).unwrap();

    assert!(description.contains("weight=Bold"));
    assert!(description.contains("color=#ff0000"));

    let page = parse_html("https://example.test/", "<a href='/'>link</a>");

    let BlockContent::Text(text) = &page.blocks[0].content else {
        panic!("expected text block");
    };

    assert_eq!(text.runs.len(), 1);
    assert_eq!(&text.text[text.runs[0].range.clone()], "link");

    assert_eq!(text.runs[0].link.as_deref(), Some("https://example.test/"));

    let page = parse_html(
        "https://example.test/",
        "<a href='/next'>one <strong>two</strong></a>",
    );

    let BlockContent::Text(text) = &page.blocks[0].content else {
        panic!("expected text block");
    };

    assert_eq!(text.runs.len(), 2);

    assert!(
        text.runs
            .iter()
            .all(|run| run.link.as_deref() == Some("https://example.test/next"))
    );

    assert!(
        page.describe_block_sources(0)
            .unwrap()
            .contains("link=Some(\"https://example.test/next\")"),
    );

    let page = parse_html(
        "https://example.test/",
        "<a href='javascript:alert(1)'>unsafe</a>",
    );

    let BlockContent::Text(text) = &page.blocks[0].content else {
        panic!("expected text block");
    };

    assert!(text.runs.iter().all(|run| run.link.is_none()));
}
