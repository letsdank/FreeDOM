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
}

impl<'a> PathDataParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input,
            cursor: 0,
            current: Point::new(0.0, 0.0),
            subpath_start: Point::new(0.0, 0.0),
            has_subpath: false,
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

                'Z' | 'z' => {
                    if !self.has_subpath {
                        return Err(self.error("close-path appears before move-to"));
                    }

                    path.close();
                    self.current = self.subpath_start;
                    command = None;
                }

                _ => return Err(self.error("unsupported SVG path command")),
            }
        }

        Ok(path)
    }

    fn parse_move_to(&mut self, path: &mut Path, relative: bool) -> Result<(), SvgPathError> {
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
        loop {
            let control = self.parse_point(relative)?;
            let to = self.parse_point(relative)?;

            path.quadratic_to(control, to);
            self.current = to;

            if !self.has_number() {
                break;
            }
        }

        Ok(())
    }

    fn parse_cubic_to(&mut self, path: &mut Path, relative: bool) -> Result<(), SvgPathError> {
        loop {
            let control_one = self.parse_point(relative)?;
            let control_two = self.parse_point(relative)?;
            let to = self.parse_point(relative)?;

            path.cubic_to(control_one, control_two, to);
            self.current = to;

            if !self.has_number() {
                break;
            }
        }

        Ok(())
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
}
