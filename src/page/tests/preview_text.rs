//! Experimental preview scene: inline runs, text painting, colors, weights and faces.

use super::*;

#[test]
fn preview_keeps_inline_sources_when_words_are_measured_together() {
    use crate::layout_preview::{PreviewCommand, PreviewLayout};

    let page = parse_html(
        "https://example.test/",
        "<div>a<span>б</span> <span>cd</span></div>",
    );
    let preview = PreviewLayout::build(&page, 100, None).unwrap();
    let runs = preview
        .display_list
        .iter()
        .find_map(|item| match item {
            PreviewCommand::Text { runs, .. } => Some(runs),
            _ => None,
        })
        .unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].text, "aб");
    assert_eq!(runs[0].sources[0].range, 0..1);
    assert_eq!(runs[0].sources[0].source, None);
    assert_eq!(runs[0].sources[0].advance, Some(0.0..8.0));
    assert_eq!(runs[0].sources[1].range, 1..3);
    assert_eq!(runs[0].sources[1].advance, Some(8.0..16.0));
    assert_eq!(
        runs[0].sources[1].source.as_ref().unwrap().indices(),
        &[0, 1]
    );
    assert_eq!(runs[1].sources[0].range, 0..2);
    assert_eq!(
        runs[1].sources[0].source.as_ref().unwrap().indices(),
        &[0, 3]
    );
    assert!(runs.iter().all(|run| run.width > 0));
    assert!(preview.format_debug().contains("inline sources="));

    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/fonts/noto/nko-hmtx.woff2");
    let bytes = normalize_font_container(std::fs::read(path).unwrap()).unwrap();
    let font = crate::graphics::font::TrueTypeFont::parse(&bytes).unwrap();
    let page = parse_html("https://example.test/", "<div>ߊ<span>ߊ</span></div>");
    let preview = PreviewLayout::build(&page, 100, Some(&bytes)).unwrap();
    let run = preview
        .display_list
        .iter()
        .find_map(|item| match item {
            PreviewCommand::Text { runs, .. } => runs.first(),
            _ => None,
        })
        .unwrap();
    let mut glyph_positions = Vec::new();
    let advance = crate::graphics::font::layout::visit_positioned_glyphs(
        &[&font],
        &run.text,
        run.style.font_size,
        |_, _, x| {
            glyph_positions.push(x);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(run.sources[0].advance, Some(0.0..glyph_positions[1]));
    assert_eq!(run.sources[1].advance, Some(glyph_positions[1]..advance));
}

#[test]
fn preview_paints_simple_inline_background_without_coloring_adjacent_text() {
    use crate::{
        layout_preview::{PreviewCommand, PreviewLayout},
        render::{GlyphRasterCache, paint_preview_scene},
    };
    let page = parse_html(
        "https://example.test/",
        "<div><span style='background:blue'>A</span>B</div>",
    );
    let layout = PreviewLayout::build(&page, 100, None).unwrap();
    let background = layout
        .display_list
        .iter()
        .find_map(|command| match command {
            PreviewCommand::InlineBackground { rect, .. } => Some(rect),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        (
            background.x,
            background.y,
            background.width,
            background.height
        ),
        (16, 0, 8, 8)
    );
    let mut pixels = vec![0; 100 * 120];
    paint_preview_scene(
        &mut XrgbFrame::new(&mut pixels, 100, 120).unwrap(),
        &page,
        &layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );
    assert_eq!(pixels[90 * 100 + 16], 0x00_00_00_FF);
    let reference = parse_html("https://example.test/", "<div><span>A</span>B</div>");
    let reference_layout = PreviewLayout::build(&reference, 100, None).unwrap();
    let mut reference_pixels = vec![0; 100 * 120];
    paint_preview_scene(
        &mut XrgbFrame::new(&mut reference_pixels, 100, 120).unwrap(),
        &reference,
        &reference_layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );
    for y in 90..98 {
        assert_eq!(
            &pixels[y * 100 + 24..y * 100 + 32],
            &reference_pixels[y * 100 + 24..y * 100 + 32]
        );
    }
    assert!((90..98).any(|y| pixels[y * 100 + 16..y * 100 + 24].contains(&0)));

    let words = parse_html(
        "https://example.test/",
        "<div><span style='background:blue'>A B</span></div>",
    );
    for (width, expected) in [
        (100, vec![(16, 0, 24)]),
        (48, vec![(16, 0, 8), (16, 10, 8)]),
    ] {
        let layout = PreviewLayout::build(&words, width, None).unwrap();
        let rectangles: Vec<_> = layout
            .display_list
            .iter()
            .filter_map(|command| match command {
                PreviewCommand::InlineBackground { rect, .. } => Some((rect.x, rect.y, rect.width)),
                _ => None,
            })
            .collect();
        assert_eq!(rectangles, expected);
        let mut image = vec![0; width * 120];
        paint_preview_scene(
            &mut XrgbFrame::new(&mut image, width, 120).unwrap(),
            &words,
            &layout,
            None,
            0,
            &mut GlyphRasterCache::new(),
        );
        assert_eq!(
            image[90 * width + 24],
            if width == 100 {
                0x00_00_00_FF
            } else {
                crate::render::BACKGROUND
            }
        );
    }

    for (html, width, expected) in [
        (
            "A<span style='background:blue'>  B</span>",
            100,
            vec![(24, 0, 16)],
        ),
        (
            "A <span style='background:blue'> B</span>",
            100,
            vec![(32, 0, 8)],
        ),
        (
            "A<span style='background:blue'> </span>B",
            100,
            vec![(24, 0, 8)],
        ),
        (
            "<span style='background:blue'>A </span>B",
            100,
            vec![(16, 0, 16)],
        ),
        (
            "<span style='background:blue'>A</span> B",
            100,
            vec![(16, 0, 8)],
        ),
        (
            "A<span style='background:blue'> B</span>",
            48,
            vec![(16, 10, 8)],
        ),
    ] {
        let page = parse_html("https://example.test/", &format!("<div>{html}</div"));
        let preview = PreviewLayout::build(&page, width, None).unwrap();
        let rects: Vec<_> = preview
            .display_list
            .iter()
            .filter_map(|command| match command {
                PreviewCommand::InlineBackground { rect, .. } => Some((rect.x, rect.y, rect.width)),
                _ => None,
            })
            .collect();
        assert_eq!(rects, expected, "{html}");
    }

    for text in ["A\nB", "A\rB", "A\u{a0}B"] {
        let html = format!("<div><span style='background:blue'>{text}</span></div>");
        let unsupported = parse_html("https://example.test/", &html);
        assert!(
            PreviewLayout::build(&unsupported, 100, None).is_err(),
            "{text:?}"
        );
    }
}

#[test]
fn preview_expands_inline_background_without_expanding_the_line() {
    use crate::{
        layout_preview::{PreviewCommand, PreviewLayout},
        render::{BACKGROUND, GlyphRasterCache, paint_preview_scene},
    };
    let page = parse_html(
        "https://example.test/",
        "<div style='padding-top:8px'><span style='background:blue;padding-top:3px;padding-bottom:4px'>A</span> B</div><div>C</div>",
    );
    let preview = PreviewLayout::build(&page, 100, None).unwrap();
    let rect = preview
        .display_list
        .iter()
        .find_map(|command| match command {
            PreviewCommand::InlineBackground { rect, .. } => Some(rect),
            _ => None,
        })
        .unwrap();
    assert_eq!((rect.x, rect.y, rect.width, rect.height), (16, 5, 8, 15));
    assert_eq!(
        (preview.boxes[0].rect.y, preview.boxes[0].rect.height),
        (0, 18)
    );
    assert_eq!(preview.boxes[1].rect.y, 18);

    let mut pixels = vec![0; 100 * 130];
    paint_preview_scene(
        &mut XrgbFrame::new(&mut pixels, 100, 130).unwrap(),
        &page,
        &preview,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );
    assert_eq!(pixels[95 * 100 + 16], 0x00_00_00_FF);
    assert_eq!(pixels[94 * 100 + 16], BACKGROUND);

    let wrapped = parse_html(
        "https://example.test/",
        "<div><span style='background:blue;padding-top:3px;padding-bottom:4px'>A B</span></div>",
    );
    assert!(PreviewLayout::build(&wrapped, 48, None).is_ok());

    let overlapping = parse_html(
        "https://example.test/",
        "<div><span>A</span> <span style='background:blue;padding-top:5px'>B</span></div>",
    );
    let reference = parse_html("https://example.test/", "<div><span>A</span> B</div>");
    let overlapping_layout = PreviewLayout::build(&overlapping, 48, None).unwrap();
    let reference_layout = PreviewLayout::build(&reference, 48, None).unwrap();
    let mut actual = vec![0; 48 * 120];
    let mut original = vec![0; 48 * 120];
    paint_preview_scene(
        &mut XrgbFrame::new(&mut actual, 48, 120).unwrap(),
        &overlapping,
        &overlapping_layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );
    paint_preview_scene(
        &mut XrgbFrame::new(&mut original, 48, 120).unwrap(),
        &reference,
        &reference_layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );
    assert!((95..98).any(|y| {
        (16..24).any(|x| original[y * 48 + x] == 0 && actual[y * 48 + x] == 0x00_00_00_FF)
    }));

    let short = parse_html(
        "https://example.test/",
        "<div style='line-height:4px'><span style='background:blue;padding-top:3px'>A B</span></div>",
    );
    let preview = PreviewLayout::build(&short, 48, None).unwrap();
    let backgrounds: Vec<_> = preview
        .display_list
        .iter()
        .filter_map(|command| match command {
            PreviewCommand::InlineBackground { rect, .. } => Some((rect.x, rect.y, rect.height)),
            _ => None,
        })
        .collect();
    assert_eq!(backgrounds.len(), 2);
    assert_eq!(backgrounds[1].1 - backgrounds[0].1, 4);

    let zero = |text| {
        parse_html(
            "https://example.test/",
            &format!(
                "<div style='padding-top:10px'><div style='line-height:0'><span style='background:blue'>{text}</span></div></div>"
            ),
        )
    };
    let page = zero("A B");
    let reference = zero("B");
    let layout = PreviewLayout::build(&page, 48, None).unwrap();
    let reference_layout = PreviewLayout::build(&reference, 48, None).unwrap();
    assert_eq!(layout.boxes[0].rect.height, 10);
    assert_eq!(layout.boxes[1].rect.height, 0);
    assert_eq!(
        layout
            .display_list
            .iter()
            .filter(|command| matches!(command, PreviewCommand::InlineBackground { .. }))
            .count(),
        2
    );
    let mut pixels = vec![0; 48 * 120];
    let mut reference_pixels = vec![0; 48 * 120];
    paint_preview_scene(
        &mut XrgbFrame::new(&mut pixels, 48, 120).unwrap(),
        &page,
        &layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );
    paint_preview_scene(
        &mut XrgbFrame::new(&mut reference_pixels, 48, 120).unwrap(),
        &reference,
        &reference_layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );
    assert_eq!(pixels, reference_pixels);
    assert!(pixels[96 * 48..104 * 48].contains(&0x00_00_00_FF));

    let plain = parse_html(
        "https://example.test/",
        "<div style='padding-top:10px'><div style='line-height:0'>A</div></div>",
    );
    let plain_layout = PreviewLayout::build(&plain, 48, None).unwrap();
    let mut plain_pixels = vec![0; 48 * 120];
    paint_preview_scene(
        &mut XrgbFrame::new(&mut plain_pixels, 48, 120).unwrap(),
        &plain,
        &plain_layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );
    assert!(plain_pixels[96 * 48..104 * 48].contains(&0));

    let margins = parse_html(
        "https://example.test/",
        "<div style='height:10px;margin-bottom:8px'></div><div style='line-height:0;margin-top:12px;margin-bottom:7px'>X</div><div style='height:10px;margin-top:5px'></div>",
    );
    let layout = PreviewLayout::build(&margins, 100, None).unwrap();
    assert_eq!(
        layout
            .boxes
            .iter()
            .map(|item| item.rect.y)
            .collect::<Vec<_>>(),
        vec![0, 22, 29]
    );
}

#[test]
fn preview_scene_paints_text_and_clips_it_below_toolbar() {
    let page = parse_html(
        "https://example.test/",
        "<div style='width:32px;height:20px;background:blue;color:white'>A</div>",
    );

    let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

    let mut pixels = vec![0x00_12_34_56; 100 * 120];
    let mut cache = crate::render::GlyphRasterCache::new();

    crate::render::paint_preview_scene(
        &mut XrgbFrame::new(&mut pixels, 100, 120).unwrap(),
        &page,
        &preview,
        None,
        1,
        &mut cache,
    );

    assert!(
        pixels[..90 * 100]
            .iter()
            .all(|&pixel| pixel == 0x00_12_34_56)
    );

    assert!((90..98).any(|y| { (16..24).any(|x| pixels[y * 100 + x] == 0x00_FF_FF_FF) }));
    assert_eq!(pixels[90 * 100 + 40], 0x00_00_00_FF);
}

#[test]
fn preview_scene_paints_later_background_over_earlier_text() {
    let page = parse_html(
        "https://example.test/",
        concat!(
            "<div style='width:32px;height:10px;",
            "background:red;color:white'>A</div>",
            "<div style='width:32px;height:10px;",
            "margin-top:-10px;background:blue'></div>"
        ),
    );

    let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

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

    assert!((90..98).all(|y| { (16..24).all(|x| pixels[y * 100 + x] == 0x00_00_00_FF) }))
}

#[test]
fn preview_rejects_inline_color_changes_instead_of_mispainting() {
    let page = parse_html(
        "https://example.test/",
        "<div>A<span style='color:red'>B</span></div>",
    );

    assert!(matches!(
        crate::layout_preview::PreviewLayout::build(&page, 100, None),
        Err(crate::layout_preview::PreviewError::InlineStyle(_))
    ));
}

#[test]
fn preview_positioned_words_match_unfragmented_text() {
    use crate::layout_preview::{PreviewCommand, PreviewLayout};
    use crate::render::{GlyphRasterCache, paint_preview_scene};

    let plain = parse_html("https://example.test/", "<div>hello world long</div>");

    let split = parse_html(
        "https://example.test/",
        "<div>hel<span>lo</span> world long</div>",
    );

    let width = 100;
    let height = 150;

    let plain_layout = PreviewLayout::build(&plain, width, None).unwrap();
    let split_layout = PreviewLayout::build(&split, width, None).unwrap();

    let runs = split_layout
        .display_list
        .iter()
        .find_map(|command| match command {
            PreviewCommand::Text { runs, .. } if !runs.is_empty() => Some(runs),
            _ => None,
        })
        .expect("split text must use positioned runs");

    assert_eq!(
        runs.iter().map(|run| run.text.as_str()).collect::<Vec<_>>(),
        vec!["hello", "world", "long"]
    );

    assert!(runs[1].y > runs[0].y);

    for scroll in [0, 6] {
        let mut actual = vec![0; width * height];
        let mut expected = vec![0; width * height];

        paint_preview_scene(
            &mut XrgbFrame::new(&mut actual, width, height).unwrap(),
            &split,
            &split_layout,
            None,
            scroll,
            &mut GlyphRasterCache::new(),
        );

        paint_preview_scene(
            &mut XrgbFrame::new(&mut expected, width, height).unwrap(),
            &plain,
            &plain_layout,
            None,
            scroll,
            &mut GlyphRasterCache::new(),
        );

        assert_eq!(actual, expected, "scroll={scroll}");
    }
}

#[test]
fn preview_uses_the_font_saved_in_each_run() {
    use crate::graphics::font::normalize_font_container;
    use crate::layout_preview::{PreviewCommand, PreviewLayout};
    use crate::render::{GlyphRasterCache, paint_preview_scene};

    let mut page = parse_html("https://example.test/", "<div>ߊ<span>ߊ</span> ߊ</div>");

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/fonts/noto/nko-hmtx.woff2"
    );

    let font = normalize_font_container(std::fs::read(path).expect("Nko font fixture must exist"))
        .unwrap();

    page.font_faces.push(LoadedFontFace {
        family: "Nko Test".to_owned(),
        font_weight: FontWeight::Normal,
        font_style: FontStyle::Normal,
        data: font,
    });

    let mut layout = PreviewLayout::build(&page, 120, None).unwrap();

    let runs = layout
        .display_list
        .iter_mut()
        .find_map(|command| match command {
            PreviewCommand::Text { runs, .. } if !runs.is_empty() => Some(runs),
            _ => None,
        })
        .expect("positioned text runs");

    assert_eq!(runs.len(), 2);

    // Change the display list directly: mixed fonts are still rejected by layout.
    runs[0].style.font_families = vec!["Nko Test".to_owned()];

    let mut actual = vec![0; 120 * 120];

    paint_preview_scene(
        &mut XrgbFrame::new(&mut actual, 120, 120).unwrap(),
        &page,
        &layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );

    let runs = layout
        .display_list
        .iter_mut()
        .find_map(|command| match command {
            PreviewCommand::Text { runs, .. } if !runs.is_empty() => Some(runs),
            _ => None,
        })
        .unwrap();

    runs[0].style.font_families.clear();

    let mut expected = vec![0; 120 * 120];

    paint_preview_scene(
        &mut XrgbFrame::new(&mut expected, 120, 120).unwrap(),
        &page,
        &layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );

    assert_ne!(actual, expected, "the saved run font must affect rendering");
}

#[test]
fn preview_paints_vector_and_bitmap_runs_with_fractional_baseline() {
    use crate::graphics::font::{
        TrueTypeFont, normalize_font_container,
        render::{GLYPH_SUBPIXEL_STEPS, split_subpixel_position},
    };
    use crate::inline_layout::PositionedTextRun;
    use crate::layout_preview::{PreviewCommand, PreviewLayout};
    use crate::page_layout::{CONTENT_TOP, LayoutRect, text_metrics};
    use crate::render::{GlyphRasterCache, paint_preview_scene};

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/fonts/noto/nko-hmtx.woff2"
    );

    let data = normalize_font_container(std::fs::read(path).expect("Nko font fixture must exist"))
        .unwrap();

    let font = TrueTypeFont::parse(&data).unwrap();

    // Find a size where the vector baseline has a fractional phase.
    let (vector_size, vector_base, phase) = (32..=64)
        .find_map(|size| {
            let ascent = font.line_metrics(size as f32)?.ascent;
            let (base, phase) = split_subpixel_position(ascent)?;

            (phase != 0).then_some((size as f32, base, phase))
        })
        .expect("fixture must have a fractional baseline");

    let bitmap = text_metrics(None, 16.0);
    let baseline = vector_base.max(bitmap.glyph_height);

    let vector_y = baseline - vector_base;
    let bitmap_y = baseline - bitmap.glyph_height;

    // The integer parts agree, but the vector phase is still nonzero.
    assert_eq!(vector_y + vector_base, bitmap_y + bitmap.glyph_height);

    assert!(phase > 0 && i32::from(phase) < GLYPH_SUBPIXEL_STEPS);

    let mut page = parse_html("https://example.test/", "<div></div>");

    page.font_faces.push(LoadedFontFace {
        family: "Nko Test".into(),
        font_weight: FontWeight::Normal,
        font_style: FontStyle::Normal,
        data,
    });

    let mut vector_style = freedom_css::TextStyle::initial();
    vector_style.font_families = vec!["Nko Test".into()];
    vector_style.font_size = vector_size;
    vector_style.color = 0x00_FF_00_00;

    let mut bitmap_style = freedom_css::TextStyle::initial();
    bitmap_style.color = 0x00_00_00_FF;

    // Construct the display list directly: layout still rejects mixed fonts.
    let mut layout = PreviewLayout {
        boxes: vec![],
        display_list: vec![PreviewCommand::Text {
            rect: LayoutRect {
                x: 16,
                y: 0,
                width: 200,
                height: 100,
            },
            text: String::new(),
            style: bitmap_style.clone(),
            runs: vec![
                PositionedTextRun {
                    line_index: 0,
                    x: 0,
                    offset_x: 0.0,
                    y: vector_y,
                    offset_y: 0.0,
                    width: 0,
                    sources: vec![],
                    leading_space: None,
                    text: "ߊ".into(),
                    style: vector_style,
                    link: None,
                },
                PositionedTextRun {
                    line_index: 0,
                    x: 90,
                    offset_x: 0.0,
                    y: bitmap_y,
                    offset_y: 0.0,
                    width: 0,
                    sources: vec![],
                    leading_space: None,
                    text: "A".into(),
                    style: bitmap_style,
                    link: None,
                },
            ],
        }],
    };

    let (width, height) = (240, 220);
    let mut pixels = vec![0; width * height];

    paint_preview_scene(
        &mut XrgbFrame::new(&mut pixels, width, height).unwrap(),
        &page,
        &layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );

    let content = &pixels[CONTENT_TOP as usize * width..];

    assert!(
        content.iter().any(|&pixel| {
            let red = (pixel >> 16) & 0xff;
            let green = (pixel >> 8) & 0xff;
            red > green
        }),
        "vector glyph must be painted red"
    );

    assert!(
        content.iter().any(|&pixel| {
            let blue = pixel & 0xff;
            let green = (pixel >> 8) & 0xff;
            blue > green
        }),
        "bitmap glyph must be painted blue"
    );

    let PreviewCommand::Text { runs, .. } = &mut layout.display_list[0] else {
        panic!("expected text command");
    };

    runs[0].offset_y = 0.5;

    let mut shifted = vec![0; width * height];

    paint_preview_scene(
        &mut XrgbFrame::new(&mut shifted, width, height).unwrap(),
        &page,
        &layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );

    assert_ne!(
        pixels, shifted,
        "vector run must use its vertical subpixel offset"
    );

    let PreviewCommand::Text { runs, .. } = &mut layout.display_list[0] else {
        panic!("expected text command");
    };

    runs[0].offset_y = 0.0;
    runs[0].offset_x = 0.5;

    let mut shifted_x = vec![0; width * height];

    paint_preview_scene(
        &mut XrgbFrame::new(&mut shifted_x, width, height).unwrap(),
        &page,
        &layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );

    assert_ne!(
        pixels, shifted_x,
        "vector run must use its horizontal subpixel offset"
    );

    // The bitmap glyph remains unchanged.
    for y in CONTENT_TOP as usize..height {
        assert_eq!(
            &pixels[y * width + 106..y * width + 114],
            &shifted[y * width + 106..y * width + 114]
        );
    }
}

