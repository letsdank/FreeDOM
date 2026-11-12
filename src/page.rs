use crate::{
    css::{
        CssRule, Display, Element as CssElement, FontFaceRule, FontStyle, FontWeight, TextStyle,
        build_author_rules, compute_style_with_ancestors, parse_font_faces,
    },
    fetch::Fetcher,
    graphics::{
        font::normalize_font_container,
        image::{decode_jpeg, decode_png_for_display},
        raster::Surface as RasterSurface,
        svg::{SvgDocument, parse_svg_at, parse_svg_element, parse_svg_element_at},
    },
    html::{Document as HtmlDocument, Element as HtmlElement, Node as HtmlNode},
    javascript::{DocumentScriptSession, DocumentState},
    styled_tree::{SourcePath, StyledTree},
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
    pub(crate) sources: Vec<SourcePath>,
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
    pub(crate) styled_tree: StyledTree,
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

    pub(crate) fn describe_block_sources(&self, index: usize) -> Option<String> {
        use std::fmt::Write;

        let block = self.blocks.get(index)?;

        let kind = match &block.content {
            BlockContent::Text(_) => "text",
            BlockContent::Svg(_) => "svg",
            BlockContent::Bitmap(_) => "image",
        };

        let mut output = format!("FreeDOM block #{index} ({kind}) - source contributors:\n");

        if block.sources.is_empty() {
            output.push_str("  no source recorded\n");
        }

        for path in block.sources.iter().take(32) {
            match self.styled_tree.source_node(path) {
                Some(HtmlNode::Element(element)) => {
                    let id = html_attribute(element, "id");

                    let _ = writeln!(
                        output,
                        "  <{}> id={id:?} path={:?}",
                        element.name,
                        path.indices()
                    );
                }

                Some(HtmlNode::Text(_)) => {
                    let _ = writeln!(output, "  text node path={:?}", path.indices());
                }

                _ => {
                    let _ = writeln!(output, "  unavailable path={:?}", path.indices());
                }
            }
        }

        if block.sources.len() > 32 {
            output.push_str("  ... output limit reached\n");
        }

        Some(output)
    }
}

#[derive(Clone)]
pub(crate) struct ElementInfo {
    pub(crate) tag: String,
    pub(crate) id: Option<String>,
    classes: Vec<String>,
    inline_style: Option<String>,
    href: Option<String>,
    src: Option<String>,
}

