use crate::{
    css::{CssRule, Display, Element as CssElement, TextStyle, build_author_rules, compute_style},
    fetch::Fetcher,
    graphics::{
        image::decode_png,
        raster::Surface as RasterSurface,
        svg::{SvgDocument, parse_svg_at, parse_svg_element, parse_svg_element_at},
    },
    html::{Document as HtmlDocument, Element as HtmlElement, Node as HtmlNode},
    javascript::{DocumentState, evaluate_with_document},
    url::Url,
};
use anyhow::Result;

pub(crate) enum BlockContent {
    Text(String),
    Svg(SvgDocument),
    Bitmap(RasterSurface),
}

pub(crate) struct Block {
    pub(crate) content: BlockContent,
    pub(crate) style: TextStyle,
    pub(crate) link: Option<String>,
}

impl Block {
    fn text(&self) -> Option<&str> {
        match &self.content {
            BlockContent::Text(text) => Some(text),
            BlockContent::Svg(_) | BlockContent::Bitmap(_) => None,
        }
    }

    fn svg(&self) -> Option<&SvgDocument> {
        match &self.content {
            BlockContent::Svg(svg) => Some(svg),
            BlockContent::Text(_) | BlockContent::Bitmap(_) => None,
        }
    }

    fn bitmap(&self) -> Option<&RasterSurface> {
        match &self.content {
            BlockContent::Bitmap(bitmap) => Some(bitmap),
            BlockContent::Text(_) | BlockContent::Svg(_) => None,
        }
    }
}

pub(crate) struct Page {
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) blocks: Vec<Block>,
}

struct ElementInfo {
    tag: String,
    id: Option<String>,
    classes: Vec<String>,
    inline_style: Option<String>,
    href: Option<String>,
    src: Option<String>,
}

impl ElementInfo {
    fn css_element(&self) -> CssElement<'_> {
        CssElement {
            tag: &self.tag,
            id: self.id.as_deref(),
            classes: &self.classes,
            inline_style: self.inline_style.as_deref(),
        }
    }
}

enum StylesheetSource {
    Inline(String),
    External(Url),
}

pub(crate) fn load_page(input: &str) -> Result<Page> {
    let input = if input.contains("://") {
        input.to_owned()
    } else {
        format!("https://{input}")
    };

    let url = Url::parse(&input)?;
    let fetcher = Fetcher::new()?;
    let response = fetcher.get(&url)?;

    let html = response.text_lossy();
    let final_url = response.url;

    let document = HtmlDocument::parse(&html);
    let stylesheets = load_author_stylesheets(&fetcher, &final_url, &document);
    let final_url_text = final_url.to_string();

    Ok(parse_dom_with_fetcher(
        &final_url_text,
        &document,
        &stylesheets,
        Some(&fetcher),
    ))
}

#[cfg(test)]
fn parse_html(url: &str, html: &str) -> Page {
    let document = HtmlDocument::parse(html);
    let stylesheets = collect_inline_stylesheets(&document);

    parse_dom(url, &document, &stylesheets)
}

fn parse_dom(url: &str, document: &HtmlDocument, stylesheets: &[String]) -> Page {
    parse_dom_with_fetcher(url, document, stylesheets, None)
}

fn parse_dom_with_fetcher(
    url: &str,
    document: &HtmlDocument,
    stylesheets: &[String],
    fetcher: Option<&Fetcher>,
) -> Page {
    let mut document_state = DocumentState::with_title(find_title(document).unwrap_or_default());

    register_document_elements(document, &mut document_state);
    run_inline_scripts(document, &mut document_state);

    let title = if document_state.title.is_empty() {
        url.to_owned()
    } else {
        document_state.title.clone()
    };

    let rules = build_author_rules(stylesheets);
    let base_style = TextStyle::initial();

    let mut blocks = Vec::new();
    let mut current_text = String::new();
    let page_url = Url::parse(url).ok();

    for child in &document.children {
        extract_blocks(
            child,
            &rules,
            &mut blocks,
            &mut current_text,
            base_style,
            page_url.as_ref(),
            fetcher,
            &document_state,
        );
    }

    flush(&mut blocks, &mut current_text, base_style);

    Page {
        url: url.to_owned(),
        title,
        blocks,
    }
}

