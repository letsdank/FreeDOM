use std::fs;

use crate::graphics::{
    raster::{Color, Surface},
    svg::{SvgDocument, parse_svg},
};

struct SvgReftestReport {
    viewport_width: usize,
    viewport_height: usize,
    different_pixels: usize,
    max_channel_difference: u8,
}

fn run_svg_reftest(test_source: &str, reference_source: &str) -> Result<SvgReftestReport, String> {
    let test = parse_svg(test_source).map_err(|error| error.to_string())?;
    let reference = parse_svg(reference_source).map_err(|error| error.to_string())?;

    let test_viewport = viewport_size(&test);
    let reference_viewport = viewport_size(&reference);

    if test_viewport != reference_viewport {
        return Err(format!(
            "WPT reftest uses different viewport sizes: test {:?}, reference {:?}",
            test_viewport, reference_viewport
        ));
    }

    let (width, height) = test_viewport;

    let actual = test
        .render(width, height, Color::WHITE)
        .map_err(|error| error.to_string())?;

    let expected = reference
        .render(width, height, Color::WHITE)
        .map_err(|error| error.to_string())?;

    Ok(compare_surfaces(&actual, &expected))
}

fn viewport_size(document: &SvgDocument) -> (usize, usize) {
    let dimension = |value: Option<f32>, fallback| {
        value
            .filter(|value| value.is_finite() && *value > 0.0)
            .map(|value| value.round() as usize)
            .unwrap_or(fallback)
            .clamp(1, 4096)
    };

    (
        dimension(document.width, 300),
        dimension(document.height, 150),
    )
}

fn compare_surfaces(actual: &Surface, expected: &Surface) -> SvgReftestReport {
    assert_eq!(actual.width(), expected.width());
    assert_eq!(actual.height(), expected.height());

    let mut different_pixels = 0;
    let mut max_channel_difference = 0;

    for y in 0..actual.height() {
        for x in 0..actual.width() {
            let actual_pixel = actual.pixel(x, y).expect("pixel lies in actual surface");
            let expected_pixel = expected
                .pixel(x, y)
                .expect("pixel lies in expected surface");

            if actual_pixel == expected_pixel {
                continue;
            }

            different_pixels += 1;

            let difference = [
                actual_pixel.red.abs_diff(expected_pixel.red),
                actual_pixel.green.abs_diff(expected_pixel.green),
                actual_pixel.blue.abs_diff(expected_pixel.blue),
                actual_pixel.alpha.abs_diff(expected_pixel.alpha),
            ]
            .into_iter()
            .max()
            .unwrap_or(0);

            max_channel_difference = max_channel_difference.max(difference);
        }
    }

    SvgReftestReport {
        viewport_width: actual.width(),
        viewport_height: actual.height(),
        different_pixels,
        max_channel_difference,
    }
}

