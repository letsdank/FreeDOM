//! Diagnostic comparisons using the experimental layout preview.
//!
//! A matching image is not a WPT pass: the preview does not yet detect all
//! unsupported CSS or provide a standards-conforming browser viewport.

use crate::{
    layout_preview::PreviewLayout,
    page::parse_xhtml_for_test,
    page_layout::CONTENT_TOP,
    render::{GlyphRasterCache, paint_preview_scene},
    url::Url,
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

#[derive(Debug, Default, PartialEq, Eq)]
struct BatchSummary {
    matched: usize,
    mismatched: usize,
    unsupported: usize,
    errors: usize,
    no_reference: usize,
}

impl BatchSummary {
    fn from_entries(entries: &[BatchEntry]) -> Self {
        let mut summary = Self::default();

        for entry in entries {
            match &entry.result {
                BatchResult::Compared(PreviewResult::Match) => {
                    summary.matched += 1;
                }

                BatchResult::Compared(PreviewResult::Mismatch { .. }) => {
                    summary.mismatched += 1;
                }

                BatchResult::Compared(PreviewResult::Unsupported { .. }) => {
                    summary.unsupported += 1;
                }

                BatchResult::Compared(PreviewResult::Error(_)) => {
                    summary.errors += 1;
                }

                BatchResult::NoReference => {
                    summary.no_reference += 1;
                }
            }
        }

        summary
    }
}

/// Finds a single XHTML reference.
///
/// URL forms requiring percent-decoding or a query are not supported yet.
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

        if !is_supported_reference_href(href) {
            return Err(format!("unsupported reference URL: {href}"));
        }

        if reference.replace(href.to_owned()).is_some() {
            return Err("multiple match references are not supported".into());
        }
    }

    Ok(reference)
}

/// Scans XHTML files recursively without following directory symlinks.
fn run_directory(
    root: &Path,
    directory: &Path,
    width: usize,
    height: usize,
) -> std::io::Result<Vec<BatchEntry>> {
    if !std::fs::symlink_metadata(directory)?.file_type().is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "root must be a non-symlink directory",
        ));
    }

    let root = root.canonicalize()?;
    let directory = directory.canonicalize()?;

    if !directory.starts_with(&root) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "directory is outside WPT root",
        ));
    }

    let mut paths = Vec::new();
    let mut pending = vec![directory];

    while let Some(current) = pending.pop() {
        for entry in std::fs::read_dir(current)? {
            let entry = entry?;
            let file_type = entry.file_type()?;

            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file()
                && entry.path().extension().is_some_and(|ext| ext == "xht")
            {
                paths.push(entry.path());
            }
        }
    }

    paths.sort();

    Ok(paths
        .into_iter()
        .map(|path| {
            let result = run_file(&root, &path, width, height);

            BatchEntry { path, result }
        })
        .collect())
}

fn run_file(root: &Path, path: &Path, width: usize, height: usize) -> BatchResult {
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

    let reference_path = match resolve_reference(root, path, &name) {
        Ok(path) => path,
        Err(result) => return BatchResult::Compared(result),
    };

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

fn is_supported_reference_href(href: &str) -> bool {
    !href.is_empty()
        && !href.starts_with("//")
        && href.ends_with(".xht")
        && href
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
}

fn resolve_reference_url(test_relative: &Path, href: &str) -> Result<PathBuf, String> {
    if !is_supported_reference_href(href) {
        return Err(format!("unsupported reference URL: {href}"));
    }

    let mut components = Vec::new();

    for component in test_relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err("invalid test path".into());
        };

        let name = name.to_str().ok_or("non-UTF-8 test path")?;

        if !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err("unsupported test path".into());
        }

        components.push(name);
    }

    if components.is_empty() {
        return Err("empty test path".into());
    }

    const ORIGIN: &str = "https://wpt.invalid/";

    let base = Url::parse(&format!("{ORIGIN}{}", components.join("/")))
        .map_err(|error| error.to_string())?;

    let joined = base
        .join(href)
        .map_err(|error| error.to_string())?
        .to_string();

    let relative = joined
        .strip_prefix(ORIGIN)
        .ok_or("reference has a different origin")?;

    let result = PathBuf::from(relative);

    if !result
        .components()
        .all(|part| matches!(part, std::path::Component::Normal(_)))
    {
        return Err("invalid resolved reference path".into());
    }

    Ok(result)
}