#[test]
fn preview_rejects_mixed_inline_font_sizes() {
    let page = parse_html(
        "https://example.test/",
        "<div>A<span style='font-size:32px'>B</span></div>",
    );

    assert!(matches!(
        crate::layout_preview::PreviewLayout::build(&page, 120, None),
        Err(crate::layout_preview::PreviewError::InlineFont(_))
    ));
}

#[test]
fn preview_paints_inline_color_changes_between_words() {
    use crate::layout_preview::PreviewLayout;
    use crate::render::{GlyphRasterCache, paint_preview_scene};

    let page = parse_html(
        "https://example.test/",
        "<div>A<span style='color:red'> \tB</span> C</div>",
    );

    let layout = PreviewLayout::build(&page, 120, None).unwrap();
    let mut pixels = vec![0; 120 * 120];

    paint_preview_scene(
        &mut XrgbFrame::new(&mut pixels, 120, 120).unwrap(),
        &page,
        &layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );

    let contains = |left: usize, color: u32| {
        (90..100).any(|y| (left..left + 8).any(|x| pixels[y * 120 + x] == color))
    };

    assert!(contains(16, 0x00_00_00_00));
    assert!(contains(32, 0x00_FF_00_00));
    assert!(contains(48, 0x00_00_00_00));
}

