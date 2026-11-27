//! Web font loading, font size resolution, family fallback and glyph painting.

use super::*;

#[test]
fn selects_a_matching_loaded_font_face() {
    let face = |family: &str, weight, style, marker| LoadedFontFace {
        family: family.to_owned(),
        font_weight: weight,
        font_style: style,
        data: vec![marker],
    };

    let mut page = Page {
        url: "https://example.test/".to_owned(),
        title: String::new(),
        blocks: Vec::new(),
        font_faces: vec![
            face("Other", FontWeight::Bold, FontStyle::Italic, 9),
            face("FreeDOM Sans", FontWeight::Bold, FontStyle::Normal, 1),
            face("FreeDOM Sans", FontWeight::Normal, FontStyle::Italic, 2),
            face("FreeDOM Sans", FontWeight::Normal, FontStyle::Normal, 3),
        ],
        styled_tree: StyledTree::default(),
        document: DocumentState::default(),
    };

    for (weight, style, marker) in [
        (FontWeight::Bold, FontStyle::Italic, 2),
        (FontWeight::Normal, FontStyle::Italic, 2),
        (FontWeight::Bold, FontStyle::Normal, 1),
        (FontWeight::Normal, FontStyle::Normal, 3),
    ] {
        assert_eq!(
            page.font_data_for(Some("freedom sans"), weight, style),
            Some([marker].as_slice()),
            "weight={weight:?}, style={style:?}"
        );
    }

    assert!(
        page.font_data_for(Some("missing"), FontWeight::Normal, FontStyle::Normal)
            .is_none()
    );

    page.font_faces
        .push(face("FreeDOM Sans", FontWeight::Bold, FontStyle::Italic, 4));

    assert_eq!(
        page.font_data_for(Some("freedom sans"), FontWeight::Bold, FontStyle::Italic),
        Some([4].as_slice())
    );

    let mut style = TextStyle::initial();

    style.font_families = vec!["missing".into(), "Other".into(), "freedom sans".into()];
    style.font_weight = FontWeight::Bold;
    style.font_style = FontStyle::Italic;

    let data: Vec<_> = page.font_data_for_families(&style).collect();

    assert_eq!(data, vec![[9].as_slice(), [4].as_slice()]);
    assert_eq!(page.font_data_for_style(&style), Some([9].as_slice()));
}

#[test]
fn loads_a_woff_font_face_through_css_and_fetch() {
    let table = [0_u8, 1, 2, 3];
    let woff = woff_with_name_table(&table);

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_request_head(&mut stream);

        assert!(request.contains("GET /fonts/test.woff "));

        let response = format!(
            "HTTP/1.1 200 OK\r\n\
                Content-Type: font/woff\r\n\
                Content-Length: {}\r\n\
                Connection: close\r\n\
                \r\n",
            woff.len(),
        );

        stream.write_all(response.as_bytes()).unwrap();
        stream.write_all(&woff).unwrap();
    });

    let page_url = format!("http://{address}/page.html");
    let base_url = Url::parse(&page_url).unwrap();
    let document = HtmlDocument::parse("<p>FreeDOM</p>");
    let stylesheets = vec![Stylesheet {
        source: r#"
                @font-face {
                    font-family: Test;
                    src: url("fonts/test.woff");
                }

                p {
                    font-family: Test;
                }
            "#
        .to_owned(),
        base_url,
    }];
    let fetcher = Fetcher::new().unwrap();
    let font_faces = load_author_fonts(&fetcher, &stylesheets);

    let page = parse_dom_with_resources(&page_url, &document, &stylesheets, font_faces);

    server.join().unwrap();

    assert_eq!(
        page.blocks[0]
            .style
            .font_families
            .first()
            .map(String::as_str),
        Some("Test")
    );

    let font_data = page
        .font_data_for(Some("Test"), FontWeight::Normal, FontStyle::Normal)
        .unwrap();

    let font = crate::graphics::font::sfnt::SfntFont::parse(font_data).unwrap();

    assert_eq!(
        font.table(crate::graphics::font::sfnt::TableTag::NAME),
        Some(table.as_slice())
    );
}

