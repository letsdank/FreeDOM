use super::*;
use crate::page_layout::measure_wrapped_text;

/// Black regular text without vector fonts, drawn with the bitmap font.
fn bitmap_style(font_size: f32) -> TextRunStyle<'static> {
    TextRunStyle {
        font_size,
        font_weight: FontWeight::Normal,
        font_style: FontStyle::Normal,
        fallback_weight: FontWeight::Normal,
        color: 0,
        font: None,
        font_stack: &[],
    }
}

#[test]
fn line_height_zero_paints_outside_block_and_clips_to_content() {
    let page = crate::page::parse_xhtml_for_test(
        "https://example.test/",
        "<html xmlns='http://www.w3.org/1999/xhtml'><body><div style='line-height:20px'>header</div><div style='line-height:0'><a href='/x' style='color:red'>XXXX</a></div><div style='color:blue'>i</div></body></html>"
    ).unwrap();
    let (width, height) = (128, 150);
    let layout = PageLayout::build(&page, width, None);
    assert!(layout.has_text_overflow());
    assert_eq!(layout.block_height(1), 0);
    let (top, bottom) = layout.block_paint_bounds(1);
    assert!(top < 0 && bottom > 0);
    let mut pixels = vec![0; width * height];
    let mut cache = BlockRasterCache::new();
    let result = paint(
        &mut XrgbFrame::new(&mut pixels, width, height).unwrap(),
        &page,
        &layout,
        PaintOptions::default(),
        &mut cache,
        &mut GlyphRasterCache::new(),
    );
    let boundary = (CONTENT_TOP + layout.block_top(1)) as usize;
    assert!(
        (boundary..boundary + 4)
            .any(|y| pixels[y * width + 24..y * width + 48].contains(&0x00ff0000))
    );
    assert!(
        result
            .link_regions
            .iter()
            .any(|region| region.target() == "https://example.test/x")
    );
    assert!(cache.cached(1).is_none());
    assert!(
        cache.cached(0).is_some(),
        "unaffected blocks should stay checked"
    );
    assert!(
        cache.cached(2).is_none(),
        "overlapping blocks cannot use an opaque cache"
    );

    let mut scrolled = vec![0; width * height];
    let result = paint(
        &mut XrgbFrame::new(&mut scrolled, width, height).unwrap(),
        &page,
        &layout,
        PaintOptions {
            scroll_y: 20,
            ..PaintOptions::default()
        },
        &mut BlockRasterCache::new(),
        &mut GlyphRasterCache::new(),
    );
    let empty = crate::page::parse_xhtml_for_test(
        "https://example.test/",
        "<html xmlns='http://www.w3.org/1999/xhtml'><body/></html>",
    )
    .unwrap();
    let empty_layout = PageLayout::build(&empty, width, None);
    let mut empty_pixels = vec![0; width * height];
    paint(
        &mut XrgbFrame::new(&mut empty_pixels, width, height).unwrap(),
        &empty,
        &empty_layout,
        PaintOptions::default(),
        &mut BlockRasterCache::new(),
        &mut GlyphRasterCache::new(),
    );
    assert_eq!(
        &scrolled[..CONTENT_TOP as usize * width],
        &empty_pixels[..CONTENT_TOP as usize * width]
    );
    assert!(
        result
            .link_regions
            .iter()
            .all(|region| region.y >= CONTENT_TOP)
    );
    assert!(
        result
            .link_regions
            .iter()
            .any(|region| { region.target() == "https://example.test/x" && region.height > 0 })
    );
}

