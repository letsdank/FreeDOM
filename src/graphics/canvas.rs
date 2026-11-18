//! A small SDR Canvas 2D drawing core.
//!
//! Rectangle operations and bitmap initialization:
//! <https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-fillrect-dev>
//!
//! Color management and full Canvas 2D semantics are not implemented yet.

use super::{
    flatten::flatten_path,
    geometry::{Path, Point, Transform},
    raster::{Color, FillRule, StrokeStyle, Surface, fill_path, stroke_path_transformed},
};

const MAX_CANVAS_DIMENSION: usize = 16_384;
const MAX_CANVAS_PIXELS: usize = 16 * 1024 * 1024;
pub(crate) const MAX_PAGE_CANVAS_PIXELS: usize = 16 * 1024 * 1024;
pub(crate) const MAX_PAGE_CANVASES: usize = 1024;
const MAX_CANVAS_PATH_COMMANDS: usize = 100_000;

const SAMPLE_OFFSETS: [(f64, f64); 4] = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];

pub struct Canvas2D {
    surface: Surface,
    fill_style: Color,
    current_path: Path,
    has_subpath: bool,
    transform: Transform,
}

impl Canvas2D {
    /// Returns None when the requested surface exceeds FreeDOM's current limits.
    pub fn new(width: usize, height: usize) -> Option<Self> {
        if !valid_size(width, height) {
            return None;
        }

        Some(Self {
            surface: Surface::new(width, height, Color::TRANSPARENT),
            fill_style: Color::BLACK,
            current_path: Path::new(),
            has_subpath: false,
            transform: Transform::IDENTITY,
        })
    }

    pub fn surface(&self) -> &Surface {
        &self.surface
    }

    pub fn fill_style(&self) -> Color {
        self.fill_style
    }

    pub fn set_fill_style(&mut self, color: Color) {
        self.fill_style = color;
    }

    /// Resets the bitmap and drawing state even when the dimensions are unchanged.
    pub fn resize(&mut self, width: usize, height: usize) -> bool {
        let Some(canvas) = Self::new(width, height) else {
            return false;
        };

        *self = canvas;
        true
    }

    pub fn fill_rect(&mut self, x: f64, y: f64, width: f64, height: f64) {
        if !self.has_translation_only() {
            if let Some(points) = self.transformed_rect(x, y, width, height) {
                let mut path = Path::new();

                path.move_to(points[0]);

                for point in &points[1..] {
                    path.line_to(*point);
                }

                path.close();

                let flattened =
                    flatten_path(&path, 0.25).expect("constant flattening tolerance is valid");

                fill_path(
                    &mut self.surface,
                    &flattened,
                    FillRule::NonZero,
                    self.fill_style,
                );
            }

            return;
        }

        let x = x + f64::from(self.transform.e);
        let y = y + f64::from(self.transform.f);

        let Some(rect) = self.clipped_rect(x, y, width, height) else {
            return;
        };

        let mut path = Path::new();

        path.move_to(Point::new(rect.left as f32, rect.top as f32));
        path.line_to(Point::new(rect.right as f32, rect.top as f32));
        path.line_to(Point::new(rect.right as f32, rect.bottom as f32));
        path.line_to(Point::new(rect.left as f32, rect.bottom as f32));
        path.close();

        let flattened = flatten_path(&path, 0.25).expect("constant flattening tolerance is valid");

        fill_path(
            &mut self.surface,
            &flattened,
            FillRule::NonZero,
            self.fill_style,
        );
    }

    pub fn clear_rect(&mut self, x: f64, y: f64, width: f64, height: f64) {
        if !self.has_translation_only() {
            if let Some(points) = self.transformed_rect(x, y, width, height) {
                self.clear_quad(points);
            }

            return;
        }

        let x = x + f64::from(self.transform.e);
        let y = y + f64::from(self.transform.f);

        let Some(rect) = self.clipped_rect(x, y, width, height) else {
            return;
        };

        for py in rect.top.floor() as usize..rect.bottom.ceil() as usize {
            for px in rect.left.floor() as usize..rect.right.ceil() as usize {
                let covered = SAMPLE_OFFSETS
                    .iter()
                    .filter(|(dx, dy)| {
                        let sx = px as f64 + dx;
                        let sy = py as f64 + dy;

                        sx >= rect.left && sx < rect.right && sy >= rect.top && sy < rect.bottom
                    })
                    .count();

                self.clear_pixel(px, py, covered);
            }
        }
    }