#[test]
fn preview_paints_single_inline_word_with_different_color() {
    use crate::layout_preview::{PreviewCommand, PreviewLayout};
    use crate::render::{GlyphRasterCache, paint_preview_scene};

    let page = parse_html(
        "https://example.test/",
        "<div><span style='color:red'>A</span></div>",
    );

    let layout = PreviewLayout::build(&page, 120, None).unwrap();

    assert!(layout.display_list.iter().any(|command| {
        matches!(command, PreviewCommand::Text { runs, .. } if !runs.is_empty())
    }));

    let mut pixels = vec![0; 120 * 120];

    paint_preview_scene(
        &mut XrgbFrame::new(&mut pixels, 120, 120).unwrap(),
        &page,
        &layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );

    assert!((90..100).any(|y| { (16..24).any(|x| pixels[y * 120 + x] == 0x00_FF_00_00) }));
}

#[test]
fn preview_keeps_inline_color_when_word_wraps() {
    use crate::layout_preview::PreviewLayout;
    use crate::render::{GlyphRasterCache, paint_preview_scene};

    let page = parse_html(
        "https://example.test/",
        "<div>A <span style='color:red'>B</span></div>",
    );

    let layout = PreviewLayout::build(&page, 48, None).unwrap();
    let mut pixels = vec![0; 48 * 120];

    paint_preview_scene(
        &mut XrgbFrame::new(&mut pixels, 48, 120).unwrap(),
        &page,
        &layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );

    assert!((90..100).any(|y| { (16..24).any(|x| pixels[y * 48 + x] == 0x00_00_00_00) }));
    assert!((100..110).any(|y| { (16..24).any(|x| pixels[y * 48 + x] == 0x00_FF_00_00) }));
}

