use crate::graphics::geometry::{Path, Point, Transform};

use super::{
    reader::{read_bytes, read_i16, read_u16},
    sfnt::FontError,
};

const ON_CURVE_POINT: u8 = 0x01;
const X_SHORT_VECTOR: u8 = 0x02;
const Y_SHORT_VECTOR: u8 = 0x04;
const REPEAT_FLAG: u8 = 0x08;
const X_IS_SAME_OR_POSITIVE: u8 = 0x10;
const Y_IS_SAME_OR_POSITIVE: u8 = 0x20;

const ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
const ARGS_ARE_XY_VALUES: u16 = 0x0002;
const WE_HAVE_A_SCALE: u16 = 0x0008;
const MORE_COMPONENTS: u16 = 0x0020;
const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 0x0040;
const WE_HAVE_A_TWO_BY_TWO: u16 = 0x0080;
const WE_HAVE_INSTRUCTIONS: u16 = 0x0100;
const SCALED_COMPONENT_OFFSET: u16 = 0x0800;
const UNSCALED_COMPONENT_OFFSET: u16 = 0x1000;

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
pub struct CompoundGlyph {
    pub bounds: GlyphBounds,
    pub components: Vec<GlyphComponent>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphComponent {
    pub glyph_id: u16,
    pub transform: Transform,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Glyph {
    Empty,
    Simple(SimpleGlyph),
    Compound(CompoundGlyph),
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

    let bounds = GlyphBounds {
        x_min: read_i16(data, 2, file_offset)?,
        y_min: read_i16(data, 4, file_offset)?,
        x_max: read_i16(data, 6, file_offset)?,
        y_max: read_i16(data, 8, file_offset)?,
    };

    if number_of_contours < 0 {
        return parse_compound_glyph(data, file_offset, bounds);
    }

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

fn parse_compound_glyph(
    data: &[u8],
    file_offset: usize,
    bounds: GlyphBounds,
) -> Result<Glyph, FontError> {
    let mut cursor = 10;
    let mut components = Vec::new();
    let mut has_instructions = false;

    loop {
        let flags = read_u16(data, cursor, file_offset)?;
        has_instructions |= flags & WE_HAVE_INSTRUCTIONS != 0;
        cursor += 2;

        let glyph_id = read_u16(data, cursor, file_offset)?;
        cursor += 2;

        if flags & ARGS_ARE_XY_VALUES == 0 {
            return Err(error(
                file_offset + cursor - 4,
                "point-matched compound glyph components are not supported yet",
            ));
        }

        let (x_offset, y_offset) = if flags & ARG_1_AND_2_ARE_WORDS != 0 {
            let x = f32::from(read_i16(data, cursor, file_offset)?);
            let y = f32::from(read_i16(data, cursor + 2, file_offset)?);
            cursor += 4;
            (x, y)
        } else {
            let x = f32::from(read_byte(data, cursor, file_offset)? as i8);
            let y = f32::from(read_byte(data, cursor + 1, file_offset)? as i8);
            cursor += 2;
            (x, y)
        };

        let transform_flags =
            flags & (WE_HAVE_A_SCALE | WE_HAVE_AN_X_AND_Y_SCALE | WE_HAVE_A_TWO_BY_TWO);

        if transform_flags.count_ones() > 1 {
            return Err(error(
                file_offset + cursor,
                "compound glyph has incompatible transform flags",
            ));
        }

        let (a, b, c, d) = if flags & WE_HAVE_A_SCALE != 0 {
            let scale = read_f2dot14(data, cursor, file_offset)?;
            cursor += 2;
            (scale, 0.0, 0.0, scale)
        } else if flags & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
            let x_scale = read_f2dot14(data, cursor, file_offset)?;
            let y_scale = read_f2dot14(data, cursor + 2, file_offset)?;
            cursor += 4;
            (x_scale, 0.0, 0.0, y_scale)
        } else if flags & WE_HAVE_A_TWO_BY_TWO != 0 {
            let a = read_f2dot14(data, cursor, file_offset)?;
            let b = read_f2dot14(data, cursor + 2, file_offset)?;
            let c = read_f2dot14(data, cursor + 4, file_offset)?;
            let d = read_f2dot14(data, cursor + 6, file_offset)?;
            cursor += 8;
            (a, b, c, d)
        } else {
            (1.0, 0.0, 0.0, 1.0)
        };

        if flags & SCALED_COMPONENT_OFFSET != 0 && flags & UNSCALED_COMPONENT_OFFSET != 0 {
            return Err(error(
                file_offset + cursor,
                "compound glyph has conflicting offset flags",
            ));
        }

        let (e, f) = if flags & SCALED_COMPONENT_OFFSET != 0 {
            (a * x_offset + c * y_offset, b * x_offset + d * y_offset)
        } else {
            (x_offset, y_offset)
        };

        components.push(GlyphComponent {
            glyph_id,
            transform: Transform { a, b, c, d, e, f },
        });

        if components.len() > 4096 {
            return Err(error(
                file_offset + cursor,
                "compound glyph has too many components",
            ));
        }

        if flags & MORE_COMPONENTS == 0 {
            break;
        }
    }

    if has_instructions {
        let instruction_length = usize::from(read_u16(data, cursor, file_offset)?);
        cursor += 2;
        read_bytes(data, cursor, instruction_length, file_offset)?;
    }

    Ok(Glyph::Compound(CompoundGlyph { bounds, components }))
}

fn read_f2dot14(data: &[u8], offset: usize, file_offset: usize) -> Result<f32, FontError> {
    Ok(f32::from(read_i16(data, offset, file_offset)?) / 16_384.0)
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
    fn reads_compound_glyph_components() {
        let mut data = vec![0_u8; 16];

        data[0..2].copy_from_slice(&(-1_i16).to_be_bytes());
        data[10..12].copy_from_slice(&ARGS_ARE_XY_VALUES.to_be_bytes());
        data[12..14].copy_from_slice(&5_u16.to_be_bytes());
        data[14] = 3;
        data[15] = (-4_i8) as u8;

        let Glyph::Compound(glyph) = parse_glyph(&data, 40).unwrap() else {
            panic!("expected a compound glyph");
        };

        assert_eq!(
            glyph.components,
            vec![GlyphComponent {
                glyph_id: 5,
                transform: Transform::translate(3.0, -4.0),
            }]
        );
    }

    #[test]
    fn reads_composite_instructions_flag_from_any_component() {
        let mut data = vec![0_u8; 10];

        data[..2].copy_from_slice(&(-1_i16).to_be_bytes());

        data.extend_from_slice(&[
            // First component: instructions + another component.
            0x01, 0x22, 0, 0, 0, 0, // Last component: no instructions flag.
            0, 0x02, 0, 1, 0, 0, // Instruction length and bytes.
            0, 2, 0xaa, 0xbb,
        ]);

        let Glyph::Compound(glyph) = parse_glyph(&data, 0).unwrap() else {
            panic!("expected a composite glyph");
        };

        assert_eq!(glyph.components.len(), 2);

        // Missing the last instruction byte must be detected.
        let error = parse_glyph(&data[..data.len() - 1], 0).unwrap_err();

        assert_eq!(error.message, "truncated font data");
    }
}
