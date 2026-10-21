#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vector {
    pub x: f32,
    pub y: f32,
}

impl Vector {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Transform {
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub const fn translate(x: f32, y: f32) -> Self {
        Self {
            e: x,
            f: y,
            ..Self::IDENTITY
        }
    }

    pub const fn scale(x: f32, y: f32) -> Self {
        Self {
            a: x,
            d: y,
            ..Self::IDENTITY
        }
    }

    pub fn rotate(radians: f32) -> Self {
        let (sin, cos) = radians.sin_cos();

        Self {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            e: 0.0,
            f: 0.0,
        }
    }

    /// Returns a transformation that applies `self` first,
    /// then applies `after`.
    pub fn then(self, after: Self) -> Self {
        Self {
            a: after.a * self.a + after.c * self.b,
            b: after.b * self.a + after.d * self.b,
            c: after.a * self.c + after.c * self.d,
            d: after.b * self.c + after.d * self.d,
            e: after.a * self.e + after.c * self.f + after.e,
            f: after.b * self.e + after.d * self.f + after.f,
        }
    }

    pub fn inverse(self) -> Option<Self> {
        if !self.a.is_finite()
            || !self.b.is_finite()
            || !self.c.is_finite()
            || !self.d.is_finite()
            || !self.e.is_finite()
            || !self.f.is_finite()
        {
            return None;
        }

        let determinant = self.a * self.d - self.b * self.c;

        if determinant.abs() <= f32::EPSILON {
            return None;
        }

        let inverse = Self {
            a: self.d / determinant,
            b: -self.b / determinant,
            c: -self.c / determinant,
            d: self.a / determinant,
            e: (self.c * self.f - self.d * self.e) / determinant,
            f: (self.b * self.e - self.a * self.f) / determinant,
        };

        (inverse.a.is_finite()
            && inverse.b.is_finite()
            && inverse.c.is_finite()
            && inverse.d.is_finite()
            && inverse.e.is_finite()
            && inverse.f.is_finite())
            .then_some(inverse)
    }

    pub fn apply_point(self, point: Point) -> Point {
        Point {
            x: self.a * point.x + self.c * point.y + self.e,
            y: self.b * point.x + self.d * point.y + self.f,
        }
    }