#[test]
fn loads_and_rasterizes_woff2_font_from_external_stylesheet() {
    use crate::graphics::{
        font::{TrueTypeFont, rasterize_text_outline},
        geometry::Transform,
        raster::{Color, Surface},
    };
    use std::{
        io::ErrorKind,
        time::{Duration, Instant},
    };

    let font_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/fonts/noto/nko-hmtx.woff2",
    );

    let font_bytes = std::fs::read(font_path).expect("transformed hmtx test font must exist");

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();

    let address = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut requests = Vec::new();

        while requests.len() < 4 {
            let mut stream = match listener.accept() {
                Ok((stream, _)) => stream,

                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for page resources; received: {requests:?}"
                    );

                    thread::sleep(Duration::from_millis(5));
                    continue;
                }

                Err(error) => panic!("failed to accpt connection: {error}"),
            };

            let request = read_request_head(&mut stream);

            let request_path = request
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_owned();

            let (content_type, body): (&str, &[u8]) = match request_path.as_str() {
                "/page/index.html" => (
                    "text/html; charset=utf-8",
                    concat!(
                        "<html><head>",
                        "<link rel=\"stylesheet\" href=\"../assets/main.css\">",
                        "</head><body><p>ߊ</p></body></html>",
                    )
                    .as_bytes(),
                ),

                "/assets/main.css" => (
                    "text/css; charset=utf-8",
                    b"@font-face { \
                            font-family: 'Nko, Test'; \
                            src: url('fonts/invalid.ttf'), url('fonts/nko.woff2') format('woff2'); \
                        } \
                        p { font-family: 'Nko, Test', sans-serif; }",
                ),

                "/assets/fonts/nko.woff2" => ("font/woff2", &font_bytes),
                "/assets/fonts/invalid.ttf" => ("font/ttf", b"not a font"),

                _ => panic!("unexpected request: {request_path}"),
            };

            let headers = format!(
                "HTTP/1.1 200 OK\r\n\
                    Content-Type: {content_type}\r\n\
                    Content-Length: {}\r\n\
                    Connection: close\r\n\
                    \r\n",
                body.len(),
            );

            stream.write_all(headers.as_bytes()).unwrap();
            stream.write_all(body).unwrap();

            requests.push(request_path);
        }

        requests
    });

    let result = load_page(&format!("http://{address}/page/index.html"));
    let requests = server.join().unwrap();
    let mut page = result.unwrap();

    assert_eq!(
        requests,
        [
            "/page/index.html",
            "/assets/main.css",
            "/assets/fonts/invalid.ttf",
            "/assets/fonts/nko.woff2",
        ]
    );

    let block = page
        .blocks
        .iter()
        .find(|block| block.text() == Some("ߊ"))
        .expect("text block must exist");

    assert_eq!(
        block.style.font_families.first().map(String::as_str),
        Some("Nko, Test")
    );

    let font_data = page
        .font_data_for(
            block.style.font_families.first().map(String::as_str),
            block.style.font_weight,
            block.style.font_style,
        )
        .expect("font must be loaded through @font-face");

    let font = TrueTypeFont::parse(font_data).unwrap();

    assert_eq!(font.glyph_id('\u{07CA}'), 20);

    let mut outline = font.outline_text("ߊ", 64.0).unwrap();
    outline.path = outline.path.transformed(Transform::translate(0.0, 96.0));

    let mut surface = Surface::new(128, 128, Color::TRANSPARENT);

    rasterize_text_outline(&mut surface, &outline, Color::BLACK).unwrap();

    assert_eq!(surface.pixel(8, 73), Some(Color::BLACK));
    assert_eq!(surface.pixel(0, 0), Some(Color::TRANSPARENT));

    use crate::{
        page_layout::{CONTENT_TOP, PageLayout},
        render::{BlockRasterCache, GlyphRasterCache, paint},
    };

    let width = 256;
    let height = 200;

    let layout = PageLayout::build(&page, width, None);
    let mut pixels = vec![0; width * height];

    let mut block_cache = BlockRasterCache::new();
    let mut glyph_cache = GlyphRasterCache::new();

    paint(
        &mut XrgbFrame::new(&mut pixels, width, height).unwrap(),
        &page,
        &layout,
        PaintOptions::default(),
        &mut block_cache,
        &mut glyph_cache,
    );

    // A cached repaint must produce the same image.
    let mut cached_pixels = vec![0; width * height];

    paint(
        &mut XrgbFrame::new(&mut cached_pixels, width, height).unwrap(),
        &page,
        &layout,
        PaintOptions::default(),
        &mut block_cache,
        &mut glyph_cache,
    );

    assert_eq!(pixels, cached_pixels);

    // Only inspect page content, excluding the toolbar.
    let content_start = CONTENT_TOP as usize * width;
    let background = 0x00_F7_F7_F7;

    assert!(
        pixels[content_start..]
            .iter()
            .any(|&pixel| pixel != background),
        "the page content must contain rendered pixels"
    );

    // Re-render without the downloaded font to verify that it affects the output.
    page.font_faces.clear();

    let fallback_layout = PageLayout::build(&page, width, None);
    let mut fallback_pixels = vec![0; width * height];

    paint(
        &mut XrgbFrame::new(&mut fallback_pixels, width, height).unwrap(),
        &page,
        &fallback_layout,
        PaintOptions::default(),
        &mut BlockRasterCache::new(),
        &mut GlyphRasterCache::new(),
    );

    assert_ne!(
        &pixels[content_start..],
        &fallback_pixels[content_start..],
        "the downloaded font must affect page rendering"
    );
}

