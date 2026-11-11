//! Diagnostic comparisons using the experimental layout preview.
//!
//! A matching image is not a WPT pass: the preview does not yet detect all
//! unsupported CSS or provide a standards-conforming browser viewport.

use crate::{
    layout_preview::PreviewLayout,
    page::parse_xhtml_for_test,
    page_layout::CONTENT_TOP,
    render::{GlyphRasterCache, paint_preview_scene},
};

#[derive(Debug, PartialEq, Eq)]
enum PreviewResult {
    Match,

    Mismatch {
        different_pixels: usize,
        max_channel_difference: u8,
    },

    Unsupported {
        document: &'static str,
        reason: String,
    },

    Error(String),
}

fn compare_xhtml_preview(
    test_source: &str,
    reference_source: &str,
    width: usize,
    height: usize,
) -> PreviewResult {
    // These dimensions include the toolbar, matching the current preview.
    let Some(pixel_count) = width.checked_mul(height) else {
        return PreviewResult::Error("preview dimensions overflow".into());
    };

    if width == 0 || height <= CONTENT_TOP as usize || pixel_count > 16_777_216 {
        return PreviewResult::Error("invalid or excessive preview dimensions".into());
    }

    let mut images = Vec::with_capacity(2);

    for (label, source) in [("test", test_source), ("reference", reference_source)] {
        let page = match parse_xhtml_for_test("https://example.test/", source) {
            Ok(page) => page,

            Err(error) => {
                return PreviewResult::Unsupported {
                    document: label,
                    reason: error.to_string(),
                };
            }
        };

        let layout = match PreviewLayout::build(&page, width, None) {
            Ok(layout) => layout,

            Err(error) => {
                return PreviewResult::Unsupported {
                    document: label,
                    reason: error.to_string(),
                };
            }
        };

        let mut pixels = vec![0; pixel_count];
        let mut cache = GlyphRasterCache::new();

        paint_preview_scene(
            &mut pixels,
            width,
            height,
            &page,
            &layout,
            None,
            0,
            &mut cache,
        );

        images.push(pixels);
    }

    let mut different_pixels = 0;
    let mut maximum = 0;

    for (&test, &reference) in images[0].iter().zip(&images[1]) {
        if test != reference {
            different_pixels += 1;

            for shift in [0, 8, 16] {
                let test_channel = ((test >> shift) & 0xff) as u8;
                let reference_channel = ((reference >> shift) & 0xff) as u8;

                maximum = maximum.max(test_channel.abs_diff(reference_channel));
            }
        }
    }

    if different_pixels == 0 {
        PreviewResult::Match
    } else {
        PreviewResult::Mismatch {
            different_pixels,
            max_channel_difference: maximum,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLUE: &str = concat!(
        "<html xmlns='http://www.w3.org/1999/xhtml'>",
        "<body>",
        "<div style='width:50px;height:20px;background:blue'></div>",
        "</body></html>",
    );

    const RED: &str = concat!(
        "<html xmlns='http://www.w3.org/1999/xhtml'>",
        "<body>",
        "<div style='width:50px;height:20px;background:red'></div>",
        "</body></html>",
    );

    #[test]
    fn matches_identical_supported_documents() {
        assert_eq!(
            compare_xhtml_preview(BLUE, BLUE, 320, 200),
            PreviewResult::Match
        );
    }

    #[test]
    fn reports_pixel_difference() {
        assert_eq!(
            compare_xhtml_preview(RED, BLUE, 320, 200),
            PreviewResult::Mismatch {
                different_pixels: 1000,
                max_channel_difference: 255,
            }
        );
    }

    #[test]
    fn reports_unsupported_layout_on_each_side() {
        let unsupported = concat!(
            "<html xmlns='http://www.w3.org/1999/xhtml'>",
            "<body>",
            "<div>A<span style='color:red'>B</span></div>",
            "</body></html>",
        );

        for (test, reference, document) in [
            (unsupported, BLUE, "test"),
            (BLUE, unsupported, "reference"),
        ] {
            assert!(matches!(
                compare_xhtml_preview(test, reference, 320, 200),
                PreviewResult::Unsupported {
                    document: actual,
                    ..
                } if actual == document
            ));
        }
    }

    #[test]
    fn rejects_invalid_viewports() {
        assert!(matches!(
            compare_xhtml_preview(BLUE, BLUE, 0, 200),
            PreviewResult::Error(_)
        ));

        assert!(matches!(
            compare_xhtml_preview(BLUE, BLUE, usize::MAX, 2),
            PreviewResult::Error(_)
        ));
    }

    #[test]
    fn compares_saved_wpt_pair_as_preview_only() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/wpt/css/CSS2/margin-padding-clear");

        let read = |name| std::fs::read_to_string(base.join(name)).expect("WPT fixture must exist");

        let result = compare_xhtml_preview(
            &read("margin-collapse-003.xht"),
            &read("margin-collapse-003-ref.xht"),
            320,
            200,
        );

        println!("Experimental preview result (not WPT): {result:?}");

        assert_eq!(result, PreviewResult::Match);
    }
}
