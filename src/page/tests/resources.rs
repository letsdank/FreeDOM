//! Subresource fetching, URL resolution, image decoding and page preparation.

use super::*;

#[test]
fn resolves_relative_external_stylesheets() {
    let dom = HtmlDocument::parse(
        r#"
                <link rel="stylesheet" href="../assets/site.css">
                <link rel="icon" href="/favicon.ico">
                <link rel="stylesheet" href="https://cdn.example.test/theme.css">
            "#,
    );

    let page_url = Url::parse("https://example.test/docs/page.html").unwrap();
    let mut sources = Vec::new();

    collect_stylesheet_sources(&dom, &page_url, &mut sources);

    let urls: Vec<String> = sources
        .into_iter()
        .filter_map(|source| match source {
            StylesheetSource::External(url) => Some(url.to_string()),
            StylesheetSource::Inline(_) => None,
        })
        .collect();

    assert_eq!(
        urls,
        vec![
            "https://example.test/assets/site.css",
            "https://cdn.example.test/theme.css",
        ]
    );
}

#[test]
fn resolves_anchor_href_against_document_url() {
    let page = parse_html(
        "https://example.test/docs/guide/index.html",
        r#"<p>Read <a href="../about.html">about us</a>.</p>"#,
    );

    let BlockContent::Text(text) = &page.blocks[0].content else {
        panic!("expected text block");
    };

    assert_eq!(text.text, "Read about us.");

    assert!(text.runs.iter().any(|run| {
        text.text[run.range.clone()].contains("about us")
            && run.link.as_deref() == Some("https://example.test/docs/about.html")
    }));
}

#[test]
fn renders_external_svg_images_through_fetch_and_url_modules() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        read_request_head(&mut stream);

        let body = r##"
                <svg>
                    <defs>
                        <rect id="tile%20one" width="4" height="5" fill="green"/>
                    </defs>
                    <use href="#tile%2520one"/>
                </svg>
            "##;

        let response = format!(
            "HTTP/1.1 200 OK\r\n\
                 Content-Type: image/svg+xml\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n\
                 {}",
            body.len(),
            body,
        );

        stream.write_all(response.as_bytes()).unwrap();
    });

    let document = HtmlDocument::parse(r#"<img src="assets/icon.svg">"#);
    let fetcher = Fetcher::new().unwrap();

    let mut page = parse_dom_with_resources(
        &format!("http://{address}/page.html"),
        &document,
        &[],
        Vec::new(),
    );

    for request in page.image_requests() {
        let result = load_image(&fetcher, &request.url);
        page.apply_image_result(request.id, result);
    }

    server.join().unwrap();

    assert_eq!(page.blocks.len(), 1);

    let svg = page.blocks[0].svg().unwrap();
    assert_eq!(svg.shapes.len(), 1);
    assert_eq!(
        svg.shapes[0].fill,
        Some(crate::graphics::raster::Color::rgba(0, 128, 0, 255))
    );
}

#[test]
fn decodes_external_png_images_through_fetch_and_renderer_types() {
    let mut source = RasterSurface::new(2, 1, crate::graphics::raster::Color::TRANSPARENT);
    source.set_pixel(0, 0, crate::graphics::raster::Color::rgba(255, 0, 0, 255));
    source.set_pixel(1, 0, crate::graphics::raster::Color::rgba(0, 0, 255, 128));

    let png = crate::graphics::image::encode_png(&source).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();

    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        read_request_head(&mut stream);

        let mut response = format!(
            "HTTP/1.1 200 OK\r\n\
                Content-Type: image/png\r\n\
                Content-Length: {}\r\n\
                Connection: close\r\n\
                \r\n",
            png.len()
        )
        .into_bytes();

        response.extend_from_slice(&png);
        stream.write_all(response.as_slice()).unwrap();
    });

    let document = HtmlDocument::parse(r#"<img src="images/pixel.png">"#);
    let fetcher = Fetcher::new().unwrap();

    let mut page = parse_dom_with_resources(
        &format!("http://{address}/page.html"),
        &document,
        &[],
        Vec::new(),
    );

    for request in page.image_requests() {
        let result = load_image(&fetcher, &request.url);
        page.apply_image_result(request.id, result);
    }

    server.join().unwrap();

    let image = page.blocks[0].bitmap().unwrap();

    assert_eq!(image.width(), 2);
    assert_eq!(image.height(), 1);
    assert_eq!(
        image.pixel(0, 0),
        Some(crate::graphics::raster::Color::rgba(255, 0, 0, 255))
    );
    assert_eq!(
        image.pixel(1, 0),
        Some(crate::graphics::raster::Color::rgba(0, 0, 255, 128))
    );
}