#[test]
fn vector_glyph_survives_zero_height_line() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/fonts/noto/nko-hmtx.woff2");
    let font = normalize_font_container(std::fs::read(path).unwrap()).unwrap();
    let page = crate::page::parse_xhtml_for_test(
        "https://example.test/",
        "<html xmlns='http://www.w3.org/1999/xhtml'><body><div style='line-height:40px'>ߊ</div><div style='line-height:0;font-size:32px;color:red'>ߊ</div></body></html>"
    ).unwrap();
    let (width, height) = (128, 170);
    let layout = PageLayout::build(&page, width, Some(&font));
    assert!(layout.has_text_overflow());
    assert_eq!(layout.block_height(1), 0);
    let mut pixels = vec![0; width * height];
    paint(
        &mut XrgbFrame::new(&mut pixels, width, height).unwrap(),
        &page,
        &layout,
        PaintOptions {
            fallback_font_data: Some(&font),
            ..PaintOptions::default()
        },
        &mut BlockRasterCache::new(),
        &mut GlyphRasterCache::new(),
    );
    assert!(pixels[CONTENT_TOP as usize * width..].iter().any(|pixel| {
        let red = (pixel >> 16) & 0xff;
        let green = (pixel >> 8) & 0xff;
        red > green && green < 240
    }));
}

#[test]
fn wrapped_text_advances_once_per_word_and_line() {
    let text = "a a a a a a";
    let mut pixels = vec![0; 64 * 30];
    let mut regions = Vec::new();
    let mut cache = GlyphRasterCache::new();

    let end_y = TextPainter::new(
        XrgbFrame::new(&mut pixels, 64, 30).unwrap(),
        0,
        None,
        &mut regions,
        &mut cache,
    )
    .draw_wrapped_text(
        TextOrigin::at(0, 0),
        64,
        text,
        &bitmap_style(16.0),
        Some("/link"),
    );

    let positions: Vec<_> = regions.iter().map(|region| (region.x, region.y)).collect();

    assert_eq!(
        positions,
        vec![(0, 0), (16, 0), (32, 0), (48, 0), (0, 10), (16, 10)]
    );

    assert_eq!(
        end_y,
        measure_wrapped_text(
            text,
            64,
            16.0,
            FontWeight::Normal,
            FontStyle::Normal,
            None,
            &[],
        )
    );

    assert_eq!(end_y, 20);
}

#[test]
fn long_wrapped_text_does_not_overflow() {
    let text = "a ".repeat(4096);
    let mut pixels = vec![0; 8 * 10];
    let mut regions = Vec::new();
    let mut cache = GlyphRasterCache::new();

    let end_y = TextPainter::new(
        XrgbFrame::new(&mut pixels, 8, 10).unwrap(),
        0,
        None,
        &mut regions,
        &mut cache,
    )
    .draw_wrapped_text(TextOrigin::at(0, 0), 8, &text, &bitmap_style(16.0), None);

    assert_eq!(end_y, 40_960);

    assert_eq!(
        end_y,
        measure_wrapped_text(
            &text,
            8,
            16.0,
            FontWeight::Normal,
            FontStyle::Normal,
            None,
            &[],
        ),
    );
}

#[test]
fn glyph_cache_distinguishes_vertical_subpixel_phases() {
    use crate::graphics::font::normalize_font_container;

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/fonts/noto/nko-hmtx.woff2"
    );

    let data = normalize_font_container(std::fs::read(path).expect("Nko font fixture must exist"))
        .unwrap();

    let font = TrueTypeFont::parse(&data).unwrap();

    let render = |offset_x: f32, offset_y: f32, cache: &mut GlyphRasterCache| {
        let mut pixels = vec![0x00_FF_FF_FF; 80 * 80];

        let style = TextRunStyle {
            font: Some(&font),
            ..bitmap_style(32.0)
        };
        let origin = TextOrigin {
            x: 10,
            y: 10,
            offset_x,
            offset_y,
        };

        assert!(draw_vector_text_line(
            &mut XrgbFrame::new(&mut pixels, 80, 80).unwrap(),
            cache,
            origin,
            "ߊ",
            &style,
            &font,
        ));

        pixels
    };

    let mut cache = GlyphRasterCache::new();

    let at_zero = render(0.0, 0.0, &mut cache);
    let at_half = render(0.0, 0.5, &mut cache);

    assert_eq!(cache.entries.len(), 2);
    assert_ne!(at_zero, at_half);

    // Repeated draws must reuse the correct cached glyph.
    assert_eq!(at_half, render(0.0, 0.5, &mut cache));
    assert_eq!(at_zero, render(0.0, 0.0, &mut cache));
    assert_eq!(cache.entries.len(), 2);

    // A fresh cache must produce the same result.
    assert_eq!(at_half, render(0.0, 0.5, &mut GlyphRasterCache::new()));

    // A whole-pixel shift reuses the same phase and moves the image down.
    let at_one = render(0.0, 1.0, &mut cache);

    assert_eq!(&at_one[80..], &at_zero[..79 * 80]);
    assert_eq!(cache.entries.len(), 2);

    let right_half = render(0.5, 0.0, &mut cache);

    assert_ne!(right_half, at_zero);
    assert_eq!(cache.entries.len(), 3);
    assert_eq!(right_half, render(0.5, 0.0, &mut cache));

    let right_one = render(1.0, 0.0, &mut cache);

    for y in 0..80 {
        assert_eq!(
            &right_one[y * 80 + 1..(y + 1) * 80],
            &at_zero[y * 80..y * 80 + 79]
        );
    }

    assert_eq!(cache.entries.len(), 3);
}