#[test]
fn preview_inline_color_matches_independent_bitmap_reference() {
    use crate::layout_preview::PreviewLayout;
    use crate::render::{GlyphRasterCache, paint_preview_scene};

    const WIDTH: usize = 64;
    const HEIGHT: usize = 120;

    const TOOLBAR: u32 = 0x00_12_34_56;
    const BACKGROUND: u32 = 0x00_F7_F7_F7;
    const BLACK: u32 = 0x00_00_00_00;
    const RED: u32 = 0x00_FF_00_00;

    // Fixed reference masks for the current 8x8 bitmap fallback.
    const A: [u8; 8] = [0x0C, 0x1E, 0x33, 0x33, 0x3F, 0x33, 0x33, 0x00];
    const B: [u8; 8] = [0x3F, 0x66, 0x66, 0x3E, 0x66, 0x66, 0x3F, 0x00];
    const C: [u8; 8] = [0x3C, 0x66, 0x03, 0x03, 0x03, 0x66, 0x3C, 0x00];

    let render = |html: &str| {
        let page = parse_html("https://example.test/", html);
        let layout = PreviewLayout::build(&page, WIDTH, None).unwrap();

        let mut pixels = vec![TOOLBAR; WIDTH * HEIGHT];

        paint_preview_scene(
            &mut XrgbFrame::new(&mut pixels, WIDTH, HEIGHT).unwrap(),
            &page,
            &layout,
            None,
            0,
            &mut GlyphRasterCache::new(),
        );

        pixels
    };

    let actual = render("<div>A <span style='color:red'>B</span> C</div>");
    let wrong_color = render("<div>A <span style='color:blue'>B</span> C</div>");

    let mut expected = vec![TOOLBAR; WIDTH * HEIGHT];
    expected[90 * WIDTH..].fill(BACKGROUND);

    // At this viewport width, C wraps onto the second line.
    let glyphs = [(16, 90, A, BLACK), (32, 90, B, RED), (16, 100, C, BLACK)];

    for (x, y, rows, color) in glyphs {
        for (dy, bits) in rows.into_iter().enumerate() {
            for dx in 0..8 {
                if bits & (1_u8 << dx) != 0 {
                    expected[(y + dy) * WIDTH + x + dx] = color;
                }
            }
        }
    }

    assert_eq!(actual, expected);
    assert_ne!(wrong_color, expected);
}