fn find_title(document: &HtmlDocument) -> Option<String> {
    document.children.iter().find_map(find_title_in_node)
}

fn find_title_in_node(node: &HtmlNode) -> Option<String> {
    let HtmlNode::Element(element) = node else {
        return None;
    };

    if element.name == "title" {
        return Some(text_content(node).trim().to_owned());
    }

    element.children.iter().find_map(find_title_in_node)
}

fn text_content(node: &HtmlNode) -> String {
    match node {
        HtmlNode::Text(text) => text.clone(),

        HtmlNode::Element(element) => element.children.iter().map(text_content).collect(),

        HtmlNode::Doctype(_) | HtmlNode::Comment(_) => String::new(),
    }
}

fn register_document_elements(root: &HtmlDocument, document: &mut DocumentState) {
    for child in &root.children {
        register_document_elements_in_node(child, None, document)
    }
}

fn register_document_elements_in_node(
    node: &HtmlNode,
    parent_node_id: Option<u64>,
    document: &mut DocumentState,
) {
    let HtmlNode::Element(element) = node else {
        return;
    };

    let node_id = document.register_parsed_element(
        &element.name,
        html_attribute(element, "id").unwrap_or(""),
        text_content(node),
        parent_node_id,
    );

    for attribute in &element.attributes {
        document.register_element_attribute(node_id, &attribute.name, attribute.value.clone());
    }

    for child in &element.children {
        register_document_elements_in_node(child, Some(node_id), document);
    }
}

fn run_inline_scripts(root: &HtmlDocument, document: &mut DocumentState) {
    for child in &root.children {
        run_inline_scripts_in_node(child, document);
    }
}

fn run_inline_scripts_in_node(node: &HtmlNode, document: &mut DocumentState) {
    let HtmlNode::Element(element) = node else {
        return;
    };

    if element.name == "script" {
        let is_inline = html_attribute(element, "src").is_none();

        if is_inline && is_classic_javascript(element) {
            let source = text_content(node);

            if !source.trim().is_empty() {
                if let Err(error) = evaluate_with_document(&source, document) {
                    eprintln!("FreeDOM: inline script error: {error}");
                }
            }
        }

        return;
    }

    for child in &element.children {
        run_inline_scripts_in_node(child, document);
    }
}