#[test]
fn styled_text_without_block_cache_matches_cached_rendering() {
    let page = crate::page::parse_xhtml_for_test(
        "https://example.test/",
        concat!(
            "<html xmlns='http://www.w3.org/1999/xhtml'>",
            "<body><div>A <span style='color:red'>B</span> ",
            "<strong>C</strong></div></body></html>"
        ),
    )
    .unwrap();

    let layout = PageLayout::build(&page, 64, None);
    assert!(layout.block_text_runs(0).is_some());

    let draw = |cache: &mut BlockRasterCache| {
        let mut pixels = vec![0; 64 * 130];

        paint(
            &mut XrgbFrame::new(&mut pixels, 64, 130).unwrap(),
            &page,
            &layout,
            PaintOptions::default(),
            cache,
            &mut GlyphRasterCache::new(),
        );

        pixels
    };

    let cached = draw(&mut BlockRasterCache::new());

    let mut disabled = BlockRasterCache::new();
    disabled.entries.push(BlockRasterCacheEntry::Uncacheable);

    let direct = draw(&mut disabled);

    assert_eq!(cached, direct);
}

#[test]
fn styled_link_remains_clickable_with_and_without_cache() {
    let page = crate::page::parse_xhtml_for_test(
        "https://example.test/",
        concat!(
            "<html xmlns='http://www.w3.org/1999/xhtml'>",
            "<head><style>a strong { color: red; }</style></head>",
            "<body><a href='/target'>A <strong>B</strong> C</a></body>",
            "</html>"
        ),
    )
    .unwrap();

    let layout = PageLayout::build(&page, 64, None);
    assert!(layout.block_text_runs(0).is_some());

    let run = |cache: &mut BlockRasterCache| {
        let mut pixels = vec![0; 64 * 130];

        let result = paint(
            &mut XrgbFrame::new(&mut pixels, 64, 130).unwrap(),
            &page,
            &layout,
            PaintOptions::default(),
            cache,
            &mut GlyphRasterCache::new(),
        );

        (pixels, result.link_regions)
    };

    let (cached_pixels, cached_regions) = run(&mut BlockRasterCache::new());

    let mut cache = BlockRasterCache::new();
    cache.entries.push(BlockRasterCacheEntry::Uncacheable);

    let (direct_pixels, direct_regions) = run(&mut cache);

    assert_eq!(cached_pixels, direct_pixels);
    assert!(cached_pixels[90 * 64..].contains(&0x00ff0000));

    for regions in [&cached_regions, &direct_regions] {
        for (x, y) in [(16, 90), (32, 90), (16, 100)] {
            assert!(
                regions.iter().any(|region| {
                    region.contains(x, y) && region.target() == "https://example.test/target"
                }),
                "missing link at {x},{y}"
            );
        }
    }
}