#[test]
fn preview_paints_bitmap_weight_between_words_and_wraps() {
    use crate::layout_preview::{PreviewCommand, PreviewLayout};
    use crate::render::{GlyphRasterCache, paint_preview_scene};

    let page = parse_html("https://example.test/", "<div>A <strong>B</strong> C</div>");
    let layout = PreviewLayout::build(&page, 64, None).unwrap();

    let runs = layout
        .display_list
        .iter()
        .find_map(|command| match command {
            PreviewCommand::Text { runs, .. } if !runs.is_empty() => Some(runs),
            _ => None,
        })
        .expect("positioned runs expected");

    assert_eq!(
        runs.iter().map(|run| run.text.as_str()).collect::<Vec<_>>(),
        vec!["A", "B", "C"]
    );

    assert_eq!(runs[0].style.font_weight, FontWeight::Normal);
    assert_eq!(runs[1].style.font_weight, FontWeight::Bold);

    assert_eq!((runs[0].x, runs[0].y), (0, 0));
    assert_eq!((runs[1].x, runs[1].y), (16, 0));
    assert_eq!((runs[2].x, runs[2].y), (0, 10));

    let mut pixels = vec![0; 64 * 120];

    paint_preview_scene(
        &mut XrgbFrame::new(&mut pixels, 64, 120).unwrap(),
        &page,
        &layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );

    assert!((90..100).any(|y| { (32..41).any(|x| pixels[y * 64 + x] == 0x00_00_00_00) }));
    assert!((100..110).any(|y| { (16..24).any(|x| pixels[y * 64 + x] == 0x00_00_00_00) }));

    let plain = parse_html("https://example.test/", "<div>A B C</div>");
    let plain_layout = PreviewLayout::build(&plain, 64, None).unwrap();
    let mut plain_pixels = vec![0; 64 * 120];

    paint_preview_scene(
        &mut XrgbFrame::new(&mut plain_pixels, 64, 120).unwrap(),
        &plain,
        &plain_layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );

    assert!(
        (90..100).any(|y| { (32..41).any(|x| pixels[y * 64 + x] != plain_pixels[y * 64 + x]) }),
        "bold bitmap must differ from normal"
    );
}