fn resolve_reference(root: &Path, test: &Path, href: &str) -> Result<PathBuf, PreviewResult> {
    let relative = test
        .strip_prefix(root)
        .map_err(|_| PreviewResult::Error("test is outside WPT root".into()))?;

    let relative =
        resolve_reference_url(relative, href).map_err(|reason| PreviewResult::Unsupported {
            document: "test",
            reason,
        })?;

    let candidate = root.join(relative);

    let reference = candidate.canonicalize().map_err(|error| {
        PreviewResult::Error(format!("cannot resolve {}: {error}", candidate.display()))
    })?;

    if !reference.starts_with(root) {
        return Err(PreviewResult::Unsupported {
            document: "test",
            reason: "reference is outside WPT root".into(),
        });
    }

    if reference == test {
        return Err(PreviewResult::Unsupported {
            document: "test",
            reason: "test cannot reference itself".into(),
        });
    }

    Ok(reference)
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

        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/wpt");

        let entries =
            run_directory(&root, &directory, 320, 200).expect("cannot read WPT fixture directory");

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

    #[test]
    fn summaries_each_diagnostic_outcome() {
        let results = vec![
            BatchResult::Compared(PreviewResult::Match),
            BatchResult::Compared(PreviewResult::Mismatch {
                different_pixels: 3,
                max_channel_difference: 20,
            }),
            BatchResult::Compared(PreviewResult::Unsupported {
                document: "test",
                reason: "unsupported".into(),
            }),
            BatchResult::Compared(PreviewResult::Error("error".into())),
            BatchResult::NoReference,
        ];

        let entries: Vec<_> = results
            .into_iter()
            .enumerate()
            .map(|(index, result)| BatchEntry {
                path: PathBuf::from(index.to_string()),
                result,
            })
            .collect();

        assert_eq!(
            BatchSummary::from_entries(&entries),
            BatchSummary {
                matched: 1,
                mismatched: 1,
                unsupported: 1,
                errors: 1,
                no_reference: 1,
            }
        );
    }

    #[test]
    fn scans_saved_css2_tree_and_prints_preview_summary() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/wpt/css/CSS2");
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/wpt");

        let entries = run_directory(&root, &directory, 320, 200).expect("cannot scan fixture tree");

        for entry in &entries {
            println!("preview only: {}: {:?}", entry.path.display(), entry.result);
        }

        let summary = BatchSummary::from_entries(&entries);

        println!("experimental preview summary (not WPT): {summary:?}");

        assert!(entries.iter().any(|entry| {
            entry
                .path
                .ends_with("margin-padding-clear/margin-collapse-003.xht")
                && entry.result == BatchResult::Compared(PreviewResult::Match)
        }));

        assert!(entries.iter().any(|entry| {
            entry.path.ends_with("cascade/specificity-007.xht")
                && entry.result == BatchResult::Compared(PreviewResult::Match)
        }));
    }

    #[test]
    fn resolves_supported_reference_urls() {
        let test = Path::new("css/CSS2/cascade/test.xth");

        assert_eq!(
            resolve_reference_url(test, "../reference/a.xht").unwrap(),
            PathBuf::from("css/CSS2/reference/a.xht")
        );

        assert_eq!(
            resolve_reference_url(test, "/css/ref.xht").unwrap(),
            PathBuf::from("css/ref.xht")
        );

        assert_eq!(
            resolve_reference_url(test, "local.xht").unwrap(),
            PathBuf::from("css/CSS2/cascade/local.xht")
        );

        for href in [
            "//other.example/a.xht",
            "https://other/a.xht",
            "a.xht?q",
            "a%2fx.xht",
            "a\\b.xht",
        ] {
            assert!(resolve_reference_url(test, href).is_err(), "{href}");
        }
    }

    #[test]
    fn refuses_self_reference_and_missing_reference() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/wpt")
            .canonicalize()
            .unwrap();

        let test = root.join("css/CSS2/margin-padding-clear/margin-collapse-003.xht");

        assert!(matches!(
            resolve_reference(&root, &test, "margin-collapse-003.xht"),
            Err(PreviewResult::Unsupported { .. })
        ));

        assert!(matches!(
            resolve_reference(&root, &test, "missing-ref.xht"),
            Err(PreviewResult::Error(_))
        ));

        assert!(resolve_reference(&root, &test, "margin-collapse-003-ref.xht").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_reference_symlink_outside_root() {
        use std::os::unix::fs::symlink;

        struct TempDir(PathBuf);

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        let path =
            std::env::temp_dir().join(format!("freedom-reftest-{}-{unique}", std::process::id(),));

        std::fs::create_dir(&path).unwrap();
        let temp = TempDir(path);

        let root = temp.0.join("wpt");
        std::fs::create_dir(&root).unwrap();

        let outside = temp.0.join("outside.xht");
        std::fs::write(&outside, "outside").unwrap();

        symlink(&outside, root.join("reference.xht")).unwrap();

        let root = root.canonicalize().unwrap();
        let test = root.join("test.xht");

        assert!(matches!(
            resolve_reference(&root, &test, "reference.xht"),
            Err(PreviewResult::Unsupported { .. })
        ));
    }
}
