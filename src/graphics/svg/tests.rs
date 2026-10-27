use super::*;
use crate::graphics::geometry::PathCommand;
use crate::url::Url;

#[test]
fn parses_path_fill_and_group_transform() {
    let document = parse_svg(
        r##"
                <svg>
                    <g fill="#00ff00" transform="translate(2 3)">
                        <path d="M 0 0 L 4 0 L 4 4 Z"/>
                    </g>
                </svg>
            "##,
    )
    .unwrap();

    assert_eq!(document.shapes.len(), 1);
    assert_eq!(document.shapes[0].fill, Some(Color::rgba(0, 255, 0, 255)));

    assert_eq!(
        document.shapes[0].path.commands()[0],
        PathCommand::MoveTo(Point::new(2.0, 3.0))
    );
}

#[test]
fn renders_svg_paths_using_viewbox_scaling() {
    let document = parse_svg(
        r##"
                <svg viewBox="0 0 10 10">
                    <path fill="#ff0000" d="M 0 0 H 10 V 10 H 0 Z"/>
                </svg>
            "##,
    )
    .unwrap();

    let surface = document.render(20, 20, Color::WHITE).unwrap();

    assert_eq!(surface.pixel(0, 0), Some(Color::rgba(255, 0, 0, 255)));
    assert_eq!(surface.pixel(10, 10), Some(Color::rgba(255, 0, 0, 255)));
    assert_eq!(surface.pixel(19, 19), Some(Color::rgba(255, 0, 0, 255)));
}

#[test]
fn inline_style_overrides_presentation_fill() {
    let document = parse_svg(
        r##"
                <svg>
                    <path
                        fill="#ff0000"
                        style="fill: #0000ff"
                        d="M 0 0 H 10 V 10 Z"
                    />
                </svg>
            "##,
    )
    .unwrap();

    assert_eq!(document.shapes[0].fill, Some(Color::rgba(0, 0, 255, 255)));
}

#[test]
fn parses_svg_rectangles() {
    let document = parse_svg(
        r##"
                <svg>
                    <rect x="2" y="3" width="10" height="20" fill="#123456"/>
                </svg>
            "##,
    )
    .unwrap();

    assert_eq!(document.shapes.len(), 1);
    assert_eq!(
        document.shapes[0].fill,
        Some(Color::rgba(0x12, 0x34, 0x56, 255))
    );

    assert_eq!(
        document.shapes[0].path.commands(),
        &[
            PathCommand::MoveTo(Point::new(2.0, 3.0)),
            PathCommand::LineTo(Point::new(12.0, 3.0)),
            PathCommand::LineTo(Point::new(12.0, 23.0)),
            PathCommand::LineTo(Point::new(2.0, 23.0)),
            PathCommand::Close,
        ]
    );
}