#[test]
fn preview_handles_vector_words_with_mixed_weight() {
    use crate::graphics::font::{
        TrueTypeFont, normalize_font_container,
        render::{GLYPH_SUBPIXEL_STEPS, split_subpixel_position},
    };
    use crate::layout_preview::{PreviewCommand, PreviewLayout};

    let inside = parse_html("https://example.test/", "<div>ߊ<strong>ߊ</strong></div>");
    let between = parse_html("https://example.test/", "<div>ߊ <strong>ߊ</strong></div>");

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/fonts/noto/nko-hmtx.woff2"
    );

    let font = normalize_font_container(std::fs::read(path).expect("Nko font fixture must exist"))
        .unwrap();

    let layout = PreviewLayout::build(&inside, 120, Some(&font)).unwrap();

    let runs = layout
        .display_list
        .iter()
        .find_map(|command| match command {
            PreviewCommand::Text { runs, .. } if runs.len() == 2 => Some(runs),
            _ => None,
        })
        .expect("two positioned runs expected");

    let face = TrueTypeFont::parse(&font).unwrap();
    let size = runs[0].style.font_size;
    let advance = face.text_advance_width("ߊ", size).unwrap();
    let glyph = face.glyph_id('ߊ');

    let kern = f32::from(face.kerning(glyph, glyph)) * size / f32::from(face.head().units_per_em);

    let (base, phase) = split_subpixel_position(advance + kern).unwrap();

    assert_eq!(runs[1].x, base);
    assert_eq!(
        runs[1].offset_x,
        f32::from(phase) / GLYPH_SUBPIXEL_STEPS as f32
    );
    assert_ne!(phase, 0);

    assert!(PreviewLayout::build(&between, 120, Some(&font)).is_ok());
}

