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
use std::path::{Path, PathBuf};

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

#[derive(Debug, PartialEq, Eq)]
enum BatchResult {
    Compared(PreviewResult),
    NoReference,
}

#[derive(Debug)]
struct BatchEntry {
    path: PathBuf,
    result: BatchResult,
}

/// Finds a single same-directory XHTML reference.
///
/// URL forms requiring resolution or decoding are not supported yet.
fn reference_name(source: &str) -> Result<Option<String>, String> {
    let document = crate::xml::parse_xhtml(source).map_err(|error| error.to_string())?;

    let Some(head) = document.first_element("head") else {
        return Ok(None);
    };

    let mut reference = None;

    for child in &head.children {
        let crate::html::Node::Element(element) = child else {
            continue;
        };

        let attribute = |name: &str| {
            element
                .attributes
                .iter()
                .find(|attribute| attribute.name == name)
                .map(|attribute| attribute.value.as_str())
        };

        if element.name == "meta"
            && attribute("name").is_some_and(|name| name.eq_ignore_ascii_case("fuzzy"))
        {
            return Err("fuzzy metadata is not supported".into());
        }

        if element.name != "link" {
            continue;
        }

        let rel = attribute("rel").unwrap_or("");

        let is_match = rel
            .split_ascii_whitespace()
            .any(|part| part.eq_ignore_ascii_case("match"));

        let is_mismatch = rel
            .split_ascii_whitespace()
            .any(|part| part.eq_ignore_ascii_case("mismatch"));

        if is_mismatch {
            return Err("mismatch references are not supported".into());
        }

        if !is_match {
            continue;
        }

        let href = attribute("href").ok_or("match link is missing href")?;

        let supported_name = !href.is_empty()
            && href != "."
            && href != ".."
            && href.ends_with(".xht")
            && href
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));

        if !supported_name {
            return Err(format!("unsupported reference URL: {href}"));
        }

        if reference.replace(href.to_owned()).is_some() {
            return Err("multiple match references are not supported".into());
        }
    }

    Ok(reference)
}

/// Scans one directory without recursive WPT discovery.
fn run_directory(
    directory: &Path,
    width: usize,
    height: usize,
) -> std::io::Result<Vec<BatchEntry>> {
    let mut paths = Vec::new();

    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;

        if entry.file_type()?.is_file() && entry.path().extension().is_some_and(|ext| ext == "xht")
        {
            paths.push(entry.path());
        }
    }

    paths.sort();

    Ok(paths
        .into_iter()
        .map(|path| {
            let result = run_file(&path, width, height);

            BatchEntry { path, result }
        })
        .collect())
}

fn run_file(path: &Path, width: usize, height: usize) -> BatchResult {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,

        Err(error) => {
            return BatchResult::Compared(PreviewResult::Error(format!(
                "cannot read {}: {error}",
                path.display(),
            )));
        }
    };

    let name = match reference_name(&source) {
        Ok(Some(name)) => name,

        Ok(None) => return BatchResult::NoReference,

        Err(reason) => {
            return BatchResult::Compared(PreviewResult::Unsupported {
                document: "test",
                reason,
            });
        }
    };

    let reference_path = path.with_file_name(name);

    let reference = match std::fs::read_to_string(&reference_path) {
        Ok(source) => source,

        Err(error) => {
            return BatchResult::Compared(PreviewResult::Error(format!(
                "cannot read {}: {error}",
                reference_path.display(),
            )));
        }
    };

    BatchResult::Compared(compare_xhtml_preview(&source, &reference, width, height))
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

    fn with_head(head: &str) -> String {
        format!(
            concat!(
                "<html xmlns='http://www.w3.org/1999/xhtml'>",
                "<head>{}</head>",
                "<body/>",
                "</html>",
            ),
            head
        )
    }

    #[test]
    fn discovers_one_relative_reference_from_xhtml_head() {
        let source = with_head(concat!(
            "<link rel='author' href='author'/>",
            "<link rel='alternate MATCH' href='example-ref.xht'/>",
        ));

        assert_eq!(
            reference_name(&source).unwrap().as_deref(),
            Some("example-ref.xht")
        );

        assert_eq!(reference_name(&with_head("")).unwrap(), None);
    }

    #[test]
    fn refuses_unsupported_reference_metadata() {
        for head in [
            "<link rel='match' href='../ref.xht'/>",
            "<link rel='match' href='ref.xht?x'/>",
            "<link rel='mismatch' href='ref.xht'/>",
            "<meta name='fuzzy' content='0-1;0-3'/>",
            concat!(
                "<link rel='match' href='a.xht'/>",
                "<link rel='match' href='b.xht'/>"
            ),
            "<link rel='match'/>",
        ] {
            assert!(reference_name(&with_head(head)).is_err(), "{head}");
        }
    }

    #[test]
    fn scans_saved_directory_without_claiming_wpt_results() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/wpt/css/CSS2/margin-padding-clear");

        let entries =
            run_directory(&directory, 320, 200).expect("cannot read WPT fixture directory");

        for entry in &entries {
            println!("preview only: {}: {:?}", entry.path.display(), entry.result);
        }

        let target = entries
            .iter()
            .find(|entry| {
                entry
                    .path
                    .file_name()
                    .is_some_and(|name| name == "margin-collapse-003.xht")
            })
            .expect("test fixture missing");

        assert_eq!(target.result, BatchResult::Compared(PreviewResult::Match));

        let reference = entries
            .iter()
            .find(|entry| {
                entry
                    .path
                    .file_name()
                    .is_some_and(|name| name == "margin-collapse-003-ref.xht")
            })
            .expect("reference fixture missing");

        assert_eq!(reference.result, BatchResult::NoReference);
    }
}