#[test]
fn link_targets_survive_inline_layout_and_cache() {
    use crate::page::{TextBlock, TextRun};

    let mut page = crate::page::parse_xhtml_for_test(
        "https://example.test/",
        "<html xmlns='http://www.w3.org/1999/xhtml'><body><p>abcd</p></body></html>",
    )
    .unwrap();

    let block = &mut page.blocks[0];
    let style = block.style.clone();

    block.content = BlockContent::Text(TextBlock {
        text: "abcd".into(),
        runs: vec![
            TextRun {
                range: 0..1,
                style: style.clone(),
                link: None,
            },
            TextRun {
                range: 1..3,
                style: style.clone(),
                link: Some("https://example.test/bc".into()),
            },
            TextRun {
                range: 3..4,
                style,
                link: None,
            },
        ],
    });

    let layout = PageLayout::build(&page, 120, None);
    let runs = layout.block_text_runs(0).expect("positioned runs");

    assert_eq!(
        runs.iter()
            .map(|run| (run.text.as_str(), run.link.as_deref()))
            .collect::<Vec<_>>(),
        [
            ("a", None),
            ("bc", Some("https://example.test/bc")),
            ("d", None)
        ]
    );

    let paint_page = |cache: &mut BlockRasterCache| {
        let mut pixels = vec![0; 120 * 120];

        let result = paint(
            &mut XrgbFrame::new(&mut pixels, 120, 120).unwrap(),
            &page,
            &layout,
            PaintOptions::default(),
            cache,
            &mut GlyphRasterCache::new(),
        );

        (pixels, result.link_regions)
    };

    let (cached_pixels, cached_regions) = paint_page(&mut BlockRasterCache::new());

    let mut no_cache = BlockRasterCache::new();
    no_cache.entries.push(BlockRasterCacheEntry::Uncacheable);

    let (direct_pixels, direct_regions) = paint_page(&mut no_cache);

    assert_eq!(cached_pixels, direct_pixels);

    for regions in [&cached_regions, &direct_regions] {
        assert!(regions.iter().any(|region| {
            region.contains(24, 90) && region.target() == "https://example.test/bc"
        }));

        assert!(
            !regions
                .iter()
                .any(|region| { region.contains(16, 90) || region.contains(40, 90) })
        );
    }
}

#[test]
fn inline_html_links_render_and_remain_clickable_with_fallback() {
    for html in [
        "<p>aa <a href='/one'>bb</a> cc <a href='/two'>dd</a> ee</p>",
        "<p>aa <a href='/one'><span style='font-size:24px'>bb</span></a> cc <a href='/two'>dd</a> ee</p>",
        "<p>aa\u{a0}<a href='/one'>bb</a> cc <a href='/two'>dd</a> ee</p>",
    ] {
        let html = format!("<html xmlns='http://www.w3.org/1999/xhtml'><body>{html}</body></html>");

        let page = crate::page::parse_xhtml_for_test("https://example.test/", &html).unwrap();

        assert_eq!(page.blocks.len(), 1);

        let layout = PageLayout::build(&page, 80, None);
        let runs = layout.block_text_runs(0).expect("positioned link runs");

        let targets: Vec<_> = runs.iter().filter_map(|run| run.link.as_deref()).collect();

        assert_eq!(
            targets,
            ["https://example.test/one", "https://example.test/two"]
        );

        let first = runs
            .iter()
            .find(|run| run.link.as_deref() == Some("https://example.test/one"))
            .unwrap();

        let second = runs
            .iter()
            .find(|run| run.link.as_deref() == Some("https://example.test/two"))
            .unwrap();

        assert!(second.y > first.y);

        let draw = |cache: &mut BlockRasterCache| {
            let mut pixels = vec![0; 80 * 160];

            let result = paint(
                &mut XrgbFrame::new(&mut pixels, 80, 160).unwrap(),
                &page,
                &layout,
                PaintOptions::default(),
                cache,
                &mut GlyphRasterCache::new(),
            );

            (pixels, result.link_regions)
        };

        let (cached, regions) = draw(&mut BlockRasterCache::new());

        let mut disabled = BlockRasterCache::new();
        disabled.entries.push(BlockRasterCacheEntry::Uncacheable);

        let (direct, direct_regions) = draw(&mut disabled);

        assert_eq!(cached, direct);

        for regions in [&regions, &direct_regions] {
            for run in runs {
                let x = 16 + run.x;
                let y = CONTENT_TOP + run.y;

                if let Some(target) = &run.link {
                    assert!(
                        regions
                            .iter()
                            .any(|region| { region.contains(x, y) && region.target() == target }),
                        "{html}"
                    );
                } else {
                    assert!(
                        !regions.iter().any(|region| region.contains(x, y)),
                        "{html}"
                    );
                }
            }
        }
    }
}