fn woff_with_name_table(table: &[u8]) -> Vec<u8> {
    assert_eq!(table.len() % 4, 0);

    const WOFF_HEADER_LENGTH: usize = 44;
    const WOFF_TABLE_DIRECTORY_ENTRY_LENGTH: usize = 20;
    const SFNT_HEADER_LENGTH: usize = 12;
    const SFNT_TABLE_DIRECTORY_ENTRY_LENGTH: usize = 16;

    let table_offset = WOFF_HEADER_LENGTH + WOFF_TABLE_DIRECTORY_ENTRY_LENGTH;
    let file_length = table_offset + table.len();
    let total_sfnt_size = SFNT_HEADER_LENGTH + SFNT_TABLE_DIRECTORY_ENTRY_LENGTH + table.len();

    let mut data = vec![0_u8; file_length];

    data[0..4].copy_from_slice(b"wOFF");
    data[4..8].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
    data[8..12].copy_from_slice(&(file_length as u32).to_be_bytes());
    data[12..14].copy_from_slice(&1_u16.to_be_bytes());
    data[16..20].copy_from_slice(&(total_sfnt_size as u32).to_be_bytes());
    data[20..22].copy_from_slice(&1_u16.to_be_bytes());

    data[44..48].copy_from_slice(b"name");
    data[48..52].copy_from_slice(&(table_offset as u32).to_be_bytes());
    data[52..56].copy_from_slice(&(table.len() as u32).to_be_bytes());
    data[56..60].copy_from_slice(&(table.len() as u32).to_be_bytes());

    data[table_offset..].copy_from_slice(table);

    data
}