impl ElementInfo {
    pub(crate) fn css_element(&self) -> CssElement<'_> {
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

#[cfg(test)]
pub(crate) fn parse_xhtml_for_test(
    url: &str,
    source: &str,
) -> Result<Page, crate::xml::XhtmlError> {
    let document = crate::xml::parse_xhtml(source)?;
    let stylesheets = collect_inline_stylesheets(&document);

    Ok(parse_dom(url, &document, &stylesheets))
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
    let styled_tree = StyledTree::build(document, &rules, &document_state);
    let font_faces = fetcher
        .map(|fetcher| load_author_fonts(fetcher, stylesheets))
        .unwrap_or_default();
    let base_style = TextStyle::initial();

    let mut blocks = Vec::new();
    let mut current_text = String::new();
    let mut trace = ExtractionTrace::default();
    let page_url = Url::parse(url).ok();

    for (index, child) in document.children.iter().enumerate() {
        trace.path.push(index);

        extract_blocks(
            child,
            &rules,
            &mut blocks,
            &mut current_text,
            base_style.clone(),
            page_url.as_ref(),
            fetcher,
            &document_state,
            &mut trace,
        );

        trace.path.pop();
    }

    flush(&mut blocks, &mut current_text, base_style, &mut trace);

    Page {
        url: url.to_owned(),
        title,
        blocks,
        font_faces,
        styled_tree,
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
    let mut session = DocumentScriptSession::new(document);

    for child in &root.children {
        run_inline_scripts_in_node(child, document, &mut session);
    }
}

fn run_inline_scripts_in_node(
    node: &HtmlNode,
    document: &mut DocumentState,
    session: &mut DocumentScriptSession,
) {
    let HtmlNode::Element(element) = node else {
        return;
    };

    if element.name == "script" {
        let is_inline = html_attribute(element, "src").is_none();

        if is_inline && is_classic_javascript(element) {
            let source = text_content(node);

            if !source.trim().is_empty() {
                if let Err(error) = session.evaluate(&source, document) {
                    eprintln!("FreeDOM: inline script error: {error}");
                }
            }
        }

        return;
    }

    for child in &element.children {
        run_inline_scripts_in_node(child, document, session);
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

        Some("image/png") => decode_png_for_display(response.bytes())
            .ok()
            .map(BlockContent::Bitmap),

        Some("image/jpeg") => decode_jpeg(response.bytes()).ok().map(BlockContent::Bitmap),

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

pub(crate) fn element_info(node: &HtmlNode) -> Option<ElementInfo> {
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

#[derive(Default)]
struct ExtractionTrace {
    path: Vec<usize>,
    pending: Vec<SourcePath>,
    ancestors: Vec<ElementInfo>,
}

impl ExtractionTrace {
    fn append_text(&mut self, current: &mut String, contents: &str, is_text_node: bool) {
        let before = current.len();

        append_collapsed(current, contents);

        if current.len() == before {
            return;
        }

        // Ordinary text is attributed to its containing element.
        // Top-level text has no containing element, so keep its own path.
        let owner = if is_text_node && self.path.len() > 1 {
            &self.path[..self.path.len() - 1]
        } else {
            &self.path[..]
        };

        let source = SourcePath::from_indices(owner);

        if self.pending.last() != Some(&source) {
            self.pending.push(source);
        }
    }

    fn current_source(&self) -> SourcePath {
        SourcePath::from_indices(&self.path)
    }
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
    trace: &mut ExtractionTrace,
) {
    if let HtmlNode::Text(contents) = node {
        trace.append_text(current, contents, true);
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
        flush(blocks, current, inherited_style, trace);
        return;
    }

    let ancestors: Vec<_> = trace
        .ancestors
        .iter()
        .map(ElementInfo::css_element)
        .collect();

    let style = compute_style_with_ancestors(
        &element.css_element(),
        inherited_style.clone(),
        rules,
        &ancestors,
    );

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
                sources: vec![trace.current_source()],
            });
        }

        return;
    }

    if element.tag == "svg" {
        flush(blocks, current, inherited_style, trace);

        let parsed_svg = match page_url {
            Some(page_url) => parse_svg_element_at(dom_element, page_url),
            None => parse_svg_element(dom_element),
        };

        if let Ok(svg) = parsed_svg {
            blocks.push(Block {
                content: BlockContent::Svg(svg),
                style,
                link: None,
                sources: vec![trace.current_source()],
            });
        }

        return;
    }

    if element.tag == "a" {
        flush(blocks, current, inherited_style, trace);

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
                sources: vec![trace.current_source()],
            });
        }

        return;
    }

    let is_block = style.display == Display::Block;

    if is_block {
        flush(blocks, current, inherited_style.clone(), trace);
    }

    if let Some(contents) = element
        .id
        .as_deref()
        .and_then(|id| document.text_content_override(id))
    {
        trace.append_text(current, contents, false);

        if is_block {
            flush(blocks, current, style, trace);
        }

        return;
    }

    trace.ancestors.push(element);

    for (index, child) in dom_element.children.iter().enumerate() {
        trace.path.push(index);

        extract_blocks(
            child,
            rules,
            blocks,
            current,
            style.clone(),
            page_url,
            fetcher,
            document,
            trace,
        );

        trace.path.pop();
    }

    trace.ancestors.pop();

    if is_block {
        flush(blocks, current, style, trace);
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

fn flush(
    blocks: &mut Vec<Block>,
    current: &mut String,
    style: TextStyle,
    trace: &mut ExtractionTrace,
) {
    let text = current.trim().to_owned();
    current.clear();

    let sources = std::mem::take(&mut trace.pending);

    if !text.is_empty() {
        blocks.push(Block {
            content: BlockContent::Text(text),
            style,
            link: None,
            sources,
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
            styled_tree: StyledTree::default(),
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
    fn layout_inspector_uses_real_block_sizes_without_polluting_cache() {
        use crate::{
            graphics::raster::{Color, Surface},
            layout_debug::paint_layout_overlay,
            page_layout::{LayoutRect, PageLayout},
            render::{BlockRasterCache, GlyphRasterCache, paint},
        };

        let page = Page {
            url: "https://example.test/".into(),
            title: String::new(),

            blocks: vec![
                Block {
                    content: BlockContent::Text("Hello".into()),
                    style: TextStyle::initial(),
                    link: None,
                    sources: Vec::new(),
                },
                Block {
                    content: BlockContent::Bitmap(Surface::new(30, 12, Color::BLACK)),
                    style: TextStyle::initial(),
                    link: None,
                    sources: Vec::new(),
                },
            ],

            font_faces: vec![],
            styled_tree: StyledTree::default(),
        };

        let (width, height) = (100, 130);
        let layout = PageLayout::build(&page, width, None);

        assert_eq!(
            layout.block_rects().collect::<Vec<_>>(),
            vec![
                LayoutRect {
                    x: 16,
                    y: 0,
                    width: 68,
                    height: 10,
                },
                LayoutRect {
                    x: 16,
                    y: 10,
                    width: 30,
                    height: 12
                }
            ]
        );

        let mut cache = BlockRasterCache::new();
        let mut glyphs = GlyphRasterCache::new();

        let mut first = vec![0; width * height];

        paint(
            &mut first,
            width,
            height,
            &page,
            &layout,
            "",
            "",
            None,
            &mut cache,
            &mut glyphs,
            0,
            false,
        );

        let mut overlay = first.clone();

        paint_layout_overlay(&mut overlay, width, height, &layout, 0);

        assert_ne!(overlay, first);

        let mut again = vec![0; width * height];

        paint(
            &mut again,
            width,
            height,
            &page,
            &layout,
            "",
            "",
            None,
            &mut cache,
            &mut glyphs,
            0,
            false,
        );

        assert_eq!(first, again);
    }

    #[test]
    fn page_styled_snapshot_tracks_nesting_and_existing_text_override() {
        let page = parse_html(
            "https://example.test",
            r#"
                <div id="outer">
                    <p id="welcome">Before</p>
                </div>

                <script>
                    const heading = document.getElementById("welcome");
                    heading.textContent = "After";
                </script>
            "#,
        );

        assert_eq!(page.blocks[0].text(), Some("After"));

        let debug = page.styled_tree.format_debug();

        assert!(debug.contains("<div>"));
        assert!(debug.contains("<p>"));
        assert!(debug.contains("text \"After\" source=script override"));
        assert!(!debug.contains("text \"Before\""));
    }

    #[test]
    fn connects_rendered_blocks_to_source_contributors() {
        let page = parse_html(
            "https://example.test/",
            r#"<p id="outer">One <span id="inner">two</span> three</p><a href="/x">Link</a><svg width="4" height="3"><rect width="4" height="3"/></svg>"#,
        );

        assert_eq!(page.blocks.len(), 3);
        assert_eq!(page.blocks[0].text(), Some("One two three"));

        let paths: Vec<_> = page.blocks[0]
            .sources
            .iter()
            .map(|path| path.indices())
            .collect();

        assert_eq!(paths, [&[0][..], &[0, 1][..], &[0][..]]);
        assert_eq!(page.blocks[1].sources[0].indices(), &[1]);
        assert_eq!(page.blocks[2].sources[0].indices(), &[2]);

        let description = page.describe_block_sources(0).unwrap();

        assert!(description.contains("<p> id=Some(\"outer\")"));
        assert!(description.contains("<span> id=Some(\"inner\")"));
        assert!(page.describe_block_sources(2).unwrap().contains("<svg>"));
    }

    #[test]
    fn script_text_override_and_top_level_text_keep_accurate_sources() {
        let page = parse_html(
            "https://example.test/",
            r#"plain<p id="welcome">Before</p><script>document.getElementById("welcome").textContent = "After";</script>"#,
        );

        assert_eq!(page.blocks[0].text(), Some("plain"));
        assert_eq!(page.blocks[0].sources[0].indices(), &[0]);

        assert!(
            page.describe_block_sources(0)
                .unwrap()
                .contains("text node")
        );

        assert_eq!(page.blocks[1].text(), Some("After"));
        assert_eq!(page.blocks[1].sources[0].indices(), &[1]);
    }

    #[test]
    fn box_tree_inspection_keeps_existing_flat_page_rendering() {
        let page = parse_html(
            "https://example.test/",
            r#"<div>before <span>inside</span><div>child</div> after</div>"#,
        );

        let before: Vec<_> = page
            .blocks
            .iter()
            .filter_map(|block| block.text().map(str::to_owned))
            .collect();

        let tree = crate::box_tree::BoxTree::build(&page.styled_tree).unwrap();
        let dump = tree.format_debug();

        assert!(dump.contains("anonymous block: inline run"));
        assert!(dump.contains("inline <span>"));

        let after: Vec<_> = page
            .blocks
            .iter()
            .filter_map(|block| block.text().map(str::to_owned))
            .collect();

        assert_eq!(after, before);
    }

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
        for (previous, parent, child, expected_y) in
            [(6, 4, 8, 18), (6, 8, -3, 15), (-5, -2, -3, 5)]
        {
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
    fn preview_positions_empty_fixed_height_blocks_with_collapsed_margins() {
        let page = parse_html(
            "https://example.test/",
            concat!(
                "<div style='height: 20px; margin-bottom: 2in'></div>",
                "<div style='height: 20px; margin-top: -2in'></div>"
            ),
        );

        let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

        let rects: Vec<_> = preview
            .boxes
            .iter()
            .map(|item| (item.rect.y, item.rect.height))
            .collect();

        assert_eq!(rects, vec![(0, 20), (20, 20)]);
    }

    #[test]
    fn preview_fixed_height_prevents_last_child_bottom_margin_escaping() {
        let page = parse_html(
            "https://example.test/",
            concat!(
                "<div style='height: 20px'>",
                "<div style='height: 5px; margin-bottom: 8px'></div>",
                "</div>",
                "<div style='height: 10px; margin-top: 3px'></div>"
            ),
        );

        let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

        let rects: Vec<_> = preview
            .boxes
            .iter()
            .map(|item| (item.rect.y, item.rect.height))
            .collect();

        assert_eq!(rects, vec![(0, 20), (0, 5), (23, 10)]);
    }

    #[test]
    fn preview_fixed_height_may_have_overflowing_content() {
        let page = parse_html(
            "https://example.test/",
            concat!(
                "<div style='height: 5px'>",
                "<div style='height: 20px'></div>",
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

        assert_eq!(rects, vec![(0, 5), (0, 20), (5, 10)]);
    }

    #[test]
    fn preview_zero_height_empty_block_remains_collapsible() {
        let page = parse_html(
            "https://example.test/",
            concat!(
                "<div style='height: 10px; margin-bottom: 8px'></div>",
                "<div style='height: 0; margin-top: 12px; ",
                "margin-bottom: -3px'></div>",
                "<div style='height: 10px; margin-top: 6px'></div>",
            ),
        );

        let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

        let rects: Vec<_> = preview
            .boxes
            .iter()
            .map(|item| (item.rect.y, item.rect.height))
            .collect();

        assert_eq!(rects, vec![(0, 10), (22, 0), (19, 10)]);
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

        crate::layout_debug::paint_preview_backgrounds(&mut pixels, 100, 140, &preview, 0);

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
            crate::css::BackgroundColor::Opaque(0x00_00_00_FF)
        );

        let mut pixels = vec![0; 100 * 140];

        crate::layout_debug::paint_preview_backgrounds(&mut pixels, 100, 140, &preview, 0);

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

        let target_layout =
            crate::layout_preview::PreviewLayout::build(&target, 100, None).unwrap();

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

        crate::layout_debug::paint_preview_backgrounds(&mut actual, 100, 140, &target_layout, 0);

        crate::layout_debug::paint_preview_backgrounds(
            &mut expected,
            100,
            140,
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

        let target_layout =
            crate::layout_preview::PreviewLayout::build(&target, width, None).unwrap();

        let reference_layout =
            crate::layout_preview::PreviewLayout::build(&reference, width, None).unwrap();

        let mut actual = vec![0; width * height];
        let mut expected = vec![0; width * height];

        crate::layout_debug::paint_preview_backgrounds(
            &mut actual,
            width,
            height,
            &target_layout,
            0,
        );

        crate::layout_debug::paint_preview_backgrounds(
            &mut expected,
            width,
            height,
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
    fn preview_scene_paints_text_and_clips_it_below_toolbar() {
        let page = parse_html(
            "https://example.test/",
            "<div style='width:32px;height:20px;background:blue;color:white'>A</div>",
        );

        let preview = crate::layout_preview::PreviewLayout::build(&page, 100, None).unwrap();

        let mut pixels = vec![0x00_12_34_56; 100 * 120];
        let mut cache = crate::render::GlyphRasterCache::new();

        crate::render::paint_preview_scene(
            &mut pixels,
            100,
            120,
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
            &mut pixels,
            100,
            110,
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

        let target_layout =
            crate::layout_preview::PreviewLayout::build(&target, width, None).unwrap();

        let reference_layout =
            crate::layout_preview::PreviewLayout::build(&reference, width, None).unwrap();

        let mut actual = vec![0; width * height];
        let mut expected = vec![0; width * height];
        let mut cache = crate::render::GlyphRasterCache::new();

        crate::render::paint_preview_scene(
            &mut actual,
            width,
            height,
            &target,
            &target_layout,
            None,
            0,
            &mut cache,
        );

        crate::render::paint_preview_scene(
            &mut expected,
            width,
            height,
            &reference,
            &reference_layout,
            None,
            0,
            &mut cache,
        );

        assert!(
            actual[crate::page_layout::CONTENT_TOP as usize * width..]
                .iter()
                .any(|&pixel| pixel == 0x00_00_00_00),
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
            &mut pixels,
            132,
            110,
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
            &mut pixels,
            100,
            200,
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
            &mut pixels,
            100,
            110,
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
                &mut actual,
                width,
                height,
                &split,
                &split_layout,
                None,
                scroll,
                &mut GlyphRasterCache::new(),
            );

            paint_preview_scene(
                &mut expected,
                width,
                height,
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
    fn preview_paints_saved_run_colors() {
        use crate::layout_preview::{PreviewCommand, PreviewLayout};
        use crate::render::{GlyphRasterCache, paint_preview_scene};

        let page = parse_html("https://example.test/", "<div>A<span>B</span> C</div>");

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

        // Mixed styles are not accepted by layout yet. Change the display list
        // directly to verify that the painter reads each saved color.
        runs[0].style.color = 0x00_FF_00_00;
        runs[1].style.color = 0x00_00_00_FF;

        let mut pixels = vec![0; 120 * 120];

        paint_preview_scene(
            &mut pixels,
            120,
            120,
            &page,
            &layout,
            None,
            0,
            &mut GlyphRasterCache::new(),
        );

        assert!(pixels[90 * 120..].contains(&0x00_FF_00_00));
        assert!(pixels[90 * 120..].contains(&0x00_00_00_FF));
    }
}