#[test]
fn mixed_inline_sizes_align_and_preserve_link_hit_testing() {
    let page = crate::page::parse_xhtml_for_test(
        "https://example.test/",
        concat!(
            "<html xmlns='http://www.w3.org/1999/xhtml'><body>",
            "<p>A<a href='/link' style='font-size:32px;color:red'>B</a>C</p>",
            "</body></html>",
        ),
    )
    .unwrap();

    let layout = PageLayout::build(&page, 120, None);
    let runs = layout.block_text_runs(0).expect("mixed-size runs");

    assert_eq!(runs.len(), 3);

    assert_eq!(
        runs.iter()
            .map(|run| run.style.font_size)
            .collect::<Vec<_>>(),
        [16.0, 32.0, 16.0]
    );

    assert!(runs[0].y > runs[1].y);
    assert_eq!(runs[0].y, runs[2].y);
    assert_eq!(runs[1].link.as_deref(), Some("https://example.test/link"));
    assert!(runs[0].link.is_none() && runs[2].link.is_none());
    assert!(runs[2].x > runs[1].x);

    let draw = |cache: &mut BlockRasterCache| {
        let mut pixels = vec![0; 120 * 150];

        let result = paint(
            &mut XrgbFrame::new(&mut pixels, 120, 150).unwrap(),
            &page,
            &layout,
            PaintOptions::default(),
            cache,
            &mut GlyphRasterCache::new(),
        );

        (pixels, result.link_regions)
    };

    let (pixels, regions) = draw(&mut BlockRasterCache::new());

    let mut no_cache = BlockRasterCache::new();
    no_cache.entries.push(BlockRasterCacheEntry::Uncacheable);

    let (direct, direct_regions) = draw(&mut no_cache);

    assert_eq!(pixels, direct);
    assert!(pixels[90 * 120..].contains(&0x00ff0000));

    for regions in [&regions, &direct_regions] {
        assert!(regions.iter().any(|region| {
            region.contains(16 + runs[1].x, 90 + runs[1].y)
                && region.target() == "https://example.test/link"
        }));

        assert!(!regions.iter().any(|region| {
            region.contains(16 + runs[0].x, 90 + runs[0].y)
                || region.contains(16 + runs[2].x, 90 + runs[2].y)
        }));
    }
}