#[test]
fn zero_font_size_does_not_draw_bitmap_fallback() {
    use crate::{
        page_layout::{CONTENT_TOP, PageLayout},
        render::{BlockRasterCache, GlyphRasterCache, paint},
    };

    let page = parse_html(
        "https://example.test",
        r#"<p style="font-size: 0">Visible?</p>"#,
    );

    assert_eq!(page.blocks.len(), 1);
    assert_eq!(page.blocks[0].style.font_size, 0.0);

    let width = 200;
    let height = 160;

    let layout = PageLayout::build(&page, width, None);

    assert_eq!(layout.block_height(0), 0);

    let mut pixels = vec![0; width * height];

    paint(
        &mut XrgbFrame::new(&mut pixels, width, height).unwrap(),
        &page,
        &layout,
        PaintOptions::default(),
        &mut BlockRasterCache::new(),
        &mut GlyphRasterCache::new(),
    );

    let content_start = CONTENT_TOP as usize * width;

    assert!(
        pixels[content_start..]
            .iter()
            .all(|&pixel| pixel == 0x00_F7_F7_F7)
    );
}

#[test]
fn percentage_font_size_flows_through_page_styles() {
    let page = parse_html(
        "https://example.test",
        r#"
                <style>
                    div { font-size: 20px }
                    p { font-size: 150% }
                </style>
                <div><p>Hello</p></div>
            "#,
    );

    assert_eq!(page.blocks[0].style.font_size, 30.0);
}

#[test]
fn font_size_inherit_flows_through_page_styles() {
    let page = parse_html(
        "https://example.test",
        r#"
                <style>
                    div { font-size: 24px }
                    p { font-size: 30px; font-size: inherit }
                </style>
                <div><p>Hello</p></div>
            "#,
    );

    assert_eq!(page.blocks[0].style.font_size, 24.0);
}

#[test]
fn absolute_font_size_flows_through_page_styles() {
    let page = parse_html(
        "https://example.test",
        r#"
                <style>p { font-size: x-large }</style>
                <p>Hello</p>
            "#,
    );

    assert_eq!(page.blocks[0].style.font_size, 24.0);
}

#[test]
fn relative_font_size_flows_through_page_styles() {
    let page = parse_html(
        "https://example.test",
        r#"
                <style>
                    div { font-size: 20px }
                    p { font-size: larger }
                </style>
                <div><p>Hello</p></div>
            "#,
    );

    assert!((page.blocks[0].style.font_size - 24.0).abs() < 0.001);
}

#[test]
fn falls_back_to_next_loaded_font_family() {
    use crate::graphics::font::normalize_font_container;
    use crate::layout_preview::PreviewLayout;
    use crate::render::{GlyphRasterCache, paint_preview_scene};

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/fonts/noto/nko-hmtx.woff2"
    );

    let font =
        normalize_font_container(std::fs::read(path).expect("font fixture must exist")).unwrap();

    let make_page = |family: &str| {
        let html = format!("<div style=\"font-family: {family};font-size:32px\">ߊ</div>");

        let mut page = parse_html("https://example.test/", &html);

        page.font_faces.push(LoadedFontFace {
            family: "Nko, Test".into(),
            font_weight: FontWeight::Normal,
            font_style: FontStyle::Normal,
            data: font.clone(),
        });

        page
    };

    let fallback = make_page("Missing, 'Nko, Test'");
    let direct = make_page("'Nko, Test'");

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

    assert_eq!(
        fallback.blocks[0].style.font_families,
        vec!["Missing", "Nko, Test"]
    );

    assert!(
        fallback
            .font_data_for_style(&fallback.blocks[0].style)
            .is_some()
    );

    assert_eq!(paint(&fallback), paint(&direct));
}

