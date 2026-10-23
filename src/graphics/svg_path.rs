use crate::graphics::geometry::{Path, Point};
use std::{error::Error, fmt};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SvgPathError {
    pub offset: usize,
    pub message: String,
}

impl fmt::Display for SvgPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid SVG path at byte {}: {}",
            self.offset, self.message
        )
    }
}

impl Error for SvgPathError {}

pub fn parse_path_data(input: &str) -> Result<Path, SvgPathError> {
    PathDataParser::new(input).parse()
}

struct PathDataParser<'a> {
    input: &'a str,
    cursor: usize,
    current: Point,
    subpath_start: Point,
    has_subpath: bool,
    last_cubic_control: Option<Point>,
    last_quadratic_control: Option<Point>,
}

impl<'a> PathDataParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input,
            cursor: 0,
            current: Point::new(0.0, 0.0),
            subpath_start: Point::new(0.0, 0.0),
            has_subpath: false,
            last_cubic_control: None,
            last_quadratic_control: None,
        }
    }

    fn parse(mut self) -> Result<Path, SvgPathError> {
        let mut path = Path::new();
        let mut command = None;

        loop {
            self.skip_separators();

            if self.is_at_end() {
                break;
            }

            if let Some(next_command) = self.take_command() {
                command = Some(next_command);
            }

            let Some(command_name) = command else {
                return Err(self.error("expected an SVG path command"));
            };

            match command_name {
                'M' => self.parse_move_to(&mut path, false)?,
                'm' => self.parse_move_to(&mut path, true)?,

                'L' => self.parse_line_to(&mut path, false)?,
                'l' => self.parse_line_to(&mut path, true)?,

                'H' => self.parse_horizontal_to(&mut path, false)?,
                'h' => self.parse_horizontal_to(&mut path, true)?,

                'V' => self.parse_vertical_to(&mut path, false)?,
                'v' => self.parse_vertical_to(&mut path, true)?,

                'Q' => self.parse_quadratic_to(&mut path, false)?,
                'q' => self.parse_quadratic_to(&mut path, true)?,

                'C' => self.parse_cubic_to(&mut path, false)?,
                'c' => self.parse_cubic_to(&mut path, true)?,

                'S' => self.parse_smooth_cubic_to(&mut path, false)?,
                's' => self.parse_smooth_cubic_to(&mut path, true)?,

                'T' => self.parse_smooth_quadratic_to(&mut path, false)?,
                't' => self.parse_smooth_quadratic_to(&mut path, true)?,

                'A' => self.parse_arc_to(&mut path, false)?,
                'a' => self.parse_arc_to(&mut path, true)?,

                'Z' | 'z' => {
                    if !self.has_subpath {
                        return Err(self.error("close-path appears before move-to"));
                    }

                    path.close();
                    self.current = self.subpath_start;
                    self.last_cubic_control = None;
                    self.last_quadratic_control = None;
                    command = None;
                }

                _ => return Err(self.error("unsupported SVG path command")),
            }
        }

        Ok(path)
    }

    fn parse_move_to(&mut self, path: &mut Path, relative: bool) -> Result<(), SvgPathError> {
        self.last_cubic_control = None;
        self.last_quadratic_control = None;

        let first = self.parse_point(relative)?;
        path.move_to(first);

        self.current = first;
        self.subpath_start = first;
        self.has_subpath = true;

        while self.has_number() {
            let to = self.parse_point(relative)?;
            path.line_to(to);
            self.current = to;
        }

        Ok(())
    }

    fn parse_line_to(&mut self, path: &mut Path, relative: bool) -> Result<(), SvgPathError> {
        self.last_cubic_control = None;
        self.last_quadratic_control = None;

        loop {
            let to = self.parse_point(relative)?;
            path.line_to(to);
            self.current = to;

            if !self.has_number() {
                break;
            }
        }

        Ok(())
    }

    fn parse_horizontal_to(&mut self, path: &mut Path, relative: bool) -> Result<(), SvgPathError> {
        self.last_cubic_control = None;
        self.last_quadratic_control = None;

        loop {
            let x = self.parse_number()?;
            let x = if relative { self.current.x + x } else { x };

            let to = Point::new(x, self.current.y);
            path.line_to(to);
            self.current = to;

            if !self.has_number() {
                break;
            }
        }

        Ok(())
    }

    fn parse_vertical_to(&mut self, path: &mut Path, relative: bool) -> Result<(), SvgPathError> {
        self.last_cubic_control = None;
        self.last_quadratic_control = None;

        loop {
            let y = self.parse_number()?;
            let y = if relative { self.current.y + y } else { y };

            let to = Point::new(self.current.x, y);
            path.line_to(to);
            self.current = to;

            if !self.has_number() {
                break;
            }
        }

        Ok(())
    }

    fn parse_quadratic_to(&mut self, path: &mut Path, relative: bool) -> Result<(), SvgPathError> {
        self.last_cubic_control = None;

        loop {
            let control = self.parse_point(relative)?;
            let to = self.parse_point(relative)?;

            self.last_quadratic_control = Some(control);
            path.quadratic_to(control, to);
            self.current = to;

            if !self.has_number() {
                break;
            }
        }

        Ok(())
    }

    fn parse_cubic_to(&mut self, path: &mut Path, relative: bool) -> Result<(), SvgPathError> {
        self.last_quadratic_control = None;

        loop {
            let control_one = self.parse_point(relative)?;
            let control_two = self.parse_point(relative)?;
            let to = self.parse_point(relative)?;

            self.last_cubic_control = Some(control_two);
            path.cubic_to(control_one, control_two, to);
            self.current = to;

            if !self.has_number() {
                break;
            }
        }

        Ok(())
    }

    fn parse_smooth_cubic_to(
        &mut self,
        path: &mut Path,
        relative: bool,
    ) -> Result<(), SvgPathError> {
        self.last_quadratic_control = None;

        loop {
            let control_one = self
                .last_cubic_control
                .map(|previous| reflect_control(previous, self.current))
                .unwrap_or(self.current);

            let control_two = self.parse_point(relative)?;
            let to = self.parse_point(relative)?;

            path.cubic_to(control_one, control_two, to);
            self.current = to;
            self.last_cubic_control = Some(control_two);

            if !self.has_number() {
                break;
            }
        }

        Ok(())
    }

    fn parse_smooth_quadratic_to(
        &mut self,
        path: &mut Path,
        relative: bool,
    ) -> Result<(), SvgPathError> {
        self.last_cubic_control = None;

        loop {
            let control = self
                .last_quadratic_control
                .map(|previous| reflect_control(previous, self.current))
                .unwrap_or(self.current);

            let to = self.parse_point(relative)?;

            path.quadratic_to(control, to);
            self.current = to;
            self.last_quadratic_control = Some(control);

            if !self.has_number() {
                break;
            }
        }

        Ok(())
    }

    fn parse_arc_to(&mut self, path: &mut Path, relative: bool) -> Result<(), SvgPathError> {
        self.last_cubic_control = None;
        self.last_quadratic_control = None;

        loop {
            let radius_x = self.parse_number()?.abs();
            let radius_y = self.parse_number()?.abs();
            let rotation = self.parse_number()?.to_radians();
            let large_arc = self.parse_arc_flag()?;
            let sweep = self.parse_arc_flag()?;
            let to = self.parse_point(relative)?;

            append_elliptical_arc(
                path,
                self.current,
                to,
                radius_x,
                radius_y,
                rotation,
                large_arc,
                sweep,
            );

            self.current = to;

            if !self.has_number() {
                break;
            }
        }

        Ok(())
    }

    fn parse_arc_flag(&mut self) -> Result<bool, SvgPathError> {
        self.skip_separators();

        match self.peek_byte() {
            Some(b'0') => {
                self.cursor += 1;
                Ok(false)
            }
            Some(b'1') => {
                self.cursor += 1;
                Ok(true)
            }
            _ => Err(self.error("expected arc flag 0 or 1")),
        }
    }

    fn parse_point(&mut self, relative: bool) -> Result<Point, SvgPathError> {
        let x = self.parse_number()?;
        let y = self.parse_number()?;

        Ok(if relative {
            Point::new(self.current.x + x, self.current.y + y)
        } else {
            Point::new(x, y)
        })
    }

    fn parse_number(&mut self) -> Result<f32, SvgPathError> {
        self.skip_separators();

        let start = self.cursor;

        if self
            .peek_byte()
            .is_some_and(|byte| byte == b'+' || byte == b'-')
        {
            self.cursor += 1;
        }

        let digits_before_dot = self.consume_digits();

        if self.peek_byte() == Some(b'.') {
            self.cursor += 1;
        }

        let digits_after_dot = self.consume_digits();

        if !digits_before_dot && !digits_after_dot {
            self.cursor = start;
            return Err(self.error("expected a number"));
        }

        if self
            .peek_byte()
            .is_some_and(|byte| byte == b'e' || byte == b'E')
        {
            self.cursor += 1;

            if self
                .peek_byte()
                .is_some_and(|byte| byte == b'+' || byte == b'-')
            {
                self.cursor += 1;
            }

            if !self.consume_digits() {
                return Err(self.error("expected exponent digits"));
            }
        }

        let value = self.input[start..self.cursor]
            .parse::<f32>()
            .map_err(|_| SvgPathError {
                offset: start,
                message: "invalid number".to_owned(),
            })?;

        if !value.is_finite() {
            return Err(SvgPathError {
                offset: start,
                message: "number must be finite".to_owned(),
            });
        }

        Ok(value)
    }

    fn consume_digits(&mut self) -> bool {
        let start = self.cursor;

        while self.peek_byte().is_some_and(|byte| byte.is_ascii_digit()) {
            self.cursor += 1;
        }

        self.cursor != start
    }

    fn has_number(&mut self) -> bool {
        self.skip_separators();

        self.peek_byte().is_some_and(|byte| {
            byte.is_ascii_digit() || byte == b'+' || byte == b'-' || byte == b'.'
        })
    }

    fn skip_separators(&mut self) {
        while self
            .peek_byte()
            .is_some_and(|byte| byte == b',' || byte.is_ascii_whitespace())
        {
            self.cursor += 1;
        }
    }

    fn take_command(&mut self) -> Option<char> {
        let byte = self.peek_byte()?;

        if !byte.is_ascii_alphabetic() {
            return None;
        }

        self.cursor += 1;
        Some(byte as char)
    }

    fn peek_byte(&self) -> Option<u8> {
        self.input.as_bytes().get(self.cursor).copied()
    }

    fn is_at_end(&self) -> bool {
        self.cursor >= self.input.len()
    }

    fn error(&self, message: &str) -> SvgPathError {
        SvgPathError {
            offset: self.cursor,
            message: message.to_owned(),
        }
    }
}