#[test]
fn nbsp_stays_unbroken_and_blank_in_normal_and_link_text() {
    let parse = |text: &str| {
        crate::page::parse_xhtml_for_test(
            "https://example.test/",
            &format!("<html xmlns='http://www.w3.org/1999/xhtml'><body>{text}</body></html>"),
        )
        .unwrap()
    };

    let plain = parse("<p>A\u{00a0}B</p>");
    let regular = parse("<p>A B</p>");

    assert_eq!(PageLayout::build(&plain, 48, None).block_height(0), 10);
    assert_eq!(PageLayout::build(&regular, 48, None).block_height(0), 20);

    for html in [
        "<p>A<a href='/x'>\u{00a0}B</a> C</p>",
        "<p>A<a href='/x' style='font-family:Other'>\u{00a0}B</a> C</p>",
    ] {
        let page = parse(html);
        let layout = PageLayout::build(&page, 64, None);
        let runs = layout.block_text_runs(0).unwrap();

        assert_eq!(
            runs.iter().map(|run| run.text.as_str()).collect::<Vec<_>>(),
            ["A", "\u{00a0}B", "C"]
        );

        assert_eq!((runs[0].y, runs[1].y), (0, 0));
        assert!(runs[2].y > 0);

        let draw = |cache: &mut BlockRasterCache| {
            let mut pixels = vec![0; 64 * 130];

            let result = paint(
                &mut XrgbFrame::new(&mut pixels, 64, 130).unwrap(),
                &page,
                &layout,
                PaintOptions::default(),
                cache,
                &mut GlyphRasterCache::new(),
            );

            (pixels, result.link_regions)
        };

        let (pixels, regions) = draw(&mut BlockRasterCache::new());

        let mut disabled = BlockRasterCache::new();
        disabled.entries.push(BlockRasterCacheEntry::Uncacheable);

        let (direct, direct_regions) = draw(&mut disabled);

        assert_eq!(pixels, direct);

        for y in 90..98 {
            for x in 24..32 {
                assert_eq!(pixels[y * 64 + x], BACKGROUND, "{html}");
            }
        }

        for regions in [&regions, &direct_regions] {
            assert!(regions.iter().any(|region| {
                region.contains(32, 90) && region.target() == "https://example.test/x"
            }));
        }
    }

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/wpt/fonts/noto/NotoSansMongolian-regular.woff2"
    );

    let bytes = normalize_font_container(std::fs::read(path).unwrap()).unwrap();
    let font = TrueTypeFont::parse(&bytes).unwrap();

    let page = parse("<p>ᠠ<a href='/x' style='color:black'>\u{00a0}</a>ᠠ</p>");
    let layout = PageLayout::build(&page, 120, Some(&bytes));
    let runs = layout.block_text_runs(0).unwrap();

    assert_eq!(runs.len(), 3);

    let mut positions = Vec::new();

    crate::graphics::font::layout::visit_positioned_glyphs(
        &[&font],
        "ᠠ\u{00a0}ᠠ",
        16.0,
        |_, _, x| {
            positions.push(x);
            Ok(())
        },
    )
    .unwrap();

    for (run, x) in runs.iter().zip(positions) {
        let (base, phase) = split_subpixel_position(x).unwrap();

        assert_eq!(
            (run.x, run.offset_x),
            (base, f32::from(phase) / GLYPH_SUBPIXEL_STEPS as f32)
        );
    }
}

#[test]
fn inherited_nowrap_keeps_one_line_and_link_clickable() {
    let parse = |body: &str| {
        crate::page::parse_xhtml_for_test(
            "https://example.test/",
            &format!("<html xmlns='http://www.w3.org/1999/xhtml'><body>{body}</body></html>"),
        )
        .unwrap()
    };

    let page = parse("<div style='white-space:nowrap'><p>A <a href='/x'>B</a> C</p></div>");

    assert_eq!(page.blocks.len(), 1);

    let layout = PageLayout::build(&page, 64, None);
    assert_eq!(layout.block_height(0), 10);

    let runs = layout.block_text_runs(0).unwrap();
    assert!(runs.iter().all(|run| run.y == 0));

    let draw = |cache: &mut BlockRasterCache| {
        let mut pixels = vec![0; 64 * 130];

        let result = paint(
            &mut XrgbFrame::new(&mut pixels, 64, 130).unwrap(),
            &page,
            &layout,
            PaintOptions::default(),
            cache,
            &mut GlyphRasterCache::new(),
        );

        (pixels, result.link_regions)
    };

    let (pixels, regions) = draw(&mut BlockRasterCache::new());

    let mut no_cache = BlockRasterCache::new();
    no_cache.entries.push(BlockRasterCacheEntry::Uncacheable);

    let (direct, direct_regions) = draw(&mut no_cache);

    assert_eq!(pixels, direct);

    for regions in [&regions, &direct_regions] {
        assert!(regions.iter().any(|region| {
            region.contains(32, 90) && region.target() == "https://example.test/x"
        }));
    }

    for keyword in ["normal", "initial"] {
        let page = parse(&format!(
            "<div style='white-space:nowrap'><p style='white-space: {keyword}'>A B C</p></div>"
        ));

        assert_eq!(
            PageLayout::build(&page, 64, None).block_height(0),
            20,
            "{keyword}"
        );
    }

    for keyword in ["inherit", "unset"] {
        let page = parse(&format!(
            "<div style='white-space:nowrap'><p style='white-space:normal;white-space:{keyword}'>A B C</p></div>"
        ));

        assert_eq!(
            PageLayout::build(&page, 64, None).block_height(0),
            10,
            "{keyword}"
        );
    }

    let page = parse("<p style='white-space:nowrap;white-space:pre'>A B C</p>");
    assert_eq!(PageLayout::build(&page, 64, None).block_height(0), 10);

    let page =
        parse("<p style='white-space:nowrap'>A <span style='font-family:Other'>B</span> C</p>");

    let fallback_layout = PageLayout::build(&page, 64, None);

    assert_eq!(fallback_layout.block_height(0), 10);
    assert!(fallback_layout.block_text_runs(0).is_some());
}