#[test]
fn preview_paints_only_bold_word_as_a_positioned_run() {
    use crate::layout_preview::{PreviewCommand, PreviewLayout};

    let page = parse_html("https://example.test/", "<div><strong>A</strong></div>");
    let layout = PreviewLayout::build(&page, 120, None).unwrap();

    let run = layout
        .display_list
        .iter()
        .find_map(|command| match command {
            PreviewCommand::Text { runs, .. } => runs.first(),
            _ => None,
        })
        .expect("bold run expected");

    assert_eq!(run.style.font_weight, FontWeight::Bold);
}

#[test]
fn preview_bitmap_bold_run_does_not_add_gap_before_punctuation() {
    use crate::layout_preview::{PreviewCommand, PreviewLayout};
    use crate::render::{GlyphRasterCache, paint_preview_scene};

    let page = parse_html("https://example.test/", "<div><strong>A</strong>. B</div>");
    let layout = PreviewLayout::build(&page, 57, None).unwrap();

    let runs = layout
        .display_list
        .iter()
        .find_map(|command| match command {
            PreviewCommand::Text { runs, .. } if !runs.is_empty() => Some(runs),
            _ => None,
        })
        .expect("positioned runs expected");

    assert_eq!(
        runs.iter()
            .map(|run| (run.text.as_str(), run.x, run.y))
            .collect::<Vec<_>>(),
        vec![("A", 0, 0), (".", 8, 0), ("B", 0, 10)]
    );

    assert_eq!(runs[0].style.font_weight, FontWeight::Bold);
    assert_eq!(runs[1].style.font_weight, FontWeight::Normal);

    let mut pixels = vec![0; 57 * 120];

    paint_preview_scene(
        &mut XrgbFrame::new(&mut pixels, 57, 120).unwrap(),
        &page,
        &layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );

    assert!(
        (90..100).any(|y| { (24..32).any(|x| pixels[y * 57 + x] == 0x00_00_00_00) }),
        "period must be painted at the unshifted advance"
    );
}

