use crate::graphics::geometry::{Path, Point};

use super::sfnt::FontError;

const ON_CURVE_POINT: u8 = 0x01;
const X_SHORT_VECTOR: u8 = 0x02;
const Y_SHORT_VECTOR: u8 = 0x04;
const REPEAT_FLAG: u8 = 0x08;
const X_IS_SAME_OR_POSITIVE: u8 = 0x10;
const Y_IS_SAME_OR_POSITIVE: u8 = 0x20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlyphBounds {
    pub x_min: i16,
    pub y_min: i16,
    pub x_max: i16,
    pub y_max: i16,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SimpleGlyph {
    pub bounds: GlyphBounds,
    pub path: Path,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Glyph {
    Empty,
    Simple(SimpleGlyph),
}

#[derive(Clone, Copy, Debug)]
struct GlyphPoint {
    position: Point,
    on_curve: bool,
}

pub fn parse_glyph(data: &[u8], file_offset: usize) -> Result<Glyph, FontError> {
    if data.is_empty() {
        return Ok(Glyph::Empty);
    }

    if data.len() < 10 {
        return Err(error(file_offset, "truncated glyph header"));
    }

    let number_of_contours = read_i16(data, 0, file_offset)?;

    if number_of_contours < 0 {
        return Err(error(
            file_offset,
            "compound TrueType glyphs are not supported yet",
        ));
    }

    let bounds = GlyphBounds {
        x_min: read_i16(data, 2, file_offset)?,
        y_min: read_i16(data, 4, file_offset)?,
        x_max: read_i16(data, 6, file_offset)?,
        y_max: read_i16(data, 8, file_offset)?,
    };

    let contour_count = usize::try_from(number_of_contours)
        .map_err(|_| error(file_offset, "invalid simple glyph contour count"))?;

    let mut cursor = 10;
    let mut contour_end_points = Vec::with_capacity(contour_count);

    for index in 0..contour_count {
        let end_point = usize::from(read_u16(data, cursor, file_offset)?);
        cursor += 2;

        if let Some(previous) = contour_end_points.last() {
            if end_point <= *previous {
                return Err(error(
                    file_offset + cursor - 2,
                    "glyph contour end points are not strictly increasing",
                ));
            }
        }

        contour_end_points.push(end_point);

        if index + 1 == contour_count && end_point == usize::MAX {
            return Err(error(file_offset + cursor - 2, "glyph has too many points"));
        }
    }

    let point_count = contour_end_points
        .last()
        .map(|end_point| end_point + 1)
        .unwrap_or(0);

    let instruction_length = usize::from(read_u16(data, cursor, file_offset)?);
    cursor += 2;

    read_bytes(data, cursor, instruction_length, file_offset)?;
    cursor += instruction_length;

    let mut flags = Vec::with_capacity(point_count);

    while flags.len() < point_count {
        let flag = read_byte(data, cursor, file_offset)?;
        cursor += 1;

        let repetitions = if flag & REPEAT_FLAG != 0 {
            let additional_count = usize::from(read_byte(data, cursor, file_offset)?);
            cursor += 1;
            additional_count + 1
        } else {
            1
        };

        if flags.len() + repetitions > point_count {
            return Err(error(
                file_offset + cursor - 1,
                "glyph flag repetition exceeds its point count",
            ));
        }

        flags.extend(std::iter::repeat_n(flag, repetitions));
    }

    let mut x = 0_i32;
    let mut x_coordinates = Vec::with_capacity(point_count);

    for &flag in &flags {
        x += read_coordinate_delta(
            data,
            &mut cursor,
            file_offset,
            flag,
            X_SHORT_VECTOR,
            X_IS_SAME_OR_POSITIVE,
        )?;

        x_coordinates.push(x);
    }

    let mut y = 0_i32;
    let mut points = Vec::with_capacity(point_count);

    for (index, &flag) in flags.iter().enumerate() {
        y += read_coordinate_delta(
            data,
            &mut cursor,
            file_offset,
            flag,
            Y_SHORT_VECTOR,
            Y_IS_SAME_OR_POSITIVE,
        )?;

        points.push(GlyphPoint {
            position: Point::new(x_coordinates[index] as f32, y as f32),
            on_curve: flag & ON_CURVE_POINT != 0,
        });
    }

    let mut path = Path::new();
    let mut contour_start = 0;

    for contour_end in contour_end_points {
        append_contour(&mut path, &points[contour_start..=contour_end]);
        contour_start = contour_end + 1;
    }

    Ok(Glyph::Simple(SimpleGlyph { bounds, path }))
}

fn append_contour(path: &mut Path, points: &[GlyphPoint]) {
    let Some(first) = points.first() else {
        return;
    };
    let last = points.last().expect("non-empty slice has a last point");

    let (start, start_index, points_to_process) = if first.on_curve {
        (first.position, 1, points.len() - 1)
    } else if last.on_curve {
        (last.position, 0, points.len() - 1)
    } else {
        (midpoint(last.position, first.position), 0, points.len())
    };

    path.move_to(start);

    let mut previous_off_curve = None;

    for step in 0..points_to_process {
        let point = points[(start_index + step) % points.len()];

        if point.on_curve {
            if let Some(control) = previous_off_curve.take() {
                path.quadratic_to(control, point.position);
            } else {
                path.line_to(point.position);
            }
        } else if let Some(previous) = previous_off_curve {
            path.quadratic_to(previous, midpoint(previous, point.position));
            previous_off_curve = Some(point.position);
        } else {
            previous_off_curve = Some(point.position);
        }
    }

    if let Some(control) = previous_off_curve {
        path.quadratic_to(control, start);
    }

    path.close();
}

fn midpoint(left: Point, right: Point) -> Point {
    Point::new((left.x + right.x) * 0.5, (left.y + right.y) * 0.5)
}

fn read_coordinate_delta(
    data: &[u8],
    cursor: &mut usize,
    file_offset: usize,
    flag: u8,
    short_vector_flag: u8,
    same_or_positive_flag: u8,
) -> Result<i32, FontError> {
    if flag & short_vector_flag != 0 {
        let magnitude = i32::from(read_byte(data, *cursor, file_offset)?);
        *cursor += 1;

        return if flag & same_or_positive_flag != 0 {
            Ok(magnitude)
        } else {
            Ok(-magnitude)
        };
    }

    if flag & same_or_positive_flag != 0 {
        return Ok(0);
    }

    let delta = i32::from(read_i16(data, *cursor, file_offset)?);
    *cursor += 2;

    Ok(delta)
}

fn read_byte(data: &[u8], offset: usize, file_offset: usize) -> Result<u8, FontError> {
    data.get(offset)
        .copied()
        .ok_or_else(|| error(file_offset + offset, "truncated glyph data"))
}

fn read_u16(data: &[u8], offset: usize, file_offset: usize) -> Result<u16, FontError> {
    let bytes = read_bytes(data, offset, 2, file_offset)?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_i16(data: &[u8], offset: usize, file_offset: usize) -> Result<i16, FontError> {
    let bytes = read_bytes(data, offset, 2, file_offset)?;
    Ok(i16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_bytes<'a>(
    data: &'a [u8],
    offset: usize,
    length: usize,
    file_offset: usize,
) -> Result<&'a [u8], FontError> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| error(file_offset + length, "glyph offset overflows"))?;

    data.get(offset..end)
        .ok_or_else(|| error(file_offset + offset, "truncated glyph data"))
}

fn error(offset: usize, message: impl Into<String>) -> FontError {
    FontError {
        offset,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use crate::graphics::geometry::PathCommand;

    use super::*;

    #[test]
    fn decodes_a_simple_triangle_glyph() {
        let mut data = vec![0_u8; 20];

        data[0..2].copy_from_slice(&1_i16.to_be_bytes());
        data[10..12].copy_from_slice(&2_u16.to_be_bytes());

        data[14] = ON_CURVE_POINT | X_IS_SAME_OR_POSITIVE | Y_IS_SAME_OR_POSITIVE;
        data[15] = ON_CURVE_POINT | X_SHORT_VECTOR | X_IS_SAME_OR_POSITIVE | Y_IS_SAME_OR_POSITIVE;
        data[16] = ON_CURVE_POINT | X_SHORT_VECTOR | Y_SHORT_VECTOR | Y_IS_SAME_OR_POSITIVE;

        data[17] = 50;
        data[18] = 25;
        data[19] = 80;

        let Glyph::Simple(glyph) = parse_glyph(&data, 100).unwrap() else {
            panic!("expected a simple glyph");
        };

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
    fn inserts_quadratic_segments_for_off_curve_points() {
        let mut data = vec![0_u8; 29];

        data[0..2].copy_from_slice(&1_i16.to_be_bytes());
        data[10..12].copy_from_slice(&2_u16.to_be_bytes());

        data[14] = ON_CURVE_POINT;
        data[15] = 0;
        data[16] = ON_CURVE_POINT;

        data[19..21].copy_from_slice(&50_i16.to_be_bytes());
        data[21..23].copy_from_slice(&50_i16.to_be_bytes());

        data[25..27].copy_from_slice(&100_i16.to_be_bytes());
        data[27..29].copy_from_slice(&(-100_i16).to_be_bytes());

        let Glyph::Simple(glyph) = parse_glyph(&data, 0).unwrap() else {
            panic!("expected a simple glyph");
        };

        assert_eq!(
            glyph.path.commands(),
            &[
                PathCommand::MoveTo(Point::new(0.0, 0.0)),
                PathCommand::QuadraticTo {
                    control: Point::new(50.0, 100.0),
                    to: Point::new(100.0, 0.0),
                },
                PathCommand::Close,
            ]
        );
    }

    #[test]
    fn reports_compound_glyphs_as_unsupported_for_now() {
        let mut data = vec![0_u8; 10];
        data[0..2].copy_from_slice(&(-1_i16).to_be_bytes());

        let error = parse_glyph(&data, 40).unwrap_err();

        assert_eq!(
            error.message,
            "compound TrueType glyphs are not supported yet"
        );
        assert_eq!(error.offset, 40);
    }
}