    fn clipped_rect(&self, x: f64, y: f64, width: f64, height: f64) -> Option<Rect> {
        if ![x, y, width, height].iter().all(|value| value.is_finite())
            || width == 0.0
            || height == 0.0
        {
            return None;
        }

        let right = x + width;
        let bottom = y + height;

        let limit_x = self.surface.width() as f64;
        let limit_y = self.surface.height() as f64;

        let rect = Rect {
            left: x.min(right).clamp(0.0, limit_x),
            top: y.min(bottom).clamp(0.0, limit_y),
            right: x.max(right).clamp(0.0, limit_x),
            bottom: y.max(bottom).clamp(0.0, limit_y),
        };

        (rect.left < rect.right && rect.top < rect.bottom).then_some(rect)
    }

    /// Starts an empty current path without changing the bitmap.
    pub fn begin_path(&mut self) {
        self.current_path = Path::new();
        self.has_subpath = false;
    }

    pub fn move_to(&mut self, x: f64, y: f64) {
        let Some(point) = self.canvas_point(x, y) else {
            return;
        };

        if self.current_path.commands().len() >= MAX_CANVAS_PATH_COMMANDS {
            return;
        }

        self.current_path.move_to(point);
        self.has_subpath = true;
    }

    pub fn line_to(&mut self, x: f64, y: f64) {
        let Some(point) = self.canvas_point(x, y) else {
            return;
        };

        if self.current_path.commands().len() >= MAX_CANVAS_PATH_COMMANDS {
            return;
        }

        if self.has_subpath {
            self.current_path.line_to(point);
        } else {
            self.current_path.move_to(point);
            self.has_subpath = true;
        }
    }

    // https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-quadraticcurveto-dev
    pub fn quadratic_curve_to(&mut self, cx: f64, cy: f64, x: f64, y: f64) {
        let (Some(control), Some(to)) = (self.canvas_point(cx, cy), self.canvas_point(x, y)) else {
            return;
        };

        let needed = if self.has_subpath { 1 } else { 2 };

        if self.current_path.commands().len().saturating_add(needed) > MAX_CANVAS_PATH_COMMANDS {
            return;
        }

        if !self.has_subpath {
            self.current_path.move_to(control);
            self.has_subpath = true;
        }

        self.current_path.quadratic_to(control, to);
    }

    // https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-beziercurveto-dev
    pub fn bezier_curve_to(&mut self, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64) {
        let (Some(control_one), Some(control_two), Some(to)) = (
            self.canvas_point(c1x, c1y),
            self.canvas_point(c2x, c2y),
            self.canvas_point(x, y),
        ) else {
            return;
        };

        let needed = if self.has_subpath { 1 } else { 2 };

        if self.current_path.commands().len().saturating_add(needed) > MAX_CANVAS_PATH_COMMANDS {
            return;
        }

        if !self.has_subpath {
            self.current_path.move_to(control_one);
            self.has_subpath = true;
        }

        self.current_path.cubic_to(control_one, control_two, to);
    }

    pub fn close_path(&mut self) {
        if self.has_subpath && self.current_path.commands().len() < MAX_CANVAS_PATH_COMMANDS {
            self.current_path.close();
        }
    }

    /// Fills without discarding the current path.
    pub fn fill(&mut self, fill_rule: FillRule) {
        let flattened =
            flatten_path(&self.current_path, 0.25).expect("constant flattening tolerance is valid");

        fill_path(&mut self.surface, &flattened, fill_rule, self.fill_style);
    }

    /// Strokes the current path without changing it.
    pub fn stroke(&mut self, color: Color, style: StrokeStyle) {
        let flattened =
            flatten_path(&self.current_path, 0.25).expect("constant flattening tolerance is valid");

        stroke_path_transformed(&mut self.surface, &flattened, style, color, self.transform);
    }

    pub(crate) fn current_transform(&self) -> Transform {
        self.transform
    }

    pub(crate) fn restore_transform(&mut self, transform: Transform) {
        self.transform = transform;
    }

