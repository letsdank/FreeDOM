use std::fmt;

use crate::graphics::geometry::{Path, PathCommand, Point};

const MAX_SUBDIVISION_DEPTH: u8 = 12;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlattenedPath {
    contours: Vec<FlattenedContour>,
}

impl FlattenedPath {
    pub fn contours(&self) -> &[FlattenedContour] {
        &self.contours
    }

    pub(crate) fn from_open_contours(contours: Vec<Vec<Point>>) -> Self {
        Self {
            contours: contours
                .into_iter()
                .filter(|points| points.len() >= 2)
                .map(|points| FlattenedContour {
                    points,
                    closed: false,
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FlattenedContour {
    points: Vec<Point>,
    closed: bool,
}

impl FlattenedContour {
    fn new(start: Point) -> Self {
        Self {
            points: vec![start],
            closed: false,
        }
    }

    pub fn points(&self) -> &[Point] {
        &self.points
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlattenError {
    InvalidTolerance,
}

impl fmt::Display for FlattenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTolerance => {
                write!(
                    formatter,
                    "flattening tolerance must be finite and positive"
                )
            }
        }
    }
}

impl std::error::Error for FlattenError {}

pub fn flatten_path(path: &Path, tolerance: f32) -> Result<FlattenedPath, FlattenError> {
    if !tolerance.is_finite() || tolerance <= 0.0 {
        return Err(FlattenError::InvalidTolerance);
    }

    let mut contours = Vec::new();
    let mut active_contour: Option<FlattenedContour> = None;
    let mut current: Option<Point> = None;
    let mut subpath_start: Option<Point> = None;

    for command in path.commands() {
        match command {
            PathCommand::MoveTo(to) => {
                finish_contour(&mut contours, &mut active_contour);

                active_contour = Some(FlattenedContour::new(*to));
                current = Some(*to);
                subpath_start = Some(*to);
            }

            PathCommand::LineTo(to) => {
                let Some(from) = current else {
                    continue;
                };

                let contour = active_contour.get_or_insert_with(|| FlattenedContour::new(from));
                append_point(&mut contour.points, *to);
                current = Some(*to);
            }

            PathCommand::QuadraticTo { control, to } => {
                let Some(from) = current else {
                    continue;
                };

                let contour = active_contour.get_or_insert_with(|| FlattenedContour::new(from));
                flatten_quadratic(&mut contour.points, from, *control, *to, tolerance, 0);
                current = Some(*to);
            }

            PathCommand::CubicTo {
                control_one,
                control_two,
                to,
            } => {
                let Some(from) = current else {
                    continue;
                };

                let contour = active_contour.get_or_insert_with(|| FlattenedContour::new(from));
                flatten_cubic(
                    &mut contour.points,
                    from,
                    *control_one,
                    *control_two,
                    *to,
                    tolerance,
                    0,
                );
                current = Some(*to);
            }

            PathCommand::Close => {
                if let Some(mut contour) = active_contour.take() {
                    contour.closed = true;
                    contours.push(contour);
                }

                current = subpath_start;
            }
        }
    }

    finish_contour(&mut contours, &mut active_contour);

    Ok(FlattenedPath { contours })
}

fn finish_contour(
    contours: &mut Vec<FlattenedContour>,
    active_contour: &mut Option<FlattenedContour>,
) {
    if let Some(contour) = active_contour.take() {
        contours.push(contour);
    }
}

fn append_point(points: &mut Vec<Point>, point: Point) {
    if points.last().copied() != Some(point) {
        points.push(point);
    }
}

fn flatten_quadratic(
    points: &mut Vec<Point>,
    from: Point,
    control: Point,
    to: Point,
    tolerance: f32,
    depth: u8,
) {
    if depth >= MAX_SUBDIVISION_DEPTH || point_line_distance(control, from, to) <= tolerance {
        append_point(points, to);
        return;
    }

    let from_control = midpoint(from, control);
    let control_to = midpoint(control, to);
    let middle = midpoint(from_control, control_to);

    flatten_quadratic(points, from, from_control, middle, tolerance, depth + 1);
    flatten_quadratic(points, middle, control_to, to, tolerance, depth + 1);
}

fn flatten_cubic(
    points: &mut Vec<Point>,
    from: Point,
    control_one: Point,
    control_two: Point,
    to: Point,
    tolerance: f32,
    depth: u8,
) {
    let flat_enough = point_line_distance(control_one, from, to) <= tolerance
        && point_line_distance(control_two, from, to) <= tolerance;

    if depth >= MAX_SUBDIVISION_DEPTH || flat_enough {
        append_point(points, to);
        return;
    }

    let first = midpoint(from, control_one);
    let second = midpoint(control_one, control_two);
    let third = midpoint(control_two, to);

    let left_control_two = midpoint(first, second);
    let right_control_one = midpoint(second, third);
    let middle = midpoint(left_control_two, right_control_one);

    flatten_cubic(
        points,
        from,
        first,
        left_control_two,
        middle,
        tolerance,
        depth + 1,
    );
    flatten_cubic(
        points,
        middle,
        right_control_one,
        third,
        to,
        tolerance,
        depth + 1,
    );
}

fn midpoint(first: Point, second: Point) -> Point {
    Point::new((first.x + second.x) * 0.5, (first.y + second.y) * 0.5)
}

fn point_line_distance(point: Point, from: Point, to: Point) -> f32 {
    let delta_x = to.x - from.x;
    let delta_y = to.y - from.y;
    let length = (delta_x * delta_x + delta_y * delta_y).sqrt();

    if length <= f32::EPSILON {
        let point_x = point.x - from.x;
        let point_y = point.y - from.y;
        return (point_x * point_x + point_y * point_y).sqrt();
    }

    ((point.x - from.x) * delta_y - (point.y - from.y) * delta_x).abs() / length
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::geometry::Path;

    #[test]
    fn preserves_contours_and_closed_state() {
        let mut path = Path::new();
        path.move_to(Point::new(1.0, 2.0));
        path.line_to(Point::new(5.0, 2.0));
        path.line_to(Point::new(5.0, 6.0));
        path.close();

        let flattened = flatten_path(&path, 0.25).unwrap();

        assert_eq!(flattened.contours().len(), 1);

        let contour = &flattened.contours()[0];
        assert!(contour.is_closed());
        assert_eq!(
            contour.points(),
            &[
                Point::new(1.0, 2.0),
                Point::new(5.0, 2.0),
                Point::new(5.0, 6.0),
            ]
        );
    }

    #[test]
    fn subdivides_curves_and_keeps_endpoints() {
        let mut path = Path::new();
        path.move_to(Point::new(0.0, 0.0));
        path.cubic_to(
            Point::new(0.0, 10.0),
            Point::new(10.0, 10.0),
            Point::new(10.0, 0.0),
        );

        let flattened = flatten_path(&path, 0.25).unwrap();
        let points = flattened.contours()[0].points();

        assert!(points.len() > 2);
        assert_eq!(points.first(), Some(&Point::new(0.0, 0.0)));
        assert_eq!(points.last(), Some(&Point::new(10.0, 0.0)));
    }

    #[test]
    fn rejects_invalid_tolerance() {
        let path = Path::new();

        assert_eq!(
            flatten_path(&path, 0.0),
            Err(FlattenError::InvalidTolerance)
        );
    }
}