fn append_elliptical_arc(
    path: &mut Path,
    from: Point,
    to: Point,
    mut radius_x: f32,
    mut radius_y: f32,
    rotation: f32,
    large_arc: bool,
    sweep: bool,
) {
    if from == to {
        return;
    }

    if radius_x == 0.0 || radius_y == 0.0 {
        path.line_to(to);
        return;
    }

    let (sin_rotation, cos_rotation) = rotation.sin_cos();
    let half_delta_x = (from.x - to.x) * 0.5;
    let half_delta_y = (from.y - to.y) * 0.5;

    let x_prime = cos_rotation * half_delta_x + sin_rotation * half_delta_y;
    let y_prime = -sin_rotation * half_delta_x + cos_rotation * half_delta_y;

    let mut radius_x_squared = radius_x * radius_x;
    let mut radius_y_squared = radius_y * radius_y;

    let radius_scale = x_prime * x_prime / radius_x_squared + y_prime * y_prime / radius_y_squared;

    if !radius_scale.is_finite() {
        path.line_to(to);
        return;
    }

    if radius_scale > 1.0 {
        let scale = radius_scale.sqrt();
        radius_x *= scale;
        radius_y *= scale;
        radius_x_squared = radius_x * radius_x;
        radius_y_squared = radius_y * radius_y;
    }

    let numerator = radius_x_squared * radius_y_squared
        - radius_x_squared * y_prime * y_prime
        - radius_y_squared * x_prime * x_prime;

    let denominator = radius_x_squared * y_prime * y_prime + radius_y_squared * x_prime * x_prime;

    let center_factor = if denominator <= f32::EPSILON {
        0.0
    } else {
        (numerator.max(0.0) / denominator).sqrt()
    };

    let center_factor = if large_arc == sweep {
        -center_factor
    } else {
        center_factor
    };

    let center_x_prime = center_factor * radius_x * y_prime / radius_y;
    let center_y_prime = -center_factor * radius_y * x_prime / radius_x;

    let center = Point::new(
        cos_rotation * center_x_prime - sin_rotation * center_y_prime + (from.x + to.x) * 0.5,
        sin_rotation * center_x_prime + cos_rotation * center_y_prime + (from.y + to.y) * 0.5,
    );

    if !point_is_finite(center) {
        path.line_to(to);
        return;
    }

    let start_x = (x_prime - center_x_prime) / radius_x;
    let start_y = (y_prime - center_y_prime) / radius_y;
    let end_x = (-x_prime - center_x_prime) / radius_x;
    let end_y = (-y_prime - center_y_prime) / radius_y;

    let start_angle = start_y.atan2(start_x);
    let mut sweep_angle = signed_angle(start_x, start_y, end_x, end_y);

    if !sweep && sweep_angle > 0.0 {
        sweep_angle -= std::f32::consts::TAU;
    } else if sweep && sweep_angle < 0.0 {
        sweep_angle += std::f32::consts::TAU;
    }

    if sweep_angle.abs() <= f32::EPSILON {
        path.line_to(to);
        return;
    }

    let segment_count = (sweep_angle.abs() / std::f32::consts::FRAC_PI_2)
        .ceil()
        .max(1.0) as u32;

    let segment_angle = sweep_angle / segment_count as f32;

    for index in 0..segment_count {
        let segment_start_angle = start_angle + segment_angle * index as f32;
        let segment_end_angle = segment_start_angle + segment_angle;

        let segment_from = if index == 0 {
            from
        } else {
            ellipse_point(
                center,
                radius_x,
                radius_y,
                cos_rotation,
                sin_rotation,
                segment_start_angle,
            )
        };

        let segment_to = if index + 1 == segment_count {
            to
        } else {
            ellipse_point(
                center,
                radius_x,
                radius_y,
                cos_rotation,
                sin_rotation,
                segment_end_angle,
            )
        };

        let handle_scale = 4.0 / 3.0 * (segment_angle * 0.25).tan();
        let start_derivative = ellipse_derivative(
            radius_x,
            radius_y,
            cos_rotation,
            sin_rotation,
            segment_start_angle,
        );
        let end_derivative = ellipse_derivative(
            radius_x,
            radius_y,
            cos_rotation,
            sin_rotation,
            segment_end_angle,
        );

        let control_one = Point::new(
            segment_from.x + start_derivative.x * handle_scale,
            segment_from.y + start_derivative.y * handle_scale,
        );

        let control_two = Point::new(
            segment_to.x - end_derivative.x * handle_scale,
            segment_to.y - end_derivative.y * handle_scale,
        );

        if !point_is_finite(control_one) || !point_is_finite(control_two) {
            path.line_to(to);
            return;
        }

        path.cubic_to(control_one, control_two, segment_to);
    }
}

