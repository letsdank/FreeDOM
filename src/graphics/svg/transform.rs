//! Parsing of SVG transform lists.
//!
//! Specification: <https://www.w3.org/TR/SVG2/coords.html#TransformProperty>

use std::{error::Error, fmt};

use crate::graphics::geometry::Transform;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SvgTransformError {
    pub offset: usize,
    pub message: String,
}

impl fmt::Display for SvgTransformError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid SVG transform at byte {}: {}",
            self.offset, self.message
        )
    }
}

impl Error for SvgTransformError {}

pub fn parse_transform_list(input: &str) -> Result<Transform, SvgTransformError> {
    TransformParser::new(input).parse()
}

struct TransformParser<'a> {
    input: &'a str,
    cursor: usize,
}

impl<'a> TransformParser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, cursor: 0 }
    }

    fn parse(&mut self) -> Result<Transform, SvgTransformError> {
        let mut result = Transform::IDENTITY;

        self.skip_separators();

        while !self.is_at_end() {
            let name = self.parse_name()?;

            self.skip_separators();
            self.expect_byte(b'(')?;

            let operation = match name {
                "matrix" => self.parse_matrix()?,
                "translate" => self.parse_translate()?,
                "scale" => self.parse_scale()?,
                "rotate" => self.parse_rotate()?,
                "skewX" => self.parse_skew_x()?,
                "skewY" => self.parse_skew_y()?,
                _ => return Err(self.error("unsupported SVG transform function")),
            };

            self.skip_separators();
            self.expect_byte(b')')?;

            if !transform_is_finite(operation) {
                return Err(self.error("transform contains a non-finite value"));
            }

            // SVG multiplies transform-list matrices on the right:
            // translate(...) scale(...) means "scale, then translate".
            result = operation.then(result);

            self.skip_separators();
        }

        Ok(result)
    }

    fn parse_matrix(&mut self) -> Result<Transform, SvgTransformError> {
        Ok(Transform {
            a: self.parse_number()?,
            b: self.parse_number()?,
            c: self.parse_number()?,
            d: self.parse_number()?,
            e: self.parse_number()?,
            f: self.parse_number()?,
        })
    }

    fn parse_translate(&mut self) -> Result<Transform, SvgTransformError> {
        let x = self.parse_number()?;
        let y = if self.next_is_closing_parenthesis() {
            0.0
        } else {
            self.parse_number()?
        };

        Ok(Transform::translate(x, y))
    }

    fn parse_scale(&mut self) -> Result<Transform, SvgTransformError> {
        let x = self.parse_number()?;
        let y = if self.next_is_closing_parenthesis() {
            x
        } else {
            self.parse_number()?
        };

        Ok(Transform::scale(x, y))
    }

    fn parse_rotate(&mut self) -> Result<Transform, SvgTransformError> {
        let angle = self.parse_number()?.to_radians();

        if self.next_is_closing_parenthesis() {
            return Ok(Transform::rotate(angle));
        }

        let center_x = self.parse_number()?;
        let center_y = self.parse_number()?;

        Ok(Transform::translate(-center_x, -center_y)
            .then(Transform::rotate(angle))
            .then(Transform::translate(center_x, center_y)))
    }

    fn parse_skew_x(&mut self) -> Result<Transform, SvgTransformError> {
        let tangent = self.parse_number()?.to_radians().tan();

        Ok(Transform {
            a: 1.0,
            b: 0.0,
            c: tangent,
            d: 1.0,
            e: 0.0,
            f: 0.0,
        })
    }

    fn parse_skew_y(&mut self) -> Result<Transform, SvgTransformError> {
        let tangent = self.parse_number()?.to_radians().tan();

        Ok(Transform {
            a: 1.0,
            b: tangent,
            c: 0.0,
            d: 1.0,
            e: 0.0,
            f: 0.0,
        })
    }

    fn parse_name(&mut self) -> Result<&'a str, SvgTransformError> {
        let start = self.cursor;

        while self
            .peek_byte()
            .is_some_and(|byte| byte.is_ascii_alphabetic())
        {
            self.cursor += 1;
        }

        if self.cursor == start {
            return Err(self.error("expected transform function name"));
        }

        Ok(&self.input[start..self.cursor])
    }

    fn parse_number(&mut self) -> Result<f32, SvgTransformError> {
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

        let value =
            self.input[start..self.cursor]
                .parse::<f32>()
                .map_err(|_| SvgTransformError {
                    offset: start,
                    message: "invalid number".to_owned(),
                })?;

        if !value.is_finite() {
            return Err(SvgTransformError {
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

    fn next_is_closing_parenthesis(&mut self) -> bool {
        self.skip_separators();
        self.peek_byte() == Some(b')')
    }

    fn expect_byte(&mut self, expected: u8) -> Result<(), SvgTransformError> {
        if self.peek_byte() == Some(expected) {
            self.cursor += 1;
            Ok(())
        } else {
            Err(self.error("unexpected transform syntax"))
        }
    }

    fn skip_separators(&mut self) {
        while self
            .peek_byte()
            .is_some_and(|byte| byte == b',' || byte.is_ascii_whitespace())
        {
            self.cursor += 1;
        }
    }

    fn peek_byte(&self) -> Option<u8> {
        self.input.as_bytes().get(self.cursor).copied()
    }

    fn is_at_end(&self) -> bool {
        self.cursor >= self.input.len()
    }

    fn error(&self, message: &str) -> SvgTransformError {
        SvgTransformError {
            offset: self.cursor,
            message: message.to_owned(),
        }
    }
}

fn transform_is_finite(transform: Transform) -> bool {
    transform.a.is_finite()
        && transform.b.is_finite()
        && transform.c.is_finite()
        && transform.d.is_finite()
        && transform.e.is_finite()
        && transform.f.is_finite()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::geometry::Point;

    #[test]
    fn parses_svg_transform_lists_in_svg_order() {
        let transform = parse_transform_list("translate(10 20) scale(2)").unwrap();

        assert_eq!(
            transform.apply_point(Point::new(1.0, 1.0)),
            Point::new(12.0, 22.0)
        );
    }

    #[test]
    fn parses_matrix_and_skew_functions() {
        let matrix = parse_transform_list("matrix(2 0 0 3 4 5)").unwrap();

        assert_eq!(
            matrix.apply_point(Point::new(1.0, 2.0)),
            Point::new(6.0, 11.0)
        );

        let skew = parse_transform_list("skewX(45)").unwrap();
        let point = skew.apply_point(Point::new(1.0, 2.0));

        assert!((point.x - 3.0).abs() < 0.000_01);
        assert!((point.y - 2.0).abs() < 0.000_01);
    }

    #[test]
    fn rotates_around_the_specified_center() {
        let transform = parse_transform_list("rotate(90 10 20)").unwrap();
        let point = transform.apply_point(Point::new(11.0, 20.0));

        assert!((point.x - 10.0).abs() < 0.000_01);
        assert!((point.y - 21.0).abs() < 0.000_01);
    }

    #[test]
    fn rejects_unknown_or_incomplete_transform_syntax() {
        assert!(parse_transform_list("perspective(10)").is_err());
        assert!(parse_transform_list("translate(10").is_err());
    }
}
