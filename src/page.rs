use crate::{
    fetch::Fetcher,
    graphics::{
        canvas::{Canvas2D, parse_canvas_dimension},
        font::normalize_font_container,
        image::{decode_jpeg, decode_png_for_display},
        raster::Surface as RasterSurface,
        svg::{SvgDocument, parse_svg_at, parse_svg_element, parse_svg_element_at},
    },
    styled_tree::{SourcePath, StyledTree},
};
use anyhow::Result;
use freedom_css::{
    CssRule, Display, Element as CssElement, FontFaceRule, FontStyle, FontWeight, TextStyle,
    build_author_rules, compute_style_with_ancestors, parse_font_faces,
};
use freedom_html::{Document as HtmlDocument, Element as HtmlElement, Node as HtmlNode};
use freedom_js::{DocumentScriptSession, DocumentState, MAX_PAGE_CANVAS_PIXELS, MAX_PAGE_CANVASES};
use freedom_url::Url;

pub(crate) enum BlockContent {
    Text(TextBlock),
    Svg(SvgDocument),
    Bitmap(RasterSurface),
    Canvas(CanvasBlock),
    PendingImage(ImageRequest),
}

#[derive(Clone, Debug)]
pub(crate) struct ImageRequest {
    pub(crate) id: usize,
    pub(crate) url: Url,
}

pub(crate) struct CanvasBlock {
    pub(crate) node_id: u64,
    pub(crate) canvas: Canvas2D,
}

pub(crate) struct TextBlock {
    pub(crate) text: String,
    pub(crate) runs: Vec<TextRun>,
}

pub(crate) struct TextRun {
    pub(crate) range: std::ops::Range<usize>,
    pub(crate) style: TextStyle,
    pub(crate) link: Option<String>,
}

pub(crate) struct Block {
    pub(crate) content: BlockContent,
    pub(crate) style: TextStyle,
    pub(crate) link: Option<String>,
    pub(crate) sources: Vec<SourcePath>,
}

#[cfg(test)]
impl Block {
    fn text(&self) -> Option<&str> {
        match &self.content {
            BlockContent::Text(text) => Some(&text.text),
            BlockContent::Svg(_)
            | BlockContent::Bitmap(_)
            | BlockContent::Canvas(_)
            | BlockContent::PendingImage(_) => None,
        }
    }

    fn svg(&self) -> Option<&SvgDocument> {
        match &self.content {
            BlockContent::Svg(svg) => Some(svg),
            BlockContent::Text(_)
            | BlockContent::Bitmap(_)
            | BlockContent::Canvas(_)
            | BlockContent::PendingImage(_) => None,
        }
    }

    fn bitmap(&self) -> Option<&RasterSurface> {
        match &self.content {
            BlockContent::Bitmap(bitmap) => Some(bitmap),
            BlockContent::Text(_)
            | BlockContent::Svg(_)
            | BlockContent::Canvas(_)
            | BlockContent::PendingImage(_) => None,
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

pub(crate) struct ResolvedFont<'a> {
    pub(crate) font: crate::graphics::font::TrueTypeFont<'a>,
    pub(crate) synthesis: (FontWeight, FontStyle),
}

impl crate::graphics::font::layout::PositionedFont for ResolvedFont<'_> {
    fn font(&self) -> &crate::graphics::font::TrueTypeFont<'_> {
        &self.font
    }
}

pub(crate) struct Page {
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) blocks: Vec<Block>,
    font_faces: Vec<LoadedFontFace>,
    pub(crate) styled_tree: StyledTree,
    pub(crate) document: DocumentState,
}

impl Page {
    #[cfg(test)]
    pub(crate) fn font_data_for(
        &self,
        family: Option<&str>,
        font_weight: FontWeight,
        font_style: FontStyle,
    ) -> Option<&[u8]> {
        self.font_face_for(family, font_weight, font_style)
            .map(|face| face.data.as_slice())
    }