#[test]
fn canvas_is_mutable_clipped_and_not_cached() {
    let mut page = crate::page::parse_xhtml_for_test(
        "https://example.test/",
        concat!(
            "<html xmlns='http://www.w3.org/1999/xhtml'>",
            "<body><canvas width='4' height='4'></canvas></body>",
            "</html>"
        ),
    )
    .unwrap();

    let layout = PageLayout::build(&page, 64, None);
    let mut cache = BlockRasterCache::new();

    for (color, expected) in [
        (RasterColor::rgba(255, 0, 0, 255), 0xff0000),
        (RasterColor::rgba(0, 255, 0, 255), 0x00ff00),
    ] {
        let BlockContent::Canvas(canvas) = &mut page.blocks[0].content else {
            panic!("canvas expected");
        };

        canvas.canvas.set_fill_style(color);
        canvas.canvas.fill_rect(0.0, 0.0, 4.0, 4.0);

        let mut pixels = vec![0; 64 * 100];

        paint(
            &mut XrgbFrame::new(&mut pixels, 64, 100).unwrap(),
            &page,
            &layout,
            PaintOptions {
                scroll_y: 2,
                ..PaintOptions::default()
            },
            &mut cache,
            &mut GlyphRasterCache::new(),
        );

        assert_eq!(pixels[90 * 64 + 16], expected);
        assert_eq!(pixels[89 * 64 + 16], BACKGROUND);
    }
}

#[test]
fn canvas_renders_and_clips_when_text_overflows() {
    let mut page = crate::page::parse_xhtml_for_test(
        "https://example.test/",
        "<html xmlns='http://www.w3.org/1999/xhtml'><body><div style='line-height:0'>X</div><canvas width='4' height='4'></canvas></body></html>",
    ).unwrap();
    let layout = PageLayout::build(&page, 64, None);
    assert!(layout.has_text_overflow());
    let index = page
        .blocks
        .iter()
        .position(|block| matches!(block.content, BlockContent::Canvas(_)))
        .unwrap();
    let BlockContent::Canvas(canvas) = &mut page.blocks[index].content else {
        unreachable!();
    };
    canvas
        .canvas
        .set_fill_style(RasterColor::rgba(255, 0, 0, 255));
    canvas.canvas.fill_rect(0.0, 0.0, 4.0, 4.0);

    let mut pixels = vec![0; 64 * 100];
    paint(
        &mut XrgbFrame::new(&mut pixels, 64, 100).unwrap(),
        &page,
        &layout,
        PaintOptions {
            scroll_y: 2,
            ..PaintOptions::default()
        },
        &mut BlockRasterCache::new(),
        &mut GlyphRasterCache::new(),
    );
    assert_eq!(pixels[90 * 64 + 16], 0xff0000);
    assert_eq!(pixels[89 * 64 + 16], BACKGROUND);
}