    // https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-translate-dev
    pub fn translate(&mut self, x: f64, y: f64) {
        let (x, y) = (x as f32, y as f32);

        if !x.is_finite() || !y.is_finite() {
            return;
        }

        let transform = Transform::translate(x, y).then(self.transform);

        if [
            transform.a,
            transform.b,
            transform.c,
            transform.d,
            transform.e,
            transform.f,
        ]
        .iter()
        .all(|value| value.is_finite())
        {
            self.transform = transform;
        }
    }

    // https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-rotate-dev
    pub fn rotate(&mut self, angle: f64) {
        let angle = angle as f32;

        if !angle.is_finite() {
            return;
        }

        let transform = Transform::rotate(angle).then(self.transform);

        if [
            transform.a,
            transform.b,
            transform.c,
            transform.d,
            transform.e,
            transform.f,
        ]
        .iter()
        .all(|value| value.is_finite())
        {
            self.transform = transform;
        }
    }

    // https://html.spec.whatwg.org/multipage/canvas.html#dom-context-2d-scale-dev
    pub fn scale(&mut self, x: f64, y: f64) {
        let (x, y) = (x as f32, y as f32);

        if !x.is_finite() || !y.is_finite() {
            return;
        }

        let transform = Transform::scale(x, y).then(self.transform);

        if [
            transform.a,
            transform.b,
            transform.c,
            transform.d,
            transform.e,
            transform.f,
        ]
        .iter()
        .all(|value| value.is_finite())
        {
            self.transform = transform;
        }
    }

    fn canvas_point(&self, x: f64, y: f64) -> Option<Point> {
        let (x, y) = (x as f32, y as f32);

        if !x.is_finite() || !y.is_finite() {
            return None;
        }

        let point = self.transform.apply_point(Point::new(x, y));

        (point.x.is_finite() && point.y.is_finite()).then_some(point)
    }

    fn has_translation_only(&self) -> bool {
        self.transform.a == 1.0
            && self.transform.b == 0.0
            && self.transform.c == 0.0
            && self.transform.d == 1.0
    }

    fn transformed_rect(&self, x: f64, y: f64, width: f64, height: f64) -> Option<[Point; 4]> {
        if ![x, y, width, height].iter().all(|value| value.is_finite())
            || width == 0.0
            || height == 0.0
        {
            return None;
        }

        let matrix = self.transform;

        let determinant =
            f64::from(matrix.a) * f64::from(matrix.d) - f64::from(matrix.b) * f64::from(matrix.c);

        if determinant == 0.0 {
            return None;
        }

        Some([
            self.canvas_point(x, y)?,
            self.canvas_point(x + width, y)?,
            self.canvas_point(x + width, y + height)?,
            self.canvas_point(x, y + height)?,
        ])
    }

    fn clear_quad(&mut self, points: [Point; 4]) {
        let min_x = points
            .iter()
            .map(|point| f64::from(point.x))
            .fold(f64::INFINITY, f64::min);

        let max_x = points
            .iter()
            .map(|point| f64::from(point.x))
            .fold(f64::NEG_INFINITY, f64::max);

        let min_y = points
            .iter()
            .map(|point| f64::from(point.y))
            .fold(f64::INFINITY, f64::min);

        let max_y = points
            .iter()
            .map(|point| f64::from(point.y))
            .fold(f64::NEG_INFINITY, f64::max);

        let start_x = min_x.floor().clamp(0.0, self.surface.width() as f64) as usize;
        let end_x = max_x.ceil().clamp(0.0, self.surface.width() as f64) as usize;
        let start_y = min_y.floor().clamp(0.0, self.surface.height() as f64) as usize;
        let end_y = max_y.ceil().clamp(0.0, self.surface.height() as f64) as usize;

        for py in start_y..end_y {
            for px in start_x..end_x {
                let covered = SAMPLE_OFFSETS
                    .iter()
                    .filter(|(dx, dy)| point_in_quad(&points, px as f64 + dx, py as f64 + dy))
                    .count();

                self.clear_pixel(px, py, covered);
            }
        }
    }