#[test]
fn page_renders_missing_glyph_from_next_family() {
    use crate::graphics::font::normalize_font_container;
    use crate::page_layout::PageLayout;
    use crate::render::{BlockRasterCache, GlyphRasterCache, paint};

    let read = |path: &str| {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);

        normalize_font_container(std::fs::read(path).expect("font fixture must exist")).unwrap()
    };

    let a = read("testdata/fonts/noto/nko-hmtx.woff2");
    let b = read("testdata/wpt/fonts/noto/NotoSansMongolian-regular.woff2");

    let mut page = parse_html(
        "https://example.test/",
        "<div style='font-family:A,B;font-size:32px'>ߊᠠ</div>",
    );

    for (family, data) in [("A", a), ("B", b)] {
        page.font_faces.push(LoadedFontFace {
            family: family.into(),
            font_weight: FontWeight::Normal,
            font_style: FontStyle::Normal,
            data,
        });
    }

    let fonts = page.vector_fonts_for_style(&page.blocks[0].style);

    assert_eq!(fonts.len(), 2);
    assert_eq!(fonts[0].font.glyph_id('ᠠ'), 0);
    assert_ne!(fonts[1].font.glyph_id('ᠠ'), 0);

    let font_refs: Vec<_> = fonts.iter().map(|entry| &entry.font).collect();

    let expected = crate::graphics::font::layout::visit_positioned_glyphs(
        &font_refs,
        "ߊᠠ",
        32.0,
        |_, _, _| Ok(()),
    )
    .unwrap();

    assert_eq!(
        crate::page_layout::stack_word_width(
            Some(&fonts[0].font),
            &fonts,
            "ߊᠠ",
            32.0,
            FontWeight::Normal,
            FontStyle::Normal,
        ),
        expected.ceil() as i32,
    );

    let draw = |page: &Page| {
        let layout = PageLayout::build(page, 120, None);
        let mut pixels = vec![0; 120 * 200];

        paint(
            &mut XrgbFrame::new(&mut pixels, 120, 200).unwrap(),
            page,
            &layout,
            PaintOptions::default(),
            &mut BlockRasterCache::new(),
            &mut GlyphRasterCache::new(),
        );

        pixels
    };

    let fallback = draw(&page);

    let preview = crate::layout_preview::PreviewLayout::build(&page, 120, None).unwrap();

    let mut preview_pixels = vec![0; 120 * 200];

    crate::render::paint_preview_scene(
        &mut XrgbFrame::new(&mut preview_pixels, 120, 200).unwrap(),
        &page,
        &preview,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );

    assert_eq!(&fallback[90 * 120..], &preview_pixels[90 * 120..]);

    let mut split = parse_html(
        "https://example.test/",
        "<div style='font-family:A,b;font-size:32px'>ߊ<span>ᠠ</span></div>",
    );

    for face in &page.font_faces {
        split.font_faces.push(LoadedFontFace {
            family: face.family.clone(),
            font_weight: face.font_weight,
            font_style: face.font_style,
            data: face.data.clone(),
        });
    }

    let split_layout = crate::layout_preview::PreviewLayout::build(&split, 120, None).unwrap();

    let mut split_pixels = vec![0; 120 * 200];

    crate::render::paint_preview_scene(
        &mut XrgbFrame::new(&mut split_pixels, 120, 200).unwrap(),
        &split,
        &split_layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );

    assert_eq!(&fallback[90 * 120..], &split_pixels[90 * 120..]);

    let mut mixed_weight = parse_html(
        "https://example.test/",
        "<div style='font-family:A,B;font-size:32px'>ߊᠠ<strong>ᠠߊ</strong></div>",
    );

    for face in &page.font_faces {
        mixed_weight.font_faces.push(LoadedFontFace {
            family: face.family.clone(),
            font_weight: face.font_weight,
            font_style: face.font_style,
            data: face.data.clone(),
        });
    }

    let layout = crate::layout_preview::PreviewLayout::build(&mixed_weight, 120, None).unwrap();

    let runs = layout
        .display_list
        .iter()
        .find_map(|command| match command {
            crate::layout_preview::PreviewCommand::Text { runs, .. } if runs.len() == 2 => {
                Some(runs)
            }

            _ => None,
        })
        .expect("two positioned runs expected");

    let mut positions = Vec::new();

    crate::graphics::font::layout::visit_positioned_glyphs(
        &fonts,
        "ߊᠠᠠߊ",
        32.0,
        |index, _, x| {
            positions.push((index, x));
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(
        positions.iter().map(|item| item.0).collect::<Vec<_>>(),
        [0, 1, 1, 0]
    );

    let (x, phase) =
        crate::graphics::font::render::split_subpixel_position(positions[2].1).unwrap();

    assert_eq!(
        (runs[1].x, runs[1].offset_x),
        (
            x,
            f32::from(phase) / crate::graphics::font::render::GLYPH_SUBPIXEL_STEPS as f32
        )
    );

    assert_eq!(runs[1].style.font_weight, FontWeight::Bold);

    let mut mixed_pixels = vec![0; 120 * 200];

    crate::render::paint_preview_scene(
        &mut XrgbFrame::new(&mut mixed_pixels, 120, 200).unwrap(),
        &mixed_weight,
        &layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );

    let mut plain = parse_html(
        "https://example.test/",
        "<div style='font-family:A,B;font-size:32px'>ߊᠠᠠߊ</div>",
    );

    for face in &page.font_faces {
        plain.font_faces.push(LoadedFontFace {
            family: face.family.clone(),
            font_weight: face.font_weight,
            font_style: face.font_style,
            data: face.data.clone(),
        });
    }

    let plain_layout = crate::layout_preview::PreviewLayout::build(&plain, 120, None).unwrap();

    let mut plain_pixels = vec![0; 120 * 200];

    crate::render::paint_preview_scene(
        &mut XrgbFrame::new(&mut plain_pixels, 120, 200).unwrap(),
        &plain,
        &plain_layout,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );

    assert_ne!(mixed_pixels, plain_pixels);

    let mut styled = parse_html(
        "https://example.test/",
        concat!(
            "<div style='font-family:A,B;font-size:32px;",
            "font-weight:bold;font-style:italic'>ߊᠠ</div>"
        ),
    );

    for face in &page.font_faces {
        styled.font_faces.push(LoadedFontFace {
            family: face.family.clone(),
            font_weight: face.font_weight,
            font_style: face.font_style,
            data: face.data.clone(),
        });
    }

    assert_eq!(
        styled.vector_fonts_for_style(&styled.blocks[0].style).len(),
        2
    );

    let synthetic = draw(&styled);

    let preview = crate::layout_preview::PreviewLayout::build(&styled, 120, None).unwrap();
    let mut styled_preview = vec![0; 120 * 200];

    crate::render::paint_preview_scene(
        &mut XrgbFrame::new(&mut styled_preview, 120, 200).unwrap(),
        &styled,
        &preview,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );

    assert_eq!(&synthetic[90 * 120..], &styled_preview[90 * 120..]);

    styled.font_faces.pop();

    let missing_synthetic = draw(&styled);

    assert_ne!(synthetic, missing_synthetic);

    // The files would require different synthesis settings.
    styled.font_faces.push(LoadedFontFace {
        family: "B".into(),
        font_weight: FontWeight::Bold,
        font_style: FontStyle::Italic,
        data: page.font_faces[1].data.clone(),
    });

    assert_eq!(
        styled.vector_fonts_for_style(&styled.blocks[0].style).len(),
        2
    );

    let resolved = styled.vector_fonts_for_style(&styled.blocks[0].style);

    assert_eq!(resolved[0].synthesis, (FontWeight::Bold, FontStyle::Italic));
    assert_eq!(
        resolved[1].synthesis,
        (FontWeight::Normal, FontStyle::Normal)
    );

    let mixed = draw(&styled);
    assert_ne!(mixed, synthetic);

    let preview = crate::layout_preview::PreviewLayout::build(&styled, 120, None).unwrap();

    let mut pixels = vec![0; 120 * 200];

    crate::render::paint_preview_scene(
        &mut XrgbFrame::new(&mut pixels, 120, 200).unwrap(),
        &styled,
        &preview,
        None,
        0,
        &mut GlyphRasterCache::new(),
    );

    assert_eq!(&mixed[90 * 120..], &pixels[90 * 120..]);

    styled.font_faces[0].font_weight = FontWeight::Bold;
    styled.font_faces[0].font_style = FontStyle::Italic;
    styled.font_faces[1].font_weight = FontWeight::Normal;
    styled.font_faces[1].font_style = FontStyle::Normal;

    let resolved = styled.vector_fonts_for_style(&styled.blocks[0].style);

    assert_eq!(
        resolved[0].synthesis,
        (FontWeight::Normal, FontStyle::Normal)
    );
    assert_eq!(resolved[1].synthesis, (FontWeight::Bold, FontStyle::Italic));

    let reverse = draw(&styled);
    assert_ne!(reverse, mixed);

    page.font_faces.pop();

    let missing = draw(&page);

    assert_ne!(fallback, missing);
}

#[test]
fn page_keeps_color_and_kerning_inside_word() {
    use crate::graphics::font::{
        TrueTypeFont, normalize_font_container,
        render::{GLYPH_SUBPIXEL_STEPS, split_subpixel_position},
    };
    use crate::page_layout::PageLayout;
    use crate::render::{BlockRasterCache, GlyphRasterCache, paint};

    let page = parse_html(
        "https://example.test/",
        "<div>ߊ<span style='color:red'>ߊ</span></div>",
    );

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/fonts/noto/nko-hmtx.woff2"
    );

    let data = normalize_font_container(std::fs::read(path).unwrap()).unwrap();
    let font = TrueTypeFont::parse(&data).unwrap();
    let layout = PageLayout::build(&page, 120, Some(&data));

    let runs = layout
        .block_text_runs(0)
        .expect("styled text runs expected");

    assert_eq!(runs.len(), 2);

    let size = runs[0].style.font_size;
    let glyph = font.glyph_id('ߊ');
    let advance = font.text_advance_width("ߊ", size).unwrap();

    let kern = f32::from(font.kerning(glyph, glyph)) * size / f32::from(font.head().units_per_em);

    let (x, phase) = split_subpixel_position(advance + kern).unwrap();

    assert_eq!(
        (runs[1].x, runs[1].offset_x),
        (x, f32::from(phase) / GLYPH_SUBPIXEL_STEPS as f32)
    );

    assert_eq!(runs[1].style.color, 0x00ff0000);

    let mut pixels = vec![0; 120 * 150];

    paint(
        &mut XrgbFrame::new(&mut pixels, 120, 150).unwrap(),
        &page,
        &layout,
        PaintOptions {
            fallback_font_data: Some(&data),
            ..PaintOptions::default()
        },
        &mut BlockRasterCache::new(),
        &mut GlyphRasterCache::new(),
    );

    assert!(
        pixels[90 * 120..]
            .iter()
            .any(|pixel| *pixel & 0x00ff0000 != 0 && *pixel & 0x0000ff00 == 0)
    );

    let bitmap = parse_html(
        "https://example.test/",
        "<div>A<span style='color:red'>B</span> C</div>",
    );

    let layout = PageLayout::build(&bitmap, 48, None);
    let runs = layout.block_text_runs(0).unwrap();

    assert_eq!(
        runs.iter().map(|run| run.text.as_str()).collect::<Vec<_>>(),
        ["A", "B", "C"]
    );

    assert_eq!((runs[0].y, runs[1].y), (0, 0));
    assert!(runs[2].y > runs[1].y);

    let mut pixels = vec![0; 48 * 130];

    paint(
        &mut XrgbFrame::new(&mut pixels, 48, 130).unwrap(),
        &bitmap,
        &layout,
        PaintOptions::default(),
        &mut BlockRasterCache::new(),
        &mut GlyphRasterCache::new(),
    );

    assert!(pixels[90 * 48..].contains(&0x00ff0000));
}
