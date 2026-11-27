//! The layout inspector overlay, styled snapshots and block source tracking.

use super::*;

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
                content: BlockContent::Text(TextBlock {
                    text: "Hello".into(),
                    runs: vec![TextRun {
                        range: 0..5,
                        style: TextStyle::initial(),
                        link: None,
                    }],
                }),
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
        document: DocumentState::default(),
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
        &mut XrgbFrame::new(&mut first, width, height).unwrap(),
        &page,
        &layout,
        PaintOptions::default(),
        &mut cache,
        &mut glyphs,
    );

    let mut overlay = first.clone();

    paint_layout_overlay(
        &mut XrgbFrame::new(&mut overlay, width, height).unwrap(),
        &layout,
        0,
    );

    assert_ne!(overlay, first);

    let mut again = vec![0; width * height];

    paint(
        &mut XrgbFrame::new(&mut again, width, height).unwrap(),
        &page,
        &layout,
        PaintOptions::default(),
        &mut cache,
        &mut glyphs,
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
