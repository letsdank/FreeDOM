//! Reconstruction of TrueType simple-glyph records.
//!
//! Specification:
//! <https://learn.microsoft.com/en-us/typography/opentype/spec/glyf>

use super::super::super::glyf::GlyphBounds;
use super::{FontError, SimpleOutline, error};

const ON_CURVE_POINT: u8 = 0x01;
const OVERLAP_SIMPLE: u8 = 0x40;

/// Serializes a simple outline as a TrueType glyph record.
///
/// This initial implementation writes every coordinate delta as a signed
/// 16-bit value and does not compress repeated flags.
pub(super) fn simple_glyph(
    outline: &SimpleOutline,
    bounds: GlyphBounds,
    instructions: &[u8],
    overlap: bool,
) -> Result<Vec<u8>, FontError> {
    let contour_count = i16::try_from(outline.contour_ends.len())
        .map_err(|_| error(0, "too many glyph contours"))?;

    if contour_count == 0 || outline.points.is_empty() {
        return Err(error(0, "simple glyph must have contours and points"));
    }

    let mut previous_end = None;

    for &end in &outline.contour_ends {
        if previous_end.is_some_and(|previous| end <= previous) {
            return Err(error(0, "invalid glyph contour endpoints"));
        }

        previous_end = Some(end);
    }

    let last_end = previous_end.expect("contour count was checked");

    if usize::from(last_end) + 1 != outline.points.len() {
        return Err(error(0, "invalid glyph contour endpoints"));
    }

    let instruction_length = u16::try_from(instructions.len())
        .map_err(|_| error(0, "glyph instructions are too long"))?;

    let mut flags = Vec::with_capacity(outline.points.len());
    let mut x_coordinates = Vec::with_capacity(outline.points.len() * 2);
    let mut y_coordinates = Vec::with_capacity(outline.points.len() * 2);

    let mut previous_x = 0_i32;
    let mut previous_y = 0_i32;

    for (index, point) in outline.points.iter().enumerate() {
        let dx = point
            .x
            .checked_sub(previous_x)
            .and_then(|delta| i16::try_from(delta).ok())
            .ok_or_else(|| error(index, "glyph x delta is out of range"))?;

        let dy = point
            .y
            .checked_sub(previous_y)
            .and_then(|delta| i16::try_from(delta).ok())
            .ok_or_else(|| error(index, "glyph y delta is out of range"))?;

        let mut flag = if point.on_curve { ON_CURVE_POINT } else { 0 };

        if index == 0 && overlap {
            flag |= OVERLAP_SIMPLE;
        }

        flags.push(flag);

        x_coordinates.extend_from_slice(&dx.to_be_bytes());
        y_coordinates.extend_from_slice(&dy.to_be_bytes());

        previous_x = point.x;
        previous_y = point.y;
    }

    let mut result = Vec::new();

    // Glyph header.
    result.extend_from_slice(&contour_count.to_be_bytes());

    for coordinate in [bounds.x_min, bounds.y_min, bounds.x_max, bounds.y_max] {
        result.extend_from_slice(&coordinate.to_be_bytes());
    }

    // End point indexes and glyph instructions.
    for end in &outline.contour_ends {
        result.extend_from_slice(&end.to_be_bytes());
    }

    result.extend_from_slice(&instruction_length.to_be_bytes());
    result.extend_from_slice(instructions);

    // TrueType stores flags, all X deltas, then all Y deltas.
    result.extend_from_slice(&flags);
    result.extend_from_slice(&x_coordinates);
    result.extend_from_slice(&y_coordinates);

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::super::OutlinePoint;
    use super::*;

    use crate::graphics::{
        font::{
            glyf::{Glyph, parse_glyph},
            reader::{write_u16, write_u32},
        },
        geometry::{PathCommand, Point},
    };

    #[test]
    fn reconstructed_glyph_is_read_by_truetype_parser() {
        let outline = SimpleOutline {
            contour_ends: vec![2],
            points: vec![
                OutlinePoint {
                    x: 0,
                    y: 0,
                    on_curve: true,
                },
                OutlinePoint {
                    x: 50,
                    y: 0,
                    on_curve: true,
                },
                OutlinePoint {
                    x: 25,
                    y: 80,
                    on_curve: true,
                },
            ],
        };

        let bounds = GlyphBounds {
            x_min: 0,
            y_min: 0,
            x_max: 50,
            y_max: 80,
        };

        let bytes = outline.to_truetype(bounds, &[0xaa, 0xbb], true).unwrap();

        assert_eq!(&bytes[12..14], &[0, 2]);
        assert_eq!(&bytes[14..16], &[0xaa, 0xbb]);
        assert_eq!(bytes[16] & 0x40, 0x40);

        let Glyph::Simple(glyph) = parse_glyph(&bytes, 0).unwrap() else {
            panic!("expected a simple glyph");
        };

        assert_eq!(glyph.bounds, bounds);

        assert_eq!(
            glyph.path.commands(),
            &[
                PathCommand::MoveTo(Point::new(0.0, 0.0)),
                PathCommand::LineTo(Point::new(50.0, 0.0)),
                PathCommand::LineTo(Point::new(25.0, 80.0)),
                PathCommand::Close,
            ]
        );
    }

    #[test]
    fn decoded_woff2_outline_round_trips_through_truetype() {
        let mut data = vec![0_u8; 36];

        write_u16(&mut data, 4, 1).unwrap();

        let sizes = [2_u32, 1, 3, 4, 0, 4, 0];

        for (index, size) in sizes.into_iter().enumerate() {
            write_u32(&mut data, 8 + index * 4, size).unwrap();
        }

        data.extend_from_slice(&[
            0, 1, // one contour
            3, // three points
            1, 11, 86, // point flags
            0, 50, 24, 79, // coordinate data
            0, 0, 0, 0, // bbox bitmap
        ]);

        let streams = super::super::parse(&data).unwrap();
        let mut cursors = super::super::OutlineCursors::default();

        let outline = streams.read_simple_outline(1, &mut cursors).unwrap();

        let bounds = GlyphBounds {
            x_min: 0,
            y_min: 0,
            x_max: 50,
            y_max: 80,
        };

        let bytes = outline.to_truetype(bounds, &[], false).unwrap();

        let Glyph::Simple(glyph) = parse_glyph(&bytes, 0).unwrap() else {
            panic!("expected a simple glyph");
        };

        assert_eq!(glyph.bounds, bounds);

        assert_eq!(
            glyph.path.commands(),
            &[
                PathCommand::MoveTo(Point::new(0.0, 0.0)),
                PathCommand::LineTo(Point::new(50.0, 0.0)),
                PathCommand::LineTo(Point::new(25.0, 80.0)),
                PathCommand::Close,
            ]
        );
    }

    #[test]
    fn rejects_invalid_outline_and_out_of_range_delta() {
        let bounds = GlyphBounds {
            x_min: 0,
            y_min: 0,
            x_max: 0,
            y_max: 0,
        };

        let out_of_range = SimpleOutline {
            contour_ends: vec![0],
            points: vec![OutlinePoint {
                x: 32768,
                y: 0,
                on_curve: true,
            }],
        };

        assert_eq!(
            out_of_range
                .to_truetype(bounds, &[], false)
                .unwrap_err()
                .message,
            "glyph x delta is out of range"
        );

        let wrong_last_end = SimpleOutline {
            contour_ends: vec![1],
            points: vec![OutlinePoint {
                x: 0,
                y: 0,
                on_curve: true,
            }],
        };

        assert_eq!(
            wrong_last_end
                .to_truetype(bounds, &[], false)
                .unwrap_err()
                .message,
            "invalid glyph contour endpoints"
        );

        let repeated_end = SimpleOutline {
            contour_ends: vec![0, 0],
            points: vec![OutlinePoint {
                x: 0,
                y: 0,
                on_curve: true,
            }],
        };

        assert_eq!(
            repeated_end
                .to_truetype(bounds, &[], false)
                .unwrap_err()
                .message,
            "invalid glyph contour endpoints"
        );

        let valid = SimpleOutline {
            contour_ends: vec![0],
            points: vec![OutlinePoint {
                x: 0,
                y: 0,
                on_curve: true,
            }],
        };

        assert_eq!(
            valid
                .to_truetype(bounds, &vec![0; 65536], false)
                .unwrap_err()
                .message,
            "glyph instructions are too long"
        );
    }
}
