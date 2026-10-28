use crate::{
    css::{
        CssRule, Display, Element as CssElement, FontFaceRule, FontStyle, FontWeight, TextStyle,
        build_author_rules, compute_style, parse_font_faces,
    },
    fetch::Fetcher,
    graphics::{
        font::normalize_font_container,
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

struct Stylesheet {
    source: String,
    base_url: Url,
}

struct LoadedFontFace {
    family: String,
    font_weight: FontWeight,
    font_style: FontStyle,
    data: Vec<u8>,
}

pub(crate) struct Page {
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) blocks: Vec<Block>,
    font_faces: Vec<LoadedFontFace>,
}

impl Page {
    pub(crate) fn font_data_for(
        &self,
        family: Option<&str>,
        font_weight: FontWeight,
        font_style: FontStyle,
    ) -> Option<&[u8]> {
        let family = family?;

        self.font_faces
            .iter()
            .find(|face| {
                face.family.eq_ignore_ascii_case(family)
                    && face.font_weight == font_weight
                    && face.font_style == font_style
            })
            .or_else(|| {
                self.font_faces
                    .iter()
                    .find(|face| face.family.eq_ignore_ascii_case(family))
            })
            .map(|face| face.data.as_slice())
    }
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

fn parse_dom(url: &str, document: &HtmlDocument, stylesheet_sources: &[String]) -> Page {
    let stylesheets: Vec<Stylesheet> = Url::parse(url)
        .map(|base_url| {
            stylesheet_sources
                .iter()
                .cloned()
                .map(|source| Stylesheet {
                    source,
                    base_url: base_url.clone(),
                })
                .collect()
        })
        .unwrap_or_default();

    parse_dom_with_fetcher(url, document, &stylesheets, None)
}

fn parse_dom_with_fetcher(
    url: &str,
    document: &HtmlDocument,
    stylesheets: &[Stylesheet],
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

    let stylesheets_texts: Vec<String> = stylesheets
        .iter()
        .map(|stylesheet| stylesheet.source.clone())
        .collect();

    let rules = build_author_rules(&stylesheets_texts);
    let font_faces = fetcher
        .map(|fetcher| load_author_fonts(fetcher, stylesheets))
        .unwrap_or_default();
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
            base_style.clone(),
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
        font_faces,
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

fn load_author_stylesheets(
    fetcher: &Fetcher,
    page_url: &Url,
    root: &HtmlDocument,
) -> Vec<Stylesheet> {
    let mut sources = Vec::new();
    collect_stylesheet_sources(root, page_url, &mut sources);

    sources
        .into_iter()
        .filter_map(|source| match source {
            StylesheetSource::Inline(source) => Some(Stylesheet {
                source,
                base_url: page_url.clone(),
            }),

            StylesheetSource::External(url) => match fetcher.get(&url) {
                Ok(response) if response.status < 400 => {
                    let source = response.text_lossy();

                    Some(Stylesheet {
                        source,
                        base_url: response.url,
                    })
                }

                Ok(response) => {
                    eprintln!(
                        "FreeDOM: stylesheet {url} returned HTTP status {}",
                        response.status
                    );
                    None
                }

                Err(error) => {
                    eprintln!("FreeDOM: failed to load stylesheet {url}: {error}");
                    None
                }
            },
        })
        .collect()
}

fn load_author_fonts(fetcher: &Fetcher, stylesheets: &[Stylesheet]) -> Vec<LoadedFontFace> {
    let mut loaded_fonts = Vec::new();

    for stylesheet in stylesheets {
        for face in parse_font_faces(std::slice::from_ref(&stylesheet.source)) {
            let Some(data) = load_font_face(fetcher, stylesheet, &face) else {
                continue;
            };

            loaded_fonts.push(LoadedFontFace {
                family: face.family,
                font_weight: face.font_weight,
                font_style: face.font_style,
                data,
            });
        }
    }

    loaded_fonts
}

fn load_font_face(
    fetcher: &Fetcher,
    stylesheet: &Stylesheet,
    face: &FontFaceRule,
) -> Option<Vec<u8>> {
    for source in &face.sources {
        let Ok(url) = stylesheet.base_url.join(source) else {
            continue;
        };

        if !Fetcher::supports(&url) {
            continue;
        }

        let response = match fetcher.get(&url) {
            Ok(response) if response.status < 400 => response,

            Ok(response) => {
                eprintln!(
                    "FreeDOM: font {url} returned HTTP status {}",
                    response.status
                );
                continue;
            }

            Err(error) => {
                eprintln!("FreeDOM: failed to load font {url}: {error}");
                continue;
            }
        };

        let data = match normalize_font_container(response.bytes().to_vec()) {
            Ok(data) => data,

            Err(error) => {
                eprintln!("FreeDOM: failed to decode font {url}: {error}");
                continue;
            }
        };

        // Validate the SFNT directory before accepting this source.
        if let Err(error) = crate::graphics::font::sfnt::SfntFont::parse(&data) {
            eprintln!("FreeDOM: invalid font {url}: {error}");
            continue;
        }

        return Some(data);
    }

    None
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

    let style = compute_style(&element.css_element(), inherited_style.clone(), rules);

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

        return;
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
        flush(blocks, current, inherited_style.clone());
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
            child,
            rules,
            blocks,
            current,
            style.clone(),
            page_url,
            fetcher,
            document,
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

        let style = page.blocks[0].style.clone();

        assert_eq!(page.blocks[0].text(), Some("FreeDOM"));
        assert_eq!(style.color, 0x00_12_34_56);
        assert_eq!(style.font_size, 20.0);
        assert_eq!(style.font_weight, FontWeight::Bold);
        assert_eq!(style.font_style, FontStyle::Italic);
    }

    #[test]
    fn selects_a_matching_loaded_font_face() {
        let page = Page {
            url: "https://example.test/".to_owned(),
            title: String::new(),
            blocks: Vec::new(),
            font_faces: vec![LoadedFontFace {
                family: "FreeDOM Sans".to_owned(),
                font_weight: FontWeight::Normal,
                font_style: FontStyle::Normal,
                data: vec![1, 2, 3],
            }],
        };

        assert_eq!(
            page.font_data_for(Some("freedom sans"), FontWeight::Normal, FontStyle::Normal,),
            Some([1, 2, 3].as_slice())
        );
    }

    #[test]
    fn loads_a_woff_font_face_through_css_and_fetch() {
        let table = [0_u8, 1, 2, 3];
        let woff = woff_with_name_table(&table);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];

            stream.read(&mut request).unwrap();

            assert!(String::from_utf8_lossy(&request).contains("GET /fonts/test.woff "));

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

        let page = parse_dom_with_fetcher(&page_url, &document, &stylesheets, Some(&fetcher));

        server.join().unwrap();

        assert_eq!(page.blocks[0].style.font_family.as_deref(), Some("Test"));

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

                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();

                let mut request = Vec::new();
                let mut chunk = [0_u8; 1024];

                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let count = stream.read(&mut chunk).unwrap();

                    assert!(count > 0, "unexpected end of request");

                    request.extend_from_slice(&chunk[..count]);

                    assert!(request.len() < 16_384, "request headers are too long");
                }

                let request = String::from_utf8(request).unwrap();

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

        assert_eq!(block.style.font_family.as_deref(), Some("Nko, Test"));

        let font_data = page
            .font_data_for(
                block.style.font_family.as_deref(),
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
            &mut pixels,
            width,
            height,
            &page,
            &layout,
            "",
            "",
            None,
            &mut block_cache,
            &mut glyph_cache,
            0,
            false,
        );

        // A cached repaint must produce the same image.
        let mut cached_pixels = vec![0; width * height];

        paint(
            &mut cached_pixels,
            width,
            height,
            &page,
            &layout,
            "",
            "",
            None,
            &mut block_cache,
            &mut glyph_cache,
            0,
            false,
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
            &mut fallback_pixels,
            width,
            height,
            &page,
            &fallback_layout,
            "",
            "",
            None,
            &mut BlockRasterCache::new(),
            &mut GlyphRasterCache::new(),
            0,
            false,
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
    fn inline_style_keeps_semicolon_inside_font_family() {
        let page = parse_html(
            "https://example.test",
            r#"<p style="font-family: 'Nko; Test'; color: green">Hello</p>"#,
        );

        assert_eq!(
            page.blocks[0].style.font_family.as_deref(),
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
            stylesheet.blocks[0].style.font_family.as_deref(),
            Some("A\u{FFFD}B")
        );

        assert_eq!(
            inline.blocks[0].style.font_family.as_deref(),
            Some("A\u{FFFD}B")
        );
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
            &mut pixels,
            width,
            height,
            &page,
            &layout,
            "",
            "",
            None,
            &mut BlockRasterCache::new(),
            &mut GlyphRasterCache::new(),
            0,
            false,
        );

        let content_start = CONTENT_TOP as usize * width;

        assert!(
            pixels[content_start..]
                .iter()
                .all(|&pixel| pixel == 0x00_F7_F7_F7)
        );
    }
}