fn is_classic_javascript(element: &HtmlElement) -> bool {
    let Some(kind) = html_attribute(element, "type") else {
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

#[cfg(test)]
fn collect_inline_stylesheets(root: &HtmlDocument) -> Vec<String> {
    let mut stylesheets = Vec::new();

    for child in &root.children {
        collect_inline_stylesheets_from_node(child, &mut stylesheets);
    }

    stylesheets
}

#[cfg(test)]
fn collect_inline_stylesheets_from_node(node: &HtmlNode, stylesheets: &mut Vec<String>) {
    let HtmlNode::Element(element) = node else {
        return;
    };

    if element.name == "style" {
        stylesheets.push(text_content(node));
    }

    for child in &element.children {
        collect_inline_stylesheets_from_node(child, stylesheets);
    }
}

fn load_author_stylesheets(fetcher: &Fetcher, page_url: &Url, root: &HtmlDocument) -> Vec<String> {
    let mut sources = Vec::new();
    collect_stylesheet_sources(root, page_url, &mut sources);

    sources
        .into_iter()
        .filter_map(|source| match source {
            StylesheetSource::Inline(css) => Some(css),

            StylesheetSource::External(url) => match fetcher.get(&url) {
                Ok(response) => Some(response.text_lossy()),

                Err(error) => {
                    eprintln!("FreeDOM: failed to load stylesheet {url}: {error}");
                    None
                }
            },
        })
        .collect()
}

fn load_image(fetcher: &Fetcher, page_url: &Url, source: &str) -> Option<BlockContent> {
    let resource_url = page_url.join(source.trim()).ok()?;
    let response = fetcher.get(&resource_url).ok()?;

    if response.status >= 400 {
        return None;
    }

    match response
        .content_type
        .as_ref()
        .map(|content_type| content_type.mime_type.as_str())
    {
        Some("image/svg+xml") => {
            let source = response.text_lossy();
            parse_svg_at(&source, &response.url)
                .ok()
                .map(BlockContent::Svg)
        }

        Some("image/png") => decode_png(response.bytes()).ok().map(BlockContent::Bitmap),

        _ => None,
    }
}

fn collect_stylesheet_sources(
    root: &HtmlDocument,
    page_url: &Url,
    sources: &mut Vec<StylesheetSource>,
) {
    for child in &root.children {
        collect_stylesheet_sources_from_node(child, page_url, sources);
    }
}

fn collect_stylesheet_sources_from_node(
    node: &HtmlNode,
    page_url: &Url,
    sources: &mut Vec<StylesheetSource>,
) {
    let HtmlNode::Element(element) = node else {
        return;
    };

    if element.name == "style" {
        sources.push(StylesheetSource::Inline(text_content(node)));
    }

    if element.name == "link" {
        let rel = html_attribute(element, "rel");
        let href = html_attribute(element, "href");

        let is_stylesheet = rel
            .map(|rel| {
                rel.split_ascii_whitespace()
                    .any(|value| value.eq_ignore_ascii_case("stylesheet"))
            })
            .unwrap_or(false);

        if is_stylesheet {
            if let Some(href) = href {
                if let Ok(url) = page_url.join(href.trim()) {
                    if matches!(url.scheme(), "http" | "https") {
                        sources.push(StylesheetSource::External(url));
                    }
                }
            }
        }
    }

    for child in &element.children {
        collect_stylesheet_sources_from_node(child, page_url, sources);
    }
}

fn html_attribute<'a>(element: &'a HtmlElement, name: &str) -> Option<&'a str> {
    element
        .attributes
        .iter()
        .find(|attribute| attribute.name == name)
        .map(|attribute| attribute.value.as_str())
}

fn element_info(node: &HtmlNode) -> Option<ElementInfo> {
    let HtmlNode::Element(element) = node else {
        return None;
    };

    let mut id = None;
    let mut classes = Vec::new();
    let mut inline_style = None;
    let mut href = None;
    let mut src = None;

    for attribute in &element.attributes {
        match attribute.name.as_str() {
            "id" => id = Some(attribute.value.clone()),

            "class" => {
                classes = attribute
                    .value
                    .split_whitespace()
                    .map(ToOwned::to_owned)
                    .collect();
            }

            "style" => inline_style = Some(attribute.value.clone()),

            "href" => href = Some(attribute.value.clone()),

            "src" => src = Some(attribute.value.clone()),

            _ => {}
        }
    }

    Some(ElementInfo {
        tag: element.name.clone(),
        id,
        classes,
        inline_style,
        href,
        src,
    })
}