fn fixture_path(relative_path: &str) -> String {
    format!(
        "{}/testdata/wpt/svg/{relative_path}",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn css_fixture_path(relative_path: &str) -> String {
    format!(
        "{}/testdata/wpt/css/{relative_path}",
        env!("CARGO_MANIFEST_DIR")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_equivalent_svg_renderings() {
        let report = run_svg_reftest(
            r##"
                <svg width="8" height="8">
                    <path fill="#00ff00" d="M 0 0 H 8 V 8 H 0 Z"/>
                </svg>
            "##,
            r##"
                <svg width="8" height="8">
                    <rect width="8" height="8" fill="#00ff00"/>
                </svg>
            "##,
        )
        .unwrap();

        assert_eq!(report.different_pixels, 0);
        assert_eq!(report.max_channel_difference, 0);
    }

    #[test]
    fn official_wpt_svg_requiredextensions_empty_string() {
        let test_source = fs::read_to_string(fixture_path(
            "struct/reftests/requiredextensions-empty-string.svg",
        ))
        .expect("WPT SVG test fixture is missing");

        let reference_source =
            fs::read_to_string(fixture_path("struct/scripted/support/blank.svg"))
                .expect("WPT SVG reference fixture is missing");

        let report = run_svg_reftest(&test_source, &reference_source).unwrap();

        eprintln!(
            "WPT SVG result: {}x{}, {} differing pixels, maximum channel different {}",
            report.viewport_width,
            report.viewport_height,
            report.different_pixels,
            report.max_channel_difference
        );

        assert_eq!(
            report.different_pixels, 0,
            "SVG reftest differs in {} pixels; maximum channel difference is {}",
            report.different_pixels, report.max_channel_difference
        );
    }

    #[test]
    fn official_wpt_svg_viewbox_zero_disables_rendering() {
        let test_source = fs::read_to_string(fixture_path(
            "coordinate-systems/viewBox-zero-disables-rendering-001.svg",
        ))
        .expect("WPT SVG test fixture is missing");

        let reference_source = fs::read_to_string(fixture_path(
            "coordinate-systems/viewBox-zero-disables-rendering-ref.svg",
        ))
        .expect("WPT SVG reference fixture is missing");

        let report = run_svg_reftest(&test_source, &reference_source).unwrap();

        eprintln!(
            "WPT SVG result: {}x{}, {} differing pixels, maximum channel difference {}",
            report.viewport_width,
            report.viewport_height,
            report.different_pixels,
            report.max_channel_difference
        );

        assert_eq!(report.different_pixels, 0);
    }

    #[test]
    fn official_wpt_svg_paint_fallbacks() {
        let reference_source = fs::read_to_string(fixture_path(
            "pservers/reftests/reference/green-100x100.svg",
        ))
        .expect("WPT SVG reference fixture is missing");

        for fixture in [
            "painting/reftests/fallback-001.svg",
            "painting/reftests/fallback-002.svg",
        ] {
            let test_source =
                fs::read_to_string(fixture_path(fixture)).expect("WPT SVG test fixture is missing");

            let report = run_svg_reftest(&test_source, &reference_source).unwrap();

            eprintln!(
                "WPT SVG {fixture}: {}x{}, {} differing pixels, maximum channel difference {}",
                report.viewport_width,
                report.viewport_height,
                report.different_pixels,
                report.max_channel_difference,
            );

            assert_eq!(
                report.different_pixels, 0,
                "{fixture}: {} differing pixels, maximum channel difference {}",
                report.different_pixels, report.max_channel_difference
            )
        }
    }

    #[test]
    fn official_wpt_svg_zero_height_and_zero_sized_viewboxes_disable_rendering() {
        let reference_source = fs::read_to_string(fixture_path(
            "coordinate-systems/viewBox-zero-disables-rendering-ref.svg",
        ))
        .expect("WPT SVG reference fixture is missing");

        for fixture in [
            "coordinate-systems/viewBox-zero-disables-rendering-002.svg",
            "coordinate-systems/viewBox-zero-disables-rendering-003.svg",
        ] {
            let test_source =
                fs::read_to_string(fixture_path(fixture)).expect("WPT SVG test fixture is missing");

            let report = run_svg_reftest(&test_source, &reference_source).unwrap();

            eprintln!(
                "WPT SVG {fixture}: {}x{}, {} differing pixels, maximum channel difference {}",
                report.viewport_width,
                report.viewport_height,
                report.different_pixels,
                report.max_channel_difference,
            );

            assert_eq!(
                report.different_pixels, 0,
                "{fixture}: {} differing pixels, maximum channel difference {}",
                report.different_pixels, report.max_channel_difference
            );
        }
    }

    #[test]
    fn official_wpt_svg_use_can_target_a_element() {
        let test_source = fs::read_to_string(fixture_path("struct/reftests/use-a.svg"))
            .expect("WPT SVG test fixture is missing");

        let reference_source =
            fs::read_to_string(fixture_path("struct/reftests/reference/green-100x100.svg"))
                .expect("WPT SVG reference fixture is missing");

        let report = run_svg_reftest(&test_source, &reference_source).unwrap();

        eprintln!(
            "WPT SVG use-a: {}x{}, {} differing pixels, maximum channel difference {}",
            report.viewport_width,
            report.viewport_height,
            report.different_pixels,
            report.max_channel_difference
        );

        assert_eq!(
            report.different_pixels, 0,
            "SVG reftest differs in {} pixels; maximum channel difference is {}",
            report.different_pixels, report.max_channel_difference
        )
    }

    #[test]
    fn official_wpt_svg_inner_svg_transform_and_viewbox() {
        let test_source = fs::read_to_string(fixture_path(
            "struct/reftests/inner-svg-transform-and-viewbox.svg",
        ))
        .expect("WPT SVG test fixture is missing");

        let reference_source = fs::read_to_string(fixture_path(
            "struct/reftests/reference/inner-svg-transform-and-viewbox-ref.svg",
        ))
        .expect("WPT SVG reference fixture is missing");

        let report = run_svg_reftest(&test_source, &reference_source).unwrap();

        assert_eq!(
            report.different_pixels, 0,
            "SVG reftest differs in {} pixels; maximum channel difference is {}",
            report.different_pixels, report.max_channel_difference
        );
    }

    #[test]
    fn official_wpt_svg_use_decodes_percent_encoded_fragments() {
        let test_source = fs::read_to_string(fixture_path("struct/reftests/use-encoding.svg"))
            .expect("WPT SVG test fixture is missing");

        let reference_source =
            fs::read_to_string(fixture_path("struct/reftests/reference/green-100x100.svg"))
                .expect("WPT SVG reference fixture is missing");

        let report = run_svg_reftest(&test_source, &reference_source).unwrap();

        assert_eq!(
            report.different_pixels, 0,
            "SVG reftest differs in {} pixels; maximum channel difference is {}",
            report.different_pixels, report.max_channel_difference
        )
    }

    #[test]
    fn official_wpt_svg_use_can_target_switch() {
        let test_source = fs::read_to_string(fixture_path("struct/reftests/use-switch.svg"))
            .expect("WPT SVG test fixture is missing");

        let reference_source =
            fs::read_to_string(fixture_path("struct/reftests/reference/green-100x100.svg"))
                .expect("WPT SVG reference fixture is missing");

        let report = run_svg_reftest(&test_source, &reference_source).unwrap();

        assert_eq!(
            report.different_pixels, 0,
            "SVG reftest differs in {} pixels; maximum channel difference is {}",
            report.different_pixels, report.max_channel_difference
        );
    }

    #[test]
    fn official_wpt_svg_clip_path_uses_nonzero_rule() {
        let test_source = fs::read_to_string(css_fixture_path(
            "css-masking/clip-path-svg-content/clip-path-clip-rule-002.svg",
        ))
        .expect("WPT SVG test fixture is missing");

        let reference_source = fs::read_to_string(css_fixture_path(
            "css-masking/clip-path-svg-content/reference/clip-path-square-001-ref.svg",
        ))
        .expect("WPT SVG reference fixture is missing");

        let report = run_svg_reftest(&test_source, &reference_source).unwrap();

        eprintln!(
            "WPT SVG result: {}x{}, {} differing pixels, maximum channel difference {}",
            report.viewport_width,
            report.viewport_height,
            report.different_pixels,
            report.max_channel_difference
        );

        assert_eq!(report.different_pixels, 0);
        assert_eq!(report.max_channel_difference, 0);
    }

    #[test]
    fn official_wpt_svg_clip_path_uses_evenodd_rule() {
        let test_source = fs::read_to_string(css_fixture_path(
            "css-masking/clip-path-svg-content/clip-path-clip-rule-001.svg",
        ))
        .expect("WPT SVG test fixture is missing");

        let reference_source = fs::read_to_string(css_fixture_path(
            "css-masking/clip-path-svg-content/reference/clip-path-square-hole-001-ref.svg",
        ))
        .expect("WPT SVG reference fixture is missing");

        let report = run_svg_reftest(&test_source, &reference_source).unwrap();

        assert_eq!(report.different_pixels, 0);
        assert_eq!(report.max_channel_difference, 0);
    }

    #[test]
    fn official_wpt_svg_clip_path_can_reference_another_clip_path() {
        let test_source = fs::read_to_string(css_fixture_path(
            "css-masking/clip-path-svg-content/clip-path-clip.svg",
        ))
        .expect("WPT SVG test fixture is missing");

        let reference_source = fs::read_to_string(css_fixture_path(
            "css-masking/clip-path-svg-content/reference/clip-path-square-002-ref.svg",
        ))
        .expect("WPT SVG reference fixture is missing");

        let report = run_svg_reftest(&test_source, &reference_source).unwrap();

        assert_eq!(report.different_pixels, 0);
        assert_eq!(report.max_channel_difference, 0);
    }

    #[test]
    fn official_wpt_svg_clip_path_content_is_clipped() {
        let test_source = fs::read_to_string(css_fixture_path(
            "css-masking/clip-path-svg-content/clip-path-content-clip-001.svg",
        ))
        .expect("WPT SVG test fixture is missing");

        let reference_source = fs::read_to_string(css_fixture_path(
            "css-masking/clip-path-svg-content/reference/clip-path-square-002-ref.svg",
        ))
        .expect("WPT SVG reference fixture is missing");

        let report = run_svg_reftest(&test_source, &reference_source).unwrap();

        assert_eq!(report.different_pixels, 0);
        assert_eq!(report.max_channel_difference, 0);
    }

    #[test]
    fn official_wpt_svg_clip_path_can_be_nested_twice() {
        let test_source = fs::read_to_string(css_fixture_path(
            "css-masking/clip-path-svg-content/clip-path-clip-nested-twice.svg",
        ))
        .expect("WPT SVG test fixture is missing");

        let reference_source = fs::read_to_string(css_fixture_path(
            "css-masking/clip-path-svg-content/reference/clip-path-square-002-ref.svg",
        ))
        .expect("WPT SVG reference fixture is missing");

        let report = run_svg_reftest(&test_source, &reference_source).unwrap();

        assert_eq!(report.different_pixels, 0);
        assert_eq!(report.max_channel_difference, 0);
    }

    #[test]
    fn official_wpt_svg_clip_path_expands_use_content() {
        let test_source = fs::read_to_string(css_fixture_path(
            "css-masking/clip-path-svg-content/clip-path-content-use-001.svg",
        ))
        .expect("WPT SVG test fixture is missing");

        let reference_source = fs::read_to_string(css_fixture_path(
            "css-masking/clip-path-svg-content/reference/clip-path-square-002-ref.svg",
        ))
        .expect("WPT SVG reference fixture is missing");

        let report = run_svg_reftest(&test_source, &reference_source).unwrap();

        assert_eq!(report.different_pixels, 0);
        assert_eq!(report.max_channel_difference, 0);
    }

    #[test]
    fn official_wpt_svg_clip_path_clips_expanded_use_content() {
        let test_source = fs::read_to_string(css_fixture_path(
            "css-masking/clip-path-svg-content/clip-path-content-use-002.svg",
        ))
        .expect("WPT SVG test fixture is missing");

        let reference_source = fs::read_to_string(css_fixture_path(
            "css-masking/clip-path-svg-content/reference/clip-path-square-002-ref.svg",
        ))
        .expect("WPT SVG reference fixture is missing");

        let report = run_svg_reftest(&test_source, &reference_source).unwrap();

        assert_eq!(report.different_pixels, 0);
        assert_eq!(report.max_channel_difference, 0);
    }

    #[test]
    fn official_wpt_svg_stroke_miter_limit_threshold() {
        let source = fs::read_to_string(fixture_path("import/painting-stroke-07-t-manual.svg"))
            .expect("WPT SVG test fixture is missing");

        let document = parse_svg(&source).unwrap();

        assert_eq!(document.width, None);
        assert_eq!(document.height, None);

        let surface = document.render(480, 360, Color::WHITE).unwrap();

        for y in [72, 108] {
            assert_eq!(
                surface.pixel(380, y),
                Some(Color::BLACK),
                "stroke-miterlimit at y={y} should preserve the miter",
            );
        }

        for y in [144, 180, 216, 252] {
            assert_eq!(
                surface.pixel(380, y),
                Some(Color::WHITE),
                "stroke-miterlimit at y={y} should fall back to a bevel join",
            );
        }
    }
}