    fn font_face_for(
        &self,
        family: Option<&str>,
        font_weight: FontWeight,
        font_style: FontStyle,
    ) -> Option<&LoadedFontFace> {
        let family = family?;

        self.font_faces
            .iter()
            .filter(|face| face.family.eq_ignore_ascii_case(family))
            // Partial CSS Fonts matching: style precedes weight.
            // https://www.w3.org/TR/css-fonts-4/#font-matching-algorithm
            .min_by_key(|face| {
                (
                    face.font_style != font_style,
                    face.font_weight != font_weight,
                )
            })
    }

    fn font_faces_for_style<'a, 'b>(
        &'a self,
        style: &'b TextStyle,
    ) -> impl Iterator<Item = &'a LoadedFontFace> + 'b
    where
        'a: 'b,
    {
        style.font_families.iter().filter_map(move |family| {
            self.font_face_for(Some(family), style.font_weight, style.font_style)
        })
    }

    #[cfg(test)]
    pub(crate) fn font_data_for_style(&self, style: &TextStyle) -> Option<&[u8]> {
        self.font_data_for_families(style).next()
    }

    #[cfg(test)]
    pub(crate) fn font_data_for_families<'a, 'b>(
        &'a self,
        style: &'b TextStyle,
    ) -> impl Iterator<Item = &'a [u8]> + 'b
    where
        'a: 'b,
    {
        self.font_faces_for_style(style)
            .map(|face| face.data.as_slice())
    }