#[test]
fn preview_positions_a_word_using_two_vector_faces() {
    use crate::graphics::font::{
        TrueTypeFont, normalize_font_container,
        render::{GLYPH_SUBPIXEL_STEPS, split_subpixel_position},
    };
    use crate::layout_preview::{PreviewCommand, PreviewLayout};
    use crate::render::{GlyphRasterCache, paint_preview_scene};

    let read = |name: &str| {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(name);

        normalize_font_container(std::fs::read(path).expect("font fixture must exist")).unwrap()
    };

    let regular = read("testdata/fonts/noto/nko-hmtx.woff2");
    let other = read("testdata/wpt/fonts/noto/NotoSansMongolian-regular.woff2");

    let mut page = parse_html(
        "https://example.test/",
        "<div style='font-family:test;font-size:32px'>ߊ<strong>ᠠ</strong></div>",
    );

    page.font_faces.push(LoadedFontFace {
        family: "test".into(),
        font_weight: FontWeight::Normal,
        font_style: FontStyle::Normal,
        data: regular,
    });

    page.font_faces.push(LoadedFontFace {
        family: "test".into(),
        font_weight: FontWeight::Bold,
        font_style: FontStyle::Normal,
        data: other,
    });

    let layout = PreviewLayout::build(&page, 120, None).unwrap();

    let runs = layout
        .display_list
        .iter()
        .find_map(|command| match command {
            PreviewCommand::Text { runs, .. } if runs.len() == 2 => Some(runs),
            _ => None,
        })
        .expect("two positioned runs expected");

    let font = TrueTypeFont::parse(&page.font_faces[0].data).unwrap();
    let advance = font.text_advance_width("ߊ", 32.0).unwrap();
    let (x, phase) = split_subpixel_position(advance).unwrap();

    assert_eq!(
        (runs[1].x, runs[1].offset_x),
        (x, f32::from(phase) / GLYPH_SUBPIXEL_STEPS as f32)
    );

    let mut pixels = vec![0; 120 * 160];

    paint_preview_scene(
        &mut XrgbFrame::new(&mut pixels, 120, 160).unwrap(),
        &page,
        &layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );

    page.font_faces.pop();

    let fallback_layout = PreviewLayout::build(&page, 120, None).unwrap();
    let mut fallback_pixels = vec![0; 120 * 160];

    paint_preview_scene(
        &mut XrgbFrame::new(&mut fallback_pixels, 120, 160).unwrap(),
        &page,
        &fallback_layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );

    assert_ne!(pixels, fallback_pixels);
}

#[test]
fn preview_synthesizes_font_style_only_when_face_is_missing() {
    use crate::graphics::font::normalize_font_container;
    use crate::layout_preview::PreviewLayout;
    use crate::render::{GlyphRasterCache, paint_preview_scene};

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/fonts/noto/nko-hmtx.woff2"
    );

    let data =
        normalize_font_container(std::fs::read(path).expect("font fixture must exist")).unwrap();

    let mut page = parse_html(
        "https://example.test/",
        "<div style='font-family:test;font-size:32px;font-weight:bold'>ߊ</div>",
    );

    for weight in [FontWeight::Normal, FontWeight::Bold] {
        page.font_faces.push(LoadedFontFace {
            family: "test".into(),
            font_weight: weight,
            font_style: FontStyle::Normal,
            data: data.clone(),
        });
    }

    let paint = |page: &Page| {
        let layout = PreviewLayout::build(page, 120, None).unwrap();
        let mut pixels = vec![0; 120 * 160];

        paint_preview_scene(
            &mut XrgbFrame::new(&mut pixels, 120, 160).unwrap(),
            page,
            &layout,
            None,
            0,
            &mut GlyphRasterCache::new(),
        );

        pixels
    };

    let actual_bold = paint(&page);

    page.font_faces.pop();

    let synthetic_bold = paint(&page);

    assert_ne!(actual_bold, synthetic_bold);

    let mut page = parse_html(
        "https://example.test/",
        "<div style='font-family:test;font-size:32px;font-style:italic'>ߊ</div>",
    );

    for font_style in [FontStyle::Normal, FontStyle::Italic] {
        page.font_faces.push(LoadedFontFace {
            family: "test".into(),
            font_weight: FontWeight::Normal,
            font_style,
            data: data.clone(),
        });
    }

    let actual_italic = paint(&page);

    page.font_faces.pop();

    let synthetic_italic = paint(&page);

    assert_ne!(actual_italic, synthetic_italic);
}