fn extract_blocks(
    node: &HtmlNode,
    rules: &[CssRule],
    blocks: &mut Vec<Block>,
    current: &mut String,
    inherited_style: TextStyle,
    page_url: Option<&Url>,
    fetcher: Option<&Fetcher>,
    document: &DocumentState,
) {
    if let HtmlNode::Text(contents) = node {
        append_collapsed(current, contents);
        return;
    }

    let Some(element) = element_info(node) else {
        return;
    };

    let HtmlNode::Element(dom_element) = node else {
        unreachable!("element_info only returns element nodes");
    };

    if matches!(
        element.tag.as_str(),
        "script" | "style" | "template" | "head" | "title" | "meta" | "link"
    ) {
        return;
    }

    if element.tag == "br" {
        flush(blocks, current, inherited_style);
        return;
    }

    let style = compute_style(&element.css_element(), inherited_style, rules);

    if style.display == Display::None {
        return;
    }

    if element.tag == "img" {
        let content = match (fetcher, page_url, element.src.as_deref()) {
            (Some(fetcher), Some(page_url), Some(source)) => load_image(fetcher, page_url, source),
            _ => None,
        };

        if let Some(content) = content {
            blocks.push(Block {
                content,
                style,
                link: None,
            });
        }
    }

    if element.tag == "svg" {
        flush(blocks, current, inherited_style);

        let parsed_svg = match page_url {
            Some(page_url) => parse_svg_element_at(dom_element, page_url),
            None => parse_svg_element(dom_element),
        };

        if let Ok(svg) = parsed_svg {
            blocks.push(Block {
                content: BlockContent::Svg(svg),
                style,
                link: None,
            });
        }

        return;
    }

    if element.tag == "a" {
        flush(blocks, current, inherited_style);

        let mut text = String::new();

        let contents = element
            .id
            .as_deref()
            .and_then(|id| document.text_content_override(id))
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| text_content(node));

        append_collapsed(&mut text, &contents);

        let target = element
            .href
            .as_deref()
            .and_then(|href| page_url.and_then(|url| url.join(href.trim()).ok()))
            .filter(|url| matches!(url.scheme(), "http" | "https"))
            .map(|url| url.to_string());

        if !text.is_empty() {
            blocks.push(Block {
                content: BlockContent::Text(text),
                style,
                link: target,
            });
        }

        return;
    }

    let is_block = style.display == Display::Block;

    if is_block {
        flush(blocks, current, inherited_style);
    }

    if let Some(contents) = element
        .id
        .as_deref()
        .and_then(|id| document.text_content_override(id))
    {
        append_collapsed(current, contents);

        if is_block {
            flush(blocks, current, style);
        }

        return;
    }

    for child in &dom_element.children {
        extract_blocks(
            child, rules, blocks, current, style, page_url, fetcher, document,
        );
    }

    if is_block {
        flush(blocks, current, style);
    }
}

fn append_collapsed(target: &mut String, source: &str) {
    for word in source.split_whitespace() {
        if !target.is_empty() {
            target.push(' ');
        }

        target.push_str(word);
    }
}

fn flush(blocks: &mut Vec<Block>, current: &mut String, style: TextStyle) {
    let text = current.trim().to_owned();
    current.clear();

    if !text.is_empty() {
        blocks.push(Block {
            content: BlockContent::Text(text),
            style,
            link: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::{FontStyle, FontWeight};
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

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

        let link = page
            .blocks
            .iter()
            .find(|block| block.text().unwrap() == "about us")
            .unwrap();

        assert_eq!(
            link.link.as_deref(),
            Some("https://example.test/docs/about.html")
        );
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
    fn renders_external_svg_images_through_fetch_and_url_modules() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            stream.read(&mut request).unwrap();

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

        let page = parse_dom_with_fetcher(
            &format!("http://{address}/page.html"),
            &document,
            &[],
            Some(&fetcher),
        );

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
            let mut request = [0; 1024];
            stream.read(&mut request).unwrap();

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

        let page = parse_dom_with_fetcher(
            &format!("http://{address}/page.html"),
            &document,
            &[],
            Some(&fetcher),
        );

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

        let style = page.blocks[0].style;

        assert_eq!(page.blocks[0].text(), Some("FreeDOM"));
        assert_eq!(style.color, 0x00_12_34_56);
        assert_eq!(style.font_size, 20.0);
        assert_eq!(style.font_weight, FontWeight::Bold);
        assert_eq!(style.font_style, FontStyle::Italic);
    }
}