    pub(crate) fn describe_block_sources(&self, index: usize) -> Option<String> {
        use std::fmt::Write;

        let block = self.blocks.get(index)?;

        let kind = match &block.content {
            BlockContent::Text(_) => "text",
            BlockContent::Svg(_) => "svg",
            BlockContent::Bitmap(_) => "image",
            BlockContent::Canvas(_) => "canvas",
            BlockContent::PendingImage(_) => "pending image",
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

        if let BlockContent::Text(text) = &block.content {
            for run in text.runs.iter().take(32) {
                let excerpt: String = text
                    .text
                    .get(run.range.clone())
                    .unwrap_or("<invalid range>")
                    .chars()
                    .take(80)
                    .collect();

                let _ = writeln!(
                    output,
                    "  text {:?} bytes={:?} weight={:?} color=#{:06x} link={:?}",
                    excerpt,
                    run.range,
                    run.style.font_weight,
                    run.style.color & 0x00ff_ffff,
                    run.link
                );
            }

            if text.runs.len() > 32 {
                output.push_str("  ... text run limit reached\n");
            }
        }

        if let BlockContent::Canvas(canvas) = &block.content {
            let connected = self.document.is_element_connected(canvas.node_id);

            let _ = writeln!(
                output,
                "  DOM node_id={} connected={connected}",
                canvas.node_id
            );
        }

        Some(output)
    }

    pub(crate) fn vector_fonts_for_style<'a>(&'a self, style: &TextStyle) -> Vec<ResolvedFont<'a>> {
        self.font_faces_for_style(style)
            .filter_map(|face| {
                let font = crate::graphics::font::TrueTypeFont::parse(&face.data).ok()?;

                let weight = if style.font_weight == FontWeight::Bold
                    && face.font_weight != FontWeight::Bold
                {
                    FontWeight::Bold
                } else {
                    FontWeight::Normal
                };

                let font_style = if style.font_style == FontStyle::Italic
                    && face.font_style != FontStyle::Italic
                {
                    FontStyle::Italic
                } else {
                    FontStyle::Normal
                };

                Some(ResolvedFont {
                    font,
                    synthesis: (weight, font_style),
                })
            })
            .collect()
    }

    pub(crate) fn image_requests(&self) -> Vec<ImageRequest> {
        self.blocks
            .iter()
            .filter_map(|block| match &block.content {
                BlockContent::PendingImage(request) => Some(request.clone()),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn apply_image_result(&mut self, id: usize, result: Option<BlockContent>) -> bool {
        if result.as_ref().is_some_and(|content| {
            !matches!(content, BlockContent::Svg(_) | BlockContent::Bitmap(_))
        }) {
            return false;
        }

        let Some(index) = self.blocks.iter().position(
            |block| matches!(&block.content,BlockContent::PendingImage(request) if request.id==id),
        ) else {
            return false;
        };

        if let Some(content) = result {
            self.blocks[index].content = content;
        } else {
            self.blocks.remove(index);
        }

        true
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

/// Main document, author stylesheets and fonts, loaded before script execution.
struct PreparedPage {
    final_url: Url,
    document: HtmlDocument,
    stylesheets: Vec<Stylesheet>,
    font_faces: Vec<LoadedFontFace>,
}

pub(crate) fn load_page(input: &str) -> Result<Page> {
    let input = if input.contains("://") {
        input.to_owned()
    } else {
        format!("https://{input}")
    };

    let url = Url::parse(&input)?;
    let fetcher = Fetcher::new()?;

    let prepared = prepare_page(&fetcher, &url)?;
    Ok(build_prepared_page(prepared))
}

fn prepare_page(fetcher: &Fetcher, url: &Url) -> Result<PreparedPage> {
    let response = fetcher.get(url)?;

    let html = response.text_lossy();
    let final_url = response.url;

    let document = HtmlDocument::parse(&html);
    let stylesheets = load_author_stylesheets(fetcher, &final_url, &document);
    let font_faces = load_author_fonts(fetcher, &stylesheets);

    Ok(PreparedPage {
        final_url,
        document,
        stylesheets,
        font_faces,
    })
}

fn build_prepared_page(prepared: PreparedPage) -> Page {
    let PreparedPage {
        final_url,
        document,
        stylesheets,
        font_faces,
    } = prepared;

    parse_dom_with_resources(&final_url.to_string(), &document, &stylesheets, font_faces)
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

#[cfg(test)]
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

    parse_dom_with_resources(url, document, &stylesheets, Vec::new())
}

fn parse_dom_with_resources(
    url: &str,
    document: &HtmlDocument,
    stylesheets: &[Stylesheet],
    font_faces: Vec<LoadedFontFace>,
) -> Page {
    let mut document_state = DocumentState::with_title(find_title(document).unwrap_or_default());

    register_document_elements(document, &mut document_state);
    let canvas_surfaces = run_inline_scripts(document, &mut document_state);

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
    let base_style = TextStyle::initial();

    let mut blocks = Vec::new();
    let mut current_text = String::new();

    let mut trace = ExtractionTrace {
        canvas_pixels: canvas_surfaces
            .values()
            .map(|canvas| canvas.surface().width() * canvas.surface().height())
            .sum(),
        canvas_count: canvas_surfaces.len(),
        canvas_surfaces,
        ..ExtractionTrace::default()
    };

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
        document: document_state,
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
    let mut path = Vec::new();

    for (index, child) in root.children.iter().enumerate() {
        path.push(index);
        register_document_elements_in_node(child, None, document, &mut path);
        path.pop();
    }
}

fn register_document_elements_in_node(
    node: &HtmlNode,
    parent_node_id: Option<u64>,
    document: &mut DocumentState,
    path: &mut Vec<usize>,
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

    if element.name == "canvas" {
        document.register_parsed_canvas(path, node_id);
    }

    for attribute in &element.attributes {
        document.register_element_attribute(node_id, &attribute.name, attribute.value.clone());
    }

    for (index, child) in element.children.iter().enumerate() {
        path.push(index);
        register_document_elements_in_node(child, Some(node_id), document, path);
        path.pop();
    }
}

fn run_inline_scripts(
    root: &HtmlDocument,
    document: &mut DocumentState,
) -> std::collections::HashMap<u64, Canvas2D> {
    let mut session = DocumentScriptSession::new(document);

    for child in &root.children {
        run_inline_scripts_in_node(child, document, &mut session);
    }

    session.into_canvas_surfaces()
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

            if !source.trim().is_empty()
                && let Err(error) = session.evaluate(&source, document)
            {
                eprintln!("FreeDOM: inline script error: {error}");
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

pub(crate) fn load_image(fetcher: &Fetcher, resource_url: &Url) -> Option<BlockContent> {
    let response = fetcher.get(resource_url).ok()?;

    if response.status >= 400 {
        return None;
    }

    let mime_type = response
        .content_type
        .as_ref()
        .map(|content_type| content_type.mime_type.as_str());

    decode_image_resource(response.bytes(), mime_type, &response.url)
}

fn decode_image_resource(
    bytes: &[u8],
    mime_type: Option<&str>,
    base_url: &Url,
) -> Option<BlockContent> {
    match mime_type {
        Some("image/svg+xml") => {
            let source = String::from_utf8_lossy(bytes);

            parse_svg_at(&source, base_url).ok().map(BlockContent::Svg)
        }

        Some("image/png") => decode_png_for_display(bytes).ok().map(BlockContent::Bitmap),

        Some("image/jpeg") => decode_jpeg(bytes).ok().map(BlockContent::Bitmap),

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

        if is_stylesheet
            && let Some(href) = href
            && let Ok(url) = page_url.join(href.trim())
            && matches!(url.scheme(), "http" | "https")
        {
            sources.push(StylesheetSource::External(url));
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
    pending_space: bool,
    text_runs: Vec<TextRun>,
    canvas_pixels: usize,
    canvas_count: usize,
    canvas_surfaces: std::collections::HashMap<u64, Canvas2D>,
    next_image_id: usize,
}

impl ExtractionTrace {
    fn append_text(
        &mut self,
        current: &mut String,
        contents: &str,
        is_text_node: bool,
        style: &TextStyle,
    ) {
        let before = current.len();

        append_collapsed(current, contents, &mut self.pending_space);

        if current.len() == before {
            return;
        }

        self.text_runs.push(TextRun {
            range: before..current.len(),
            style: style.clone(),
            link: None,
        });

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

#[expect(
    clippy::too_many_arguments,
    reason = "recursive extraction state is passed explicitly"
)]
fn extract_blocks(
    node: &HtmlNode,
    rules: &[CssRule],
    blocks: &mut Vec<Block>,
    current: &mut String,
    inherited_style: TextStyle,
    page_url: Option<&Url>,
    document: &DocumentState,
    trace: &mut ExtractionTrace,
) {
    if let HtmlNode::Text(contents) = node {
        trace.append_text(current, contents, true, &inherited_style);
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

    if element.tag == "canvas" {
        flush(blocks, current, inherited_style, trace);

        let Some(node_id) = document.connected_parsed_canvas(&trace.path) else {
            return;
        };

        let width = document
            .element_attribute(node_id, "width")
            .and_then(parse_canvas_dimension)
            .unwrap_or(300);

        let height = document
            .element_attribute(node_id, "height")
            .and_then(parse_canvas_dimension)
            .unwrap_or(150);

        if let Some(canvas) = trace.canvas_surfaces.remove(&node_id) {
            blocks.push(Block {
                content: BlockContent::Canvas(CanvasBlock { node_id, canvas }),
                style,
                link: None,
                sources: vec![trace.current_source()],
            });

            return;
        }

        let total = width
            .checked_mul(height)
            .and_then(|pixels| trace.canvas_pixels.checked_add(pixels));

        if let Some(total) = total.filter(|total| {
            *total <= MAX_PAGE_CANVAS_PIXELS && trace.canvas_count < MAX_PAGE_CANVASES
        }) && let Some(canvas) = Canvas2D::new(width, height)
        {
            trace.canvas_pixels = total;
            trace.canvas_count += 1;

            blocks.push(Block {
                content: BlockContent::Canvas(CanvasBlock { node_id, canvas }),
                style,
                link: None,
                sources: vec![trace.current_source()],
            });
        }

        return;
    }

    if element.tag == "img" {
        let url = match (page_url, element.src.as_deref()) {
            (Some(page_url), Some(source)) => page_url.join(source.trim()).ok(),

            _ => None,
        };

        if let Some(url) = url {
            let id = trace.next_image_id;

            let Some(next_id) = id.checked_add(1) else {
                return;
            };

            trace.next_image_id = next_id;

            blocks.push(Block {
                content: BlockContent::PendingImage(ImageRequest { id, url }),
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

    if element.tag == "a" && style.display != Display::Block {
        let target = element
            .href
            .as_deref()
            .and_then(|href| page_url.and_then(|url| url.join(href.trim()).ok()))
            .filter(|url| matches!(url.scheme(), "http" | "https"))
            .map(|url| url.to_string());

        let first_run = trace.text_runs.len();

        if let Some(contents) = element
            .id
            .as_deref()
            .and_then(|id| document.text_content_override(id))
        {
            trace.append_text(current, contents, false, &style);
        } else {
            trace.ancestors.push(element.clone());

            for (index, child) in dom_element.children.iter().enumerate() {
                trace.path.push(index);

                collect_link_text(child, rules, &style, document, current, trace);

                trace.path.pop();
            }

            trace.ancestors.pop();
        }

        for run in &mut trace.text_runs[first_run..] {
            run.link = target.clone();
        }

        return;
    }

    if element.tag == "a" {
        flush(blocks, current, inherited_style, trace);

        let mut text = String::new();

        let mut link_trace = ExtractionTrace {
            path: trace.path.clone(),
            ancestors: trace.ancestors.clone(),
            ..ExtractionTrace::default()
        };

        if let Some(contents) = element
            .id
            .as_deref()
            .and_then(|id| document.text_content_override(id))
        {
            link_trace.append_text(&mut text, contents, false, &style);
        } else {
            link_trace.ancestors.push(element.clone());

            for (index, child) in dom_element.children.iter().enumerate() {
                link_trace.path.push(index);

                collect_link_text(child, rules, &style, document, &mut text, &mut link_trace);

                link_trace.path.pop();
            }
        }

        let target = element
            .href
            .as_deref()
            .and_then(|href| page_url.and_then(|url| url.join(href.trim()).ok()))
            .filter(|url| matches!(url.scheme(), "http" | "https"))
            .map(|url| url.to_string());

        for run in &mut link_trace.text_runs {
            run.link = target.clone();
        }

        if !text.is_empty() {
            blocks.push(Block {
                content: BlockContent::Text(TextBlock {
                    text,
                    runs: link_trace.text_runs,
                }),
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
        trace.append_text(current, contents, false, &style);

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

fn collect_link_text(
    node: &HtmlNode,
    rules: &[CssRule],
    inherited: &TextStyle,
    document: &DocumentState,
    text: &mut String,
    trace: &mut ExtractionTrace,
) {
    if let HtmlNode::Text(contents) = node {
        trace.append_text(text, contents, true, inherited);
        return;
    }

    let (Some(element), HtmlNode::Element(dom_element)) = (element_info(node), node) else {
        return;
    };

    if matches!(
        element.tag.as_str(),
        "script"
            | "style"
            | "template"
            | "head"
            | "title"
            | "meta"
            | "link"
            | "br"
            | "img"
            | "svg"
            | "canvas"
    ) {
        return;
    }

    let ancestors: Vec<_> = trace
        .ancestors
        .iter()
        .map(ElementInfo::css_element)
        .collect();

    let style =
        compute_style_with_ancestors(&element.css_element(), inherited.clone(), rules, &ancestors);

    if style.display == Display::None {
        return;
    }

    if let Some(contents) = element
        .id
        .as_deref()
        .and_then(|id| document.text_content_override(id))
    {
        trace.append_text(text, contents, false, &style);
        return;
    }

    trace.ancestors.push(element);

    for (index, child) in dom_element.children.iter().enumerate() {
        trace.path.push(index);

        collect_link_text(child, rules, &style, document, text, trace);

        trace.path.pop();
    }

    trace.ancestors.pop();
}

// Partial white-space: normal processing across inline text nodes.
// https://www.w3.org/TR/css-text-3/#white-space-processing
fn append_collapsed(target: &mut String, source: &str, pending_space: &mut bool) {
    for ch in source.chars() {
        if matches!(ch, ' ' | '\t' | '\n' | '\r' | '\x0c') {
            *pending_space = !target.is_empty();
        } else {
            if *pending_space && !target.is_empty() {
                target.push(' ');
            }

            target.push(ch);
            *pending_space = false;
        }
    }
}

fn flush(
    blocks: &mut Vec<Block>,
    current: &mut String,
    style: TextStyle,
    trace: &mut ExtractionTrace,
) {
    let text = std::mem::take(current);
    trace.pending_space = false;

    let sources = std::mem::take(&mut trace.pending);
    let runs = std::mem::take(&mut trace.text_runs);

    if !text.is_empty() {
        blocks.push(Block {
            content: BlockContent::Text(TextBlock { text, runs }),
            style,
            link: None,
            sources,
        });
    }
}

#[cfg(test)]
mod tests;