    pub fn apply_vector(self, vector: Vector) -> Vector {
        Vector {
            x: self.a * vector.x + self.c * vector.y,
            y: self.b * vector.x + self.d * vector.y,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub min: Point,
    pub max: Point,
}

impl Rect {
    pub const fn from_point(point: Point) -> Self {
        Self {
            min: point,
            max: point,
        }
    }

    pub fn include(mut self, point: Point) -> Self {
        self.min.x = self.min.x.min(point.x);
        self.min.y = self.min.y.min(point.y);
        self.max.x = self.max.x.max(point.x);
        self.max.y = self.max.y.max(point.y);
        self
    }

    pub fn width(self) -> f32 {
        self.max.x - self.min.x
    }

    pub fn height(self) -> f32 {
        self.max.y - self.min.y
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PathCommand {
    MoveTo(Point),
    LineTo(Point),
    QuadraticTo {
        control: Point,
        to: Point,
    },
    CubicTo {
        control_one: Point,
        control_two: Point,
        to: Point,
    },
    Close,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path {
    commands: Vec<PathCommand>,
}

impl Path {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn commands(&self) -> &[PathCommand] {
        &self.commands
    }

    pub fn move_to(&mut self, to: Point) {
        self.commands.push(PathCommand::MoveTo(to));
    }

    pub fn line_to(&mut self, to: Point) {
        self.commands.push(PathCommand::LineTo(to));
    }

    pub fn quadratic_to(&mut self, control: Point, to: Point) {
        self.commands.push(PathCommand::QuadraticTo { control, to });
    }

    pub fn cubic_to(&mut self, control_one: Point, control_two: Point, to: Point) {
        self.commands.push(PathCommand::CubicTo {
            control_one,
            control_two,
            to,
        });
    }

    pub fn close(&mut self) {
        self.commands.push(PathCommand::Close);
    }

    pub fn transformed(&self, transform: Transform) -> Self {
        let commands = self
            .commands
            .iter()
            .map(|command| match command {
                PathCommand::MoveTo(to) => PathCommand::MoveTo(transform.apply_point(*to)),

                PathCommand::LineTo(to) => PathCommand::LineTo(transform.apply_point(*to)),

                PathCommand::QuadraticTo { control, to } => PathCommand::QuadraticTo {
                    control: transform.apply_point(*control),
                    to: transform.apply_point(*to),
                },

                PathCommand::CubicTo {
                    control_one,
                    control_two,
                    to,
                } => PathCommand::CubicTo {
                    control_one: transform.apply_point(*control_one),
                    control_two: transform.apply_point(*control_two),
                    to: transform.apply_point(*to),
                },

                PathCommand::Close => PathCommand::Close,
            })
            .collect();

        Self { commands }
    }

    // These are bounds of the control points, not exact geometric bounds of the curves.
    pub fn control_bounds(&self) -> Option<Rect> {
        let mut bounds: Option<Rect> = None;

        let mut include = |point: Point| {
            bounds = Some(match bounds {
                Some(existing) => existing.include(point),
                None => Rect::from_point(point),
            });
        };

        for command in &self.commands {
            match command {
                PathCommand::MoveTo(to) | PathCommand::LineTo(to) => include(*to),

                PathCommand::QuadraticTo { control, to } => {
                    include(*control);
                    include(*to);
                }

                PathCommand::CubicTo {
                    control_one,
                    control_two,
                    to,
                } => {
                    include(*control_one);
                    include(*control_two);
                    include(*to);
                }

                PathCommand::Close => {}
            };
        }

        bounds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_affine_transforms_to_points_and_vectors() {
        let transform = Transform::translate(10.0, -5.0);

        assert_eq!(
            transform.apply_point(Point::new(2.0, 3.0)),
            Point::new(12.0, -2.0)
        );

        assert_eq!(
            transform.apply_vector(Vector::new(2.0, 3.0)),
            Vector::new(2.0, 3.0)
        );
    }

    #[test]
    fn transforms_svg_and_font_style_curve_commands() {
        let mut path = Path::new();
        path.move_to(Point::new(1.0, 2.0));
        path.quadratic_to(Point::new(3.0, 4.0), Point::new(5.0, 6.0));
        path.cubic_to(
            Point::new(7.0, 8.0),
            Point::new(9.0, 10.0),
            Point::new(11.0, 12.0),
        );
        path.close();

        let transformed = path.transformed(Transform::scale(2.0, 3.0));

        assert_eq!(
            transformed.commands(),
            &[
                PathCommand::MoveTo(Point::new(2.0, 6.0)),
                PathCommand::QuadraticTo {
                    control: Point::new(6.0, 12.0),
                    to: Point::new(10.0, 18.0),
                },
                PathCommand::CubicTo {
                    control_one: Point::new(14.0, 24.0),
                    control_two: Point::new(18.0, 30.0),
                    to: Point::new(22.0, 36.0),
                },
                PathCommand::Close,
            ]
        )
    }

    #[test]
    fn composes_transforms_in_application_order() {
        let transform = Transform::scale(2.0, 3.0).then(Transform::translate(5.0, -7.0));

        assert_eq!(
            transform.apply_point(Point::new(1.0, 2.0)),
            Point::new(7.0, -1.0)
        );
    }

    #[test]
    fn rotates_points_and_inverts_transforms() {
        let rotation = Transform::rotate(std::f32::consts::FRAC_PI_2);
        let rotated = rotation.apply_point(Point::new(2.0, 0.0));

        assert!(rotated.x.abs() < 0.000_01);
        assert!((rotated.y - 2.0).abs() < 0.000_01);

        let transform = Transform::scale(2.0, 3.0).then(Transform::translate(5.0, -7.0));

        let original = Point::new(4.0, -2.0);
        let restored = transform
            .inverse()
            .unwrap()
            .apply_point(transform.apply_point(original));

        assert!((restored.x - original.x).abs() < 0.000_01);
        assert!((restored.y - original.y).abs() < 0.000_01);
    }

    #[test]
    fn does_not_invert_singular_matrices() {
        assert!(Transform::scale(0.0, 1.0).inverse().is_none());
    }
}