#[test]
fn builds_page_from_prepend_navigation() {
    fn require_send<T: Send>() {}

    require_send::<PreparedPage>();

    let url = Url::parse("https://example.test/final").unwrap();

    let document = HtmlDocument::parse(
        "<title>Before</title>\
             <p>Hello</p>\
             <script>document.title = 'After';</script>",
    );

    let stylesheets = vec![Stylesheet {
        source: "p { color: #123456; }".to_owned(),
        base_url: url.clone(),
    }];

    let prepared = PreparedPage {
        final_url: url,
        document,
        stylesheets,
        font_faces: vec![LoadedFontFace {
            family: "Test".into(),
            font_weight: FontWeight::Normal,
            font_style: FontStyle::Normal,
            data: vec![1, 2, 3],
        }],
    };

    let page = build_prepared_page(prepared);

    assert_eq!(page.url, "https://example.test/final");
    assert_eq!(page.title, "After");
    assert_eq!(page.blocks[0].text(), Some("Hello"));
    assert_eq!(page.blocks[0].style.color, 0x00_12_34_56);
    assert_eq!(
        page.font_data_for(Some("Test"), FontWeight::Normal, FontStyle::Normal),
        Some([1, 2, 3].as_slice())
    );
}

#[test]
fn decodes_image_resource_without_network() {
    use crate::graphics::raster::Color;

    let url = Url::parse("https://example.test/assets/icon.svg").unwrap();

    let mut image = RasterSurface::new(1, 1, Color::TRANSPARENT);
    image.set_pixel(0, 0, Color::rgba(255, 0, 0, 255));

    let png = crate::graphics::image::encode_png(&image).unwrap();

    let Some(BlockContent::Bitmap(decoded)) = decode_image_resource(&png, Some("image/png"), &url)
    else {
        panic!("PNG expected");
    };

    assert_eq!(decoded.pixel(0, 0), Some(Color::rgba(255, 0, 0, 255)));

    let svg = br##"
            <svg>
                <defs>
                    <rect
                        id="tile%20one"
                        width="4"
                        height="5"
                        fill="green"
                    />
                </defs>
                <use href="#tile%2520one"/>
            </svg>
        "##;

    let Some(BlockContent::Svg(decoded)) = decode_image_resource(svg, Some("image/svg+xml"), &url)
    else {
        panic!("SVG expected");
    };

    assert_eq!(decoded.shapes.len(), 1);
    assert_eq!(decoded.shapes[0].fill, Some(Color::rgba(0, 128, 0, 255)));

    assert!(decode_image_resource(&png, Some("application/octet-stream"), &url).is_none());
    assert!(decode_image_resource(b"bad png", Some("image/png"), &url).is_none());
}

#[test]
fn queues_visible_image_requests_and_applies_out_of_order_results() {
    let mut page = parse_html(
        "https://example.test/a/page.html",
        r#"
                <p>Before</p>
                <img src='../one.png'>
                <img style='display:none' src='hidden.png'>
                <img src='two.png'>
                <p>After</p>
            "#,
    );

    let requests = page.image_requests();

    assert_eq!(requests.len(), 2);

    assert_eq!(requests[0].url.to_string(), "https://example.test/one.png");
    assert_eq!(
        requests[1].url.to_string(),
        "https://example.test/a/two.png"
    );

    assert_ne!(requests[0].id, requests[1].id);

    let layout = crate::page_layout::PageLayout::build(&page, 200, None);

    let first = page
        .blocks
        .iter()
        .position(|block| {
            matches!(
                &block.content,
                BlockContent::PendingImage(request)
                    if request.id == requests[0].id
            )
        })
        .unwrap();

    assert_eq!(layout.block_height(first), 0);

    let image = RasterSurface::new(1, 1, Color::BLACK);

    assert!(page.apply_image_result(requests[1].id, Some(BlockContent::Bitmap(image))));

    assert!(page.apply_image_result(requests[0].id, None));

    // A completed request cannot be applied twice.
    assert!(!page.apply_image_result(requests[1].id, None));

    assert!(page.image_requests().is_empty());
    assert_eq!(page.blocks.len(), 3);
    assert_eq!(page.blocks[0].text(), Some("Before"));
    assert!(page.blocks[1].bitmap().is_some());
    assert_eq!(page.blocks[2].text(), Some("After"));
}