    fn clear_pixel(&mut self, x: usize, y: usize, covered: usize) {
        if covered == 0 {
            return;
        }

        let Some(mut color) = self.surface.pixel(x, y) else {
            return;
        };

        color.alpha = ((u16::from(color.alpha) * (4 - covered) as u16 + 2) / 4) as u8;

        if color.alpha == 0 {
            color = Color::TRANSPARENT;
        }

        self.surface.set_pixel(x, y, color);
    }
}

fn point_in_quad(points: &[Point; 4], x: f64, y: f64) -> bool {
    let mut positive = false;
    let mut negative = false;

    for index in 0..4 {
        let a = points[index];
        let b = points[(index + 1) % 4];

        let cross = (f64::from(b.x) - f64::from(a.x)) * (y - f64::from(a.y))
            - (f64::from(b.y) - f64::from(a.y)) * (x - f64::from(a.x));

        positive |= cross > 0.0;
        negative |= cross < 0.0;

        if positive && negative {
            return false;
        }
    }

    true
}

struct Rect {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

fn valid_size(width: usize, height: usize) -> bool {
    width <= MAX_CANVAS_DIMENSION
        && height <= MAX_CANVAS_DIMENSION
        && width
            .checked_mul(height)
            .is_some_and(|pixels| pixels <= MAX_CANVAS_PIXELS)
}

// Partial HTML non-negative integer parser for canvas dimensions.
// https://html.spec.whatwg.org/multipage/common-microsyntaxes.html#rules-for-parsing-non-negative-integers
pub(crate) fn parse_canvas_dimension(input: &str) -> Option<usize> {
    let input = input.trim_start_matches(|ch| matches!(ch, ' ' | '\t' | '\n' | '\r' | '\x0c'));

    let (negative, digits) = if let Some(rest) = input.strip_prefix('-') {
        (true, rest)
    } else {
        (false, input.strip_prefix('+').unwrap_or(input))
    };

    let mut value = 0_usize;
    let mut found = false;

    for digit in digits.bytes().take_while(u8::is_ascii_digit) {
        found = true;

        value = value
            .saturating_mul(10)
            .saturating_add(usize::from(digit - b'0'));
    }

    if !found || (negative && value != 0) {
        None
    } else {
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_rectangles_clip_blend_clear_and_reset() {
        let mut canvas = Canvas2D::new(4, 3).unwrap();

        assert_eq!(canvas.surface().pixel(0, 0), Some(Color::TRANSPARENT));
        assert_eq!(canvas.fill_style(), Color::BLACK);

        canvas.set_fill_style(Color::rgba(255, 0, 0, 255));
        canvas.fill_rect(3.0, 2.0, -3.0, -2.0);

        assert_eq!(
            canvas.surface.pixel(0, 0),
            Some(Color::rgba(255, 0, 0, 255))
        );

        assert_eq!(canvas.surface.pixel(3, 0), Some(Color::TRANSPARENT));

        canvas.set_fill_style(Color::rgba(0, 0, 255, 128));
        canvas.fill_rect(-10.0, 0.0, 12.0, 1.0);

        assert_eq!(
            canvas.surface().pixel(0, 0),
            Some(Color::rgba(127, 0, 128, 255))
        );

        canvas.clear_rect(0.0, 0.0, 0.5, 1.0);

        assert_eq!(
            canvas.surface().pixel(0, 0),
            Some(Color::rgba(127, 0, 128, 128))
        );

        canvas.clear_rect(0.0, 0.0, 1.0, 1.0);
        assert_eq!(canvas.surface().pixel(0, 0), Some(Color::TRANSPARENT));

        canvas.set_fill_style(Color::rgba(0, 255, 0, 255));
        canvas.fill_rect(3.5, 2.0, 1.0, 1.0);

        assert_eq!(
            canvas.surface().pixel(3, 2),
            Some(Color::rgba(0, 255, 0, 128))
        );

        let before = canvas.surface().clone();

        canvas.fill_rect(f64::NAN, 0.0, 1.0, 1.0);
        assert_eq!(&before, canvas.surface());

        assert!(!canvas.resize(usize::MAX, 1));
        assert_eq!(&before, canvas.surface());

        assert!(canvas.resize(4, 3));
        assert_eq!(canvas.surface().pixel(1, 1), Some(Color::TRANSPARENT));
        assert_eq!(canvas.fill_style(), Color::BLACK);

        assert!(Canvas2D::new(0, 0).is_some());
    }
}