#[test]
fn converts_rounded_rectangle_to_cubic_curves() {
    let document = parse_svg(r#"<svg><rect width="20" height="10" rx="3"/></svg>"#).unwrap();

    let commands = document.shapes[0].path.commands();

    assert_eq!(commands.len(), 10);
    assert!(matches!(commands[2], PathCommand::CubicTo { .. }));
    assert!(matches!(commands[4], PathCommand::CubicTo { .. }));
    assert!(matches!(commands[6], PathCommand::CubicTo { .. }));
    assert!(matches!(commands[8], PathCommand::CubicTo { .. }));
    assert_eq!(commands[9], PathCommand::Close);
}

#[test]
fn rounded_rectangle_with_different_radii_has_symmetric_top_corners() {
    let document =
        parse_svg(r#"<svg><rect x="5" y="7" width="40" height="20" rx="8" ry="3"/></svg>"#)
            .unwrap();

    let commands = document.shapes[0].path.commands();

    let (
        PathCommand::CubicTo {
            control_one: top_right,
            ..
        },
        PathCommand::CubicTo {
            control_two: top_left,
            ..
        },
    ) = (&commands[2], &commands[8])
    else {
        panic!("expected cubic curves for the top corners");
    };

    assert_eq!(top_right.y, 7.0);
    assert_eq!(top_left.y, 7.0);

    // The control points should be mirrored across the rectangle's center.
    assert!((top_left.x + top_right.x - 50.0).abs() < 0.0001);
}

#[test]
fn converts_circles_and_ellipses_to_cubic_paths() {
    let document = parse_svg(
        r#"
                <svg>
                    <circle cx="10" cy="20" r="5"/>
                    <ellipse cx="30" cy="40" rx="8" ry="3"/>
                </svg>
            "#,
    )
    .unwrap();

    assert_eq!(document.shapes.len(), 2);

    let circle = document.shapes[0].path.commands();
    assert_eq!(circle.len(), 6);
    assert_eq!(circle[0], PathCommand::MoveTo(Point::new(15.0, 20.0)));
    assert!(matches!(circle[1], PathCommand::CubicTo { .. }));
    assert_eq!(circle[5], PathCommand::Close);

    let ellipse = document.shapes[1].path.commands();
    assert_eq!(ellipse[0], PathCommand::MoveTo(Point::new(38.0, 40.0)));
    assert_eq!(ellipse[5], PathCommand::Close);
}

#[test]
fn renders_elliptically_rounded_rectangle_like_its_path() {
    let rectangle = parse_svg(
        r#"
                <svg width="50" height="40">
                    <rect x="5" y="7" width="40" height="20" rx="8" ry="3"/>
                </svg>
            "#,
    )
    .unwrap();

    let reference = parse_svg(
        r#"
                <svg width="50" height="40">
                    <path d="
                        M 13 7
                        L 37 7
                        C 41.4182784 7 45 8.3431456 45 10
                        L 45 24
                        C 45 25.6568544 41.4182784 27 37 27
                        L 13 27
                        C 8.5817216 27 5 25.6568544 5 24
                        L 5 10
                        C 5 8.3431456 8.5817216 7 13 7
                        Z
                    "/>
                </svg>
            "#,
    )
    .unwrap();

    let actual = rectangle.render(50, 40, Color::WHITE).unwrap();
    let expected = reference.render(50, 40, Color::WHITE).unwrap();

    // Ensure the comparison does not pass because both images are empty.
    assert_eq!(actual.pixel(25, 17), Some(Color::BLACK));
    assert_eq!(expected.pixel(25, 17), Some(Color::BLACK));

    for y in 0..40 {
        for x in 0..50 {
            assert_eq!(
                actual.pixel(x, y),
                expected.pixel(x, y),
                "pixel mismatch at ({x}, {y})",
            );
        }
    }
}

#[test]
fn ignores_zero_radius_ellipses() {
    let document = parse_svg(r#"<svg><circle cx="10" cy="10" r="0"/></svg>"#).unwrap();

    assert!(document.shapes.is_empty());
}

#[test]
fn parses_polygon_and_polyline_points() {
    let document = parse_svg(
        r#"
                <svg>
                    <polygon points="1,2 3,4 5,6"/>
                    <polyline points="10 20, 30 40, 50 60"/>
                </svg>
            "#,
    )
    .unwrap();

    assert_eq!(document.shapes.len(), 2);

    assert_eq!(
        document.shapes[0].path.commands(),
        &[
            PathCommand::MoveTo(Point::new(1.0, 2.0)),
            PathCommand::LineTo(Point::new(3.0, 4.0)),
            PathCommand::LineTo(Point::new(5.0, 6.0)),
            PathCommand::Close,
        ]
    );

    assert_eq!(
        document.shapes[1].path.commands(),
        &[
            PathCommand::MoveTo(Point::new(10.0, 20.0)),
            PathCommand::LineTo(Point::new(30.0, 40.0)),
            PathCommand::LineTo(Point::new(50.0, 60.0)),
        ]
    );
}

#[test]
fn rejects_odd_polygon_coordinate_counts() {
    let error = parse_svg(r#"<svg><polygon points="1 2 3"/></svg>"#).unwrap_err();

    assert_eq!(
        error.message,
        "polygon and polyline points must contain coordinate pairs"
    );
}

#[test]
fn renders_stroked_line_elements() {
    let document = parse_svg(
        r##"
                <svg>
                    <line
                        x1="2"
                        y1="4"
                        x2="8"
                        y2="4"
                        fill="none"
                        stroke="#000000"
                        stroke-width="2"
                    />
                </svg>
            "##,
    )
    .unwrap();

    assert_eq!(document.shapes.len(), 1);
    assert_eq!(document.shapes[0].fill, None);
    assert_eq!(document.shapes[0].stroke, Some(Color::BLACK));
    assert_eq!(document.shapes[0].stroke_width, 2.0);

    let surface = document.render(10, 10, Color::TRANSPARENT).unwrap();

    assert_eq!(surface.pixel(4, 4), Some(Color::BLACK));
    assert_eq!(surface.pixel(0, 4), Some(Color::TRANSPARENT));
}

#[test]
fn skips_elements_that_require_unsupported_extensions() {
    let document = parse_svg(
        r##"
                <svg>
                    <rect width="100" height="100" fill="red" requiredExtensions=""/>
                </svg>
            "##,
    )
    .unwrap();

    assert!(document.shapes.is_empty());
}

#[test]
fn zero_sized_viewbox_disables_rendering() {
    let document = parse_svg(
        r##"
                <svg viewBox="0 0 0 10">
                    <rect width="10" height="10" fill="red"/>
                </svg>
            "##,
    )
    .unwrap();

    let surface = document.render(20, 20, Color::WHITE).unwrap();

    assert_eq!(surface.pixel(10, 10), Some(Color::WHITE));
}

#[test]
fn uses_fallback_color_when_paint_server_is_unavailable() {
    let without_fallback =
        parse_svg(r##"<svg><rect width="10" height="10" fill="url(#missing)"/></svg>"##).unwrap();

    assert!(without_fallback.shapes.is_empty());

    let with_fallback =
        parse_svg(r##"<svg><rect width="10" height="10" fill="url(#missing) green"/></svg>"##)
            .unwrap();

    assert_eq!(with_fallback.shapes.len(), 1);
    assert_eq!(
        with_fallback.shapes[0].fill,
        Some(Color::rgba(0, 128, 0, 255))
    );
}

#[test]
fn parses_preserve_aspect_ratio_values() {
    assert_eq!(
        parse_preserve_aspect_ratio("xMaxYMin slice").unwrap(),
        PreserveAspectRatio::Align {
            horizontal: ViewBoxAlignment::Max,
            vertical: ViewBoxAlignment::Min,
            meet_or_slice: MeetOrSlice::Slice,
        }
    );

    assert_eq!(
        parse_preserve_aspect_ratio("none").unwrap(),
        PreserveAspectRatio::None
    );
}

#[test]
fn renders_non_uniform_viewbox_with_preserve_aspect_ratio_none() {
    let document = parse_svg(
        r##"
                <svg viewBox="0 0 10 10" preserveAspectRatio="none">
                    <rect width="10" height="10" fill="red"/>
                </svg>
            "##,
    )
    .unwrap();

    let surface = document.render(20, 10, Color::WHITE).unwrap();

    assert_eq!(surface.pixel(19, 9), Some(Color::rgba(255, 0, 0, 255)));
}

#[test]
fn aligns_meet_viewbox_to_maximum_edges() {
    let transform = view_box_transform(
        ViewBox {
            min_x: 0.0,
            min_y: 0.0,
            width: 10.0,
            height: 10.0,
        },
        PreserveAspectRatio::Align {
            horizontal: ViewBoxAlignment::Max,
            vertical: ViewBoxAlignment::Max,
            meet_or_slice: MeetOrSlice::Meet,
        },
        20.0,
        10.0,
    );

    assert_eq!(
        transform.apply_point(Point::new(0.0, 0.0)),
        Point::new(10.0, 0.0)
    );
}

#[test]
fn expands_local_use_references() {
    let document = parse_svg(
        r##"
                <svg>
                    <defs>
                        <rect id="tile" width="4" height="5" fill="green"/>
                    </defs>
                    <use href="#tile" x="2" y="3"/>
                </svg>
            "##,
    )
    .unwrap();

    assert_eq!(document.shapes.len(), 1);
    assert_eq!(
        document.shapes[0].path.commands()[0],
        PathCommand::MoveTo(Point::new(2.0, 3.0))
    );
    assert_eq!(document.shapes[0].fill, Some(Color::rgba(0, 128, 0, 255)));
}

#[test]
fn maps_nested_svg_viewboxes_into_their_viewports() {
    let document = parse_svg(
        r##"
                <svg>
                    <svg
                        x="10"
                        y="20"
                        width="20"
                        height="10"
                        viewBox="0 0 10 10"
                        preserveAspectRatio="none"
                    >
                        <rect width="10" height="10" fill="green"/>
                    </svg>
                </svg>
            "##,
    )
    .unwrap();

    let surface = document.render(50, 50, Color::WHITE).unwrap();

    assert_eq!(surface.pixel(25, 25), Some(Color::rgba(0, 128, 0, 255)));
}

#[test]
fn clips_nested_svg_to_its_viewport() {
    let document = parse_svg(
        r##"
                <svg>
                    <svg
                        x="10"
                        y="20"
                        width="20"
                        height="10"
                        viewBox="0 0 10 10"
                        preserveAspectRatio="none"
                    >
                        <rect width="20" height="10" fill="red"/>
                    </svg>
                </svg>
            "##,
    )
    .unwrap();

    let surface = document.render(50, 50, Color::WHITE).unwrap();

    assert_eq!(surface.pixel(25, 25), Some(Color::rgba(255, 0, 0, 255)));
    assert_eq!(surface.pixel(35, 25), Some(Color::WHITE));
}

#[test]
fn applies_fill_and_stroke_opacity() {
    let document = parse_svg(
        r##"
                <svg>
                    <rect
                        width="10"
                        height="10"
                        fill="red"
                        fill-opacity="0.5"
                        stroke="green"
                        stroke-opacity="0.25"
                    />
                </svg>
            "##,
    )
    .unwrap();

    assert_eq!(document.shapes[0].fill, Some(Color::rgba(255, 0, 0, 128)));

    assert_eq!(document.shapes[0].stroke, Some(Color::rgba(0, 128, 0, 64)));

    let surface = document.render(12, 12, Color::WHITE).unwrap();

    assert_eq!(surface.pixel(5, 5), Some(Color::rgba(255, 127, 127, 255)));
}

#[test]
fn decodes_percent_encoded_use_fragment_identifiers() {
    let document = parse_svg(
        r##"
                <svg>
                    <defs>
                        <rect id="tile one" width="4" height="5" fill="green"/>
                    </defs>
                    <use href="#tile%20one"/>
                </svg>
            "##,
    )
    .unwrap();

    assert_eq!(document.shapes.len(), 1);
    assert_eq!(document.shapes[0].fill, Some(Color::rgba(0, 128, 0, 255)));
}

#[test]
fn selects_the_first_supported_switch_child() {
    let document = parse_svg(
        r##"
                <svg>
                    <switch>
                        <rect width="10" height="10" fill="red"/>
                        <rect width="10" height="10" fill="green"/>
                    </switch>
                </svg>
            "##,
    )
    .unwrap();

    assert_eq!(document.shapes.len(), 1);
    assert_eq!(document.shapes[0].fill, Some(Color::rgba(255, 0, 0, 255)));
}

#[test]
fn skips_unsupported_switch_children() {
    let document = parse_svg(
        r##"
                <svg>
                    <switch>
                        <rect
                            width="10"
                            height="10"
                            fill="red"
                            requiredExtensions="https://example.invalid/extension"
                        />
                        <rect width="10" height="10" fill="green"/>
                    </switch>
                </svg>
            "##,
    )
    .unwrap();

    assert_eq!(document.shapes.len(), 1);
    assert_eq!(document.shapes[0].fill, Some(Color::rgba(0, 128, 0, 255)));
}

#[test]
fn applies_local_clip_paths() {
    let document = parse_svg(
        r##"
                <svg>
                    <clipPath id="center">
                        <rect x="2" y="2" width="4" height="4"/>
                    </clipPath>
                    <rect width="8" height="8" fill="red" clip-path="url(#center)"/>
                </svg>
            "##,
    )
    .unwrap();

    let surface = document.render(8, 8, Color::WHITE).unwrap();

    assert_eq!(surface.pixel(3, 3), Some(Color::rgba(255, 0, 0, 255)));
    assert_eq!(surface.pixel(1, 1), Some(Color::WHITE));
}

#[test]
fn resolves_double_encoded_use_identifiers_through_document_url() {
    let html = crate::html::Document::parse(
        r##"
                <svg>
                    <defs>
                        <rect id="tile%20one" width="4" height="5" fill="green"/>
                    </defs>
                    <use href="#tile%2520one"/>
                </svg>
            "##,
    );

    let root = html.first_element("svg").unwrap();
    let base_url = Url::parse("https://example.test/assets/scene.svg").unwrap();

    let document = parse_svg_element_at(root, &base_url).unwrap();

    assert_eq!(document.shapes.len(), 1);
    assert_eq!(document.shapes[0].fill, Some(Color::rgba(0, 128, 0, 255)));
}

#[test]
fn parses_stroke_join_properties() {
    let document = parse_svg(
        r#"
                <svg>
                    <path
                        d="M 2 6 L 5 2 L 8 6"
                        fill="none"
                        stroke="black"
                        stroke-linejoin="round"
                        style="stroke-miterlimit: 7"
                    />
                </svg>
            "#,
    )
    .unwrap();

    assert_eq!(document.shapes.len(), 1);
    assert_eq!(document.shapes[0].stroke_line_join, StrokeLineJoin::Round);
    assert_eq!(document.shapes[0].stroke_miter_limit, 7.0);
}

#[test]
fn applies_embedded_stylesheet_rules() {
    let document = parse_svg(
        r#"
                <svg>
                    <style>
                        line { stroke: red; stroke-width: 2; }
                        #target { stroke: blue; stroke-width: 4px; }
                    </style>
                    <line id="target" x1="1" y1="1" x2="9" y2="1"/>
                </svg>
            "#,
    )
    .unwrap();

    assert_eq!(document.shapes.len(), 1);
    assert_eq!(document.shapes[0].stroke, Some(Color::rgba(0, 0, 255, 255)));
    assert_eq!(document.shapes[0].stroke_width, 4.0);
}

#[test]
fn parses_dash_properties_and_path_length() {
    let document = parse_svg(
        r#"
                <svg>
                    <path
                        d="M 0 0 H 100"
                        fill="none"
                        stroke="blue"
                        pathLength="25"
                        style="stroke-dasharray: 3 1; stroke-dashoffset: 2"
                    />
                </svg>
            "#,
    )
    .unwrap();

    assert_eq!(document.shapes.len(), 1);
    assert_eq!(document.shapes[0].stroke_dash_array, vec![3.0, 1.0]);
    assert_eq!(document.shapes[0].stroke_dash_offset, 2.0);
    assert_eq!(document.shapes[0].path_length, Some(25.0));
}

#[test]
fn renders_user_space_linear_gradients() {
    let document = parse_svg(
        r#"
                <svg width="10" height="2">
                    <defs>
                        <linearGradient
                            id="sky"
                            gradientUnits="userSpaceOnUse"
                            x1="0"
                            y1="0"
                            x2="10"
                            y2="0"
                        >
                            <stop offset="0" stop-color="red"/>
                            <stop offset="1" style="stop-color: blue"/>
                        </linearGradient>
                    </defs>
                    <rect width="10" height="2" fill="url(#sky)"/>
                </svg>
            "#,
    )
    .unwrap();

    let surface = document.render(10, 2, Color::TRANSPARENT).unwrap();

    let left = surface.pixel(1, 1).unwrap();
    let right = surface.pixel(8, 1).unwrap();

    assert!(left.red > left.blue);
    assert!(right.blue > right.red);
}

#[test]
fn renders_object_bounding_box_linear_gradients() {
    let document = parse_svg(
        r#"
                <svg width="10" height="2">
                    <defs>
                        <linearGradient id="sky">
                            <stop offset="0" stop-color="red"/>
                            <stop offset="1" stop-color="blue"/>
                        </linearGradient>
                    </defs>
                    <rect x="2" y="0" width="6" height="2" fill="url(#sky)"/>
                </svg>
            "#,
    )
    .unwrap();

    let surface = document.render(10, 2, Color::TRANSPARENT).unwrap();

    assert_eq!(surface.pixel(1, 1), Some(Color::TRANSPARENT));

    let left = surface.pixel(3, 1).unwrap();
    let right = surface.pixel(7, 1).unwrap();

    assert!(left.red > left.blue);
    assert!(right.blue > right.red);
}

#[test]
fn inherits_linear_gradient_definitions() {
    let document = parse_svg(
        r##"
                <svg width="10" height="2">
                    <defs>
                        <linearGradient id="base" x1="0" y1="0" x2="1" y2="0">
                            <stop offset="0" stop-color="red"/>
                            <stop offset="1" stop-color="blue"/>
                        </linearGradient>
                        <linearGradient id="derived" href="#base"/>
                    </defs>
                    <rect width="10" height="2" fill="url(#derived)"/>
                </svg>
            "##,
    )
    .unwrap();

    let surface = document.render(10, 2, Color::TRANSPARENT).unwrap();

    let left = surface.pixel(1, 1).unwrap();
    let right = surface.pixel(8, 1).unwrap();

    assert!(left.red > left.blue);
    assert!(right.blue > right.red);
}

#[test]
fn inherits_gradient_spread_method() {
    let document = parse_svg(
        r##"
                <svg width="12" height="4">
                    <defs>
                        <linearGradient
                            id="base"
                            gradientUnits="userSpaceOnUse"
                            x1="0"
                            y1="0"
                            x2="4"
                            y2="0"
                            spreadMethod="repeat"
                        >
                            <stop offset="0" stop-color="red"/>
                            <stop offset="1" stop-color="blue"/>
                        </linearGradient>
                        <linearGradient id="reflected" href="#base" spreadMethod="reflect"/>
                    </defs>

                    <rect width="12" height="2" fill="url(#base)"/>
                    <rect y="2" width="12" height="2" fill="url(#reflected)"/>
                </svg>
            "##,
    )
    .unwrap();

    let surface = document.render(12, 4, Color::TRANSPARENT).unwrap();

    let repeated = surface.pixel(5, 1).unwrap();
    let reflected = surface.pixel(5, 3).unwrap();

    assert!(repeated.red > repeated.blue);
    assert!(reflected.blue > reflected.red);
}

#[test]
fn stores_linear_gradients_as_fill_paints() {
    let document = parse_svg(
        r##"
                <svg>
                    <defs>
                        <linearGradient id="sky">
                            <stop offset="0" stop-color="red"/>
                            <stop offset="1" stop-color="blue"/>
                        </linearGradient>
                    </defs>
                    <rect width="10" height="10" fill="url(#sky)"/>
                </svg>
            "##,
    )
    .unwrap();

    assert!(matches!(
        &document.shapes[0].fill_paint,
        Some(Paint::LinearGradient(_))
    ));
}

#[test]
fn parses_svg_numbers_in_attributes_paths_and_transforms() {
    let numbers = parse_number_list("1e1-2.5e+1,.5").unwrap();
    assert_eq!(numbers, vec![10.0, -25.0, 0.5]);

    let path = parse_path_data("M1e1-2.5e+1L.5,3").unwrap();
    assert_eq!(
        path.commands(),
        &[
            PathCommand::MoveTo(Point::new(10.0, -25.0)),
            PathCommand::LineTo(Point::new(0.5, 3.0)),
        ],
    );

    let transform = parse_transform_list("translate(1e1,-2.5e+1)").unwrap();
    assert_eq!(
        transform.apply_point(Point::new(0.0, 0.0)),
        Point::new(10.0, -25.0),
    );
}

#[test]
fn reports_invalid_svg_number_positions_in_paths_and_transforms() {
    let path_error = parse_path_data("M1e 2").unwrap_err();
    assert_eq!(path_error.offset, 3);
    assert_eq!(path_error.message, "expected exponent digits");

    let transform_error = parse_transform_list("translate(1e 2)").unwrap_err();
    assert_eq!(transform_error.offset, 12);
    assert_eq!(transform_error.message, "expected exponent digits");

    assert!(parse_number_list("1e999").is_err());

    let path_error = parse_path_data("M1e999 0").unwrap_err();
    assert_eq!(path_error.offset, 1);
    assert_eq!(path_error.message, "number must be finite");

    let transform_error = parse_transform_list("translate(1e999,0)").unwrap_err();
    assert_eq!(transform_error.offset, 10);
    assert_eq!(transform_error.message, "number must be finite");
}

#[test]
fn semicolon_inside_css_string_does_not_create_a_fill_declaration() {
    let document = parse_svg(
        r#"
            <svg>
                <rect
                    width="10"
                    height="10"
                    fill="red"
                    style="--x: 'a; fill: blue; b'"
                />
            </svg>
        "#,
    )
    .unwrap();

    assert_eq!(document.shapes[0].fill, Some(Color::rgba(255, 0, 0, 255)));
}