fn ellipse_point(
    center: Point,
    radius_x: f32,
    radius_y: f32,
    cos_rotation: f32,
    sin_rotation: f32,
    angle: f32,
) -> Point {
    let (sin_angle, cos_angle) = angle.sin_cos();

    Point::new(
        center.x + radius_x * cos_rotation * cos_angle - radius_y * sin_rotation * sin_angle,
        center.y + radius_x * sin_rotation * cos_angle + radius_y * cos_rotation * sin_angle,
    )
}

fn ellipse_derivative(
    radius_x: f32,
    radius_y: f32,
    cos_rotation: f32,
    sin_rotation: f32,
    angle: f32,
) -> Point {
    let (sin_angle, cos_angle) = angle.sin_cos();

    Point::new(
        -radius_x * cos_rotation * sin_angle - radius_y * sin_rotation * cos_angle,
        -radius_x * sin_rotation * sin_angle + radius_y * cos_rotation * cos_angle,
    )
}

fn signed_angle(from_x: f32, from_y: f32, to_x: f32, to_y: f32) -> f32 {
    (from_x * to_y - from_y * to_x).atan2(from_x * to_x + from_y * to_y)
}

fn point_is_finite(point: Point) -> bool {
    point.x.is_finite() && point.y.is_finite()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::geometry::PathCommand;

    #[test]
    fn parses_line_commands_and_implicit_line_tos() {
        let path = parse_path_data("M 10 20 L 30 40 50 60 H 70 V 80 Z").unwrap();

        assert_eq!(
            path.commands(),
            &[
                PathCommand::MoveTo(Point::new(10.0, 20.0)),
                PathCommand::LineTo(Point::new(30.0, 40.0)),
                PathCommand::LineTo(Point::new(50.0, 60.0)),
                PathCommand::LineTo(Point::new(70.0, 60.0)),
                PathCommand::LineTo(Point::new(70.0, 80.0)),
                PathCommand::Close,
            ]
        );
    }

    #[test]
    fn parses_relative_lines_and_curves() {
        let path = parse_path_data("M10 10 l5 0 0 5 q 2 3 4 5 c 1 2 3 4 5 6 z").unwrap();

        assert_eq!(
            path.commands(),
            &[
                PathCommand::MoveTo(Point::new(10.0, 10.0)),
                PathCommand::LineTo(Point::new(15.0, 10.0)),
                PathCommand::LineTo(Point::new(15.0, 15.0)),
                PathCommand::QuadraticTo {
                    control: Point::new(17.0, 18.0),
                    to: Point::new(19.0, 20.0),
                },
                PathCommand::CubicTo {
                    control_one: Point::new(20.0, 22.0),
                    control_two: Point::new(22.0, 24.0),
                    to: Point::new(24.0, 26.0),
                },
                PathCommand::Close,
            ]
        );
    }

    #[test]
    fn rejects_a_path_without_an_initial_command() {
        let error = parse_path_data("10 20").unwrap_err();

        assert_eq!(error.message, "expected an SVG path command");
    }

    #[test]
    fn parses_smooth_curve_commands() {
        let path =
            parse_path_data("M 10 10 C 12 14 16 18 20 20 S 28 26 30 30 Q 34 36 38 40 T 46 50")
                .unwrap();

        assert_eq!(
            path.commands(),
            &[
                PathCommand::MoveTo(Point::new(10.0, 10.0)),
                PathCommand::CubicTo {
                    control_one: Point::new(12.0, 14.0),
                    control_two: Point::new(16.0, 18.0),
                    to: Point::new(20.0, 20.0),
                },
                PathCommand::CubicTo {
                    control_one: Point::new(24.0, 22.0),
                    control_two: Point::new(28.0, 26.0),
                    to: Point::new(30.0, 30.0),
                },
                PathCommand::QuadraticTo {
                    control: Point::new(34.0, 36.0),
                    to: Point::new(38.0, 40.0),
                },
                PathCommand::QuadraticTo {
                    control: Point::new(42.0, 44.0),
                    to: Point::new(46.0, 50.0),
                }
            ]
        )
    }

    #[test]
    fn reflects_controls_for_repeated_smooth_curves() {
        let path = parse_path_data(
            "M 0 0 \
            C 1 2 2 4 6 8 \
            S 10 12 13 15 \
            S 20 22 24 26 \
            Q 26 28 30 32 \
            T 37 39 \
            T 45 47",
        )
        .unwrap();

        assert_eq!(
            path.commands(),
            &[
                PathCommand::MoveTo(Point::new(0.0, 0.0)),
                PathCommand::CubicTo {
                    control_one: Point::new(1.0, 2.0),
                    control_two: Point::new(2.0, 4.0),
                    to: Point::new(6.0, 8.0),
                },
                PathCommand::CubicTo {
                    control_one: Point::new(10.0, 12.0),
                    control_two: Point::new(10.0, 12.0),
                    to: Point::new(13.0, 15.0),
                },
                PathCommand::CubicTo {
                    control_one: Point::new(16.0, 18.0),
                    control_two: Point::new(20.0, 22.0),
                    to: Point::new(24.0, 26.0),
                },
                PathCommand::QuadraticTo {
                    control: Point::new(26.0, 28.0),
                    to: Point::new(30.0, 32.0),
                },
                PathCommand::QuadraticTo {
                    control: Point::new(34.0, 36.0),
                    to: Point::new(37.0, 39.0),
                },
                PathCommand::QuadraticTo {
                    control: Point::new(40.0, 42.0),
                    to: Point::new(45.0, 47.0),
                }
            ]
        );
    }

    #[test]
    fn converts_elliptical_arcs_to_cubic_curves() {
        let path = parse_path_data("M 10 10 A 10 10 0 0 1 30 10").unwrap();

        assert_eq!(path.commands().len(), 3);
        assert!(matches!(path.commands()[1], PathCommand::CubicTo { .. }));
        assert!(matches!(path.commands()[2], PathCommand::CubicTo { .. }));

        let PathCommand::CubicTo { to, .. } = path.commands()[2] else {
            panic!("the final arc segment must be cubic");
        };

        assert_eq!(to, Point::new(30.0, 10.0));
    }

    #[test]
    fn turns_zero_radius_arc_into_a_line() {
        let path = parse_path_data("M 1 2 A 0 5 0 0 1 8 9").unwrap();

        assert_eq!(
            path.commands(),
            &[
                PathCommand::MoveTo(Point::new(1.0, 2.0)),
                PathCommand::LineTo(Point::new(8.0, 9.0))
            ]
        );
    }

    #[test]
    fn rejects_invalid_arc_flags() {
        let error = parse_path_data("M 0 0 A 5 5 0 2 0 10 10").unwrap_err();

        assert_eq!(error.message, "expected arc flag 0 or 1");
    }
}

fn reflect_control(control: Point, center: Point) -> Point {
    Point::new(2.0 * center.x - control.x, 2.0 * center.y - control.y)
}
