use crate::graphics::flatten::FlattenedPath;
use crate::graphics::geometry::Point;

const SAMPLE_OFFSETS: [(f32, f32); 4] = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Color {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

impl Color {
    pub const TRANSPARENT: Self = Self::rgba(0, 0, 0, 0);
    pub const BLACK: Self = Self::rgba(0, 0, 0, 255);
    pub const WHITE: Self = Self::rgba(255, 255, 255, 255);

    pub const fn rgba(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FillRule {
    EvenOdd,

    #[default]
    NonZero,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokeStyle {
    pub width: f32,
    pub line_cap: StrokeLineCap,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StrokeLineCap {
    #[default]
    Butt,
    Round,
    Square,
}

impl Default for StrokeStyle {
    fn default() -> Self {
        Self {
            width: 1.0,
            line_cap: StrokeLineCap::Butt,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Surface {
    width: usize,
    height: usize,
    pixels: Vec<Color>,
}

impl Surface {
    pub fn new(width: usize, height: usize, clear_color: Color) -> Self {
        Self {
            width,
            height,
            pixels: vec![clear_color; width.saturating_mul(height)],
        }
    }

    pub fn pixels(&self) -> &[Color] {
        &self.pixels
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn pixel(&self, x: usize, y: usize) -> Option<Color> {
        self.index(x, y).map(|index| self.pixels[index])
    }

    pub fn clear(&mut self, color: Color) {
        self.pixels.fill(color);
    }

    pub fn set_pixel(&mut self, x: usize, y: usize, color: Color) -> bool {
        let Some(index) = self.index(x, y) else {
            return false;
        };

        self.pixels[index] = color;
        true
    }

    pub fn blit_xrgb8888(
        &self,
        destination: &mut [u32],
        destination_width: usize,
        destination_height: usize,
        origin_x: i32,
        origin_y: i32,
    ) {
        self.blit_scaled_xrgb8888(
            destination,
            destination_width,
            destination_height,
            origin_x,
            origin_y,
            self.width,
            self.height,
        );
    }

    pub fn blit_scaled_xrgb8888(
        &self,
        destination: &mut [u32],
        destination_width: usize,
        destination_height: usize,
        origin_x: i32,
        origin_y: i32,
        target_width: usize,
        target_height: usize,
    ) {
        if self.width == 0 || self.height == 0 || target_width == 0 || target_height == 0 {
            return;
        }

        for target_y in 0..target_height {
            let destination_y = origin_y.saturating_add(target_y as i32);

            if destination_y < 0 || destination_y >= destination_height as i32 {
                continue;
            }

            let source_y = target_y * self.height / target_height;

            for target_x in 0..target_width {
                let destination_x = origin_x.saturating_add(target_x as i32);

                if destination_x < 0 || destination_x >= destination_width as i32 {
                    continue;
                }

                let source_x = target_x * self.width / target_width;
                let source = self.pixels[source_y * self.width + source_x];

                if source.alpha == 0 {
                    continue;
                }

                let destination_index =
                    destination_y as usize * destination_width + destination_x as usize;

                let Some(destination_pixel) = destination.get_mut(destination_index) else {
                    continue;
                };

                let background = Color::rgba(
                    ((*destination_pixel >> 16) & 0xff) as u8,
                    ((*destination_pixel >> 8) & 0xff) as u8,
                    (*destination_pixel & 0xff) as u8,
                    255,
                );

                let result = blend_source_over(background, source, 1.0);

                *destination_pixel =
                    (result.red as u32) << 16 | (result.green as u32) << 8 | result.blue as u32;
            }
        }
    }

    fn blend_pixel(&mut self, x: usize, y: usize, color: Color, coverage: f32) {
        let Some(index) = self.index(x, y) else {
            return;
        };

        self.pixels[index] = blend_source_over(self.pixels[index], color, coverage);
    }

    fn index(&self, x: usize, y: usize) -> Option<usize> {
        (x < self.width && y < self.height).then_some(y * self.width + x)
    }
}

pub fn fill_path(surface: &mut Surface, path: &FlattenedPath, fill_rule: FillRule, color: Color) {
    fill_path_clipped(surface, path, fill_rule, color, &[])
}

pub fn fill_path_clipped(
    surface: &mut Surface,
    path: &FlattenedPath,
    fill_rule: FillRule,
    color: Color,
    clips: &[(FlattenedPath, FillRule)],
) {
    if color.alpha == 0 {
        return;
    }

    let Some((start_x, end_x, start_y, end_y)) = pixel_bounds(surface, path, 1.0) else {
        return;
    };

    for y in start_y..end_y {
        for x in start_x..end_x {
            let coverage = SAMPLE_OFFSETS
                .iter()
                .filter(|(offset_x, offset_y)| {
                    let sample = Point::new(x as f32 + offset_x, y as f32 + offset_y);

                    sample_is_inside(path, sample, fill_rule)
                        && clips
                        .iter()
                        .all(|(clip, fill_rule)| sample_is_inside(clip, sample, *fill_rule))
                })
                .count() as f32
                / SAMPLE_OFFSETS.len() as f32;

            if coverage > 0.0 {
                surface.blend_pixel(x, y, color, coverage);
            }
        }
    }
}

pub fn stroke_path(surface: &mut Surface, path: &FlattenedPath, style: StrokeStyle, color: Color) {
    stroke_path_clipped(surface, path, style, color, &[])
}

pub fn stroke_path_clipped(
    surface: &mut Surface,
    path: &FlattenedPath,
    style: StrokeStyle,
    color: Color,
    clips: &[(FlattenedPath, FillRule)],
) {
    if color.alpha == 0 || !style.width.is_finite() || style.width <= 0.0 {
        return;
    }

    let radius = style.width * 0.5;

    let Some((start_x, end_x, start_y, end_y)) = pixel_bounds(surface, path, radius + 1.0) else {
        return;
    };

    for y in start_y..end_y {
        for x in start_x..end_x {
            let coverage = SAMPLE_OFFSETS
                .iter()
                .filter(|(offset_x, offset_y)| {
                    let sample = Point::new(x as f32 + offset_x, y as f32 + offset_y);

                    sample_is_on_stroke(path, sample, radius, style.line_cap)
                        && clips
                        .iter()
                        .all(|(clip, fill_rule)| sample_is_inside(clip, sample, *fill_rule))
                })
                .count() as f32
                / SAMPLE_OFFSETS.len() as f32;

            if coverage > 0.0 {
                surface.blend_pixel(x, y, color, coverage);
            }
        }
    }
}

fn pixel_bounds(
    surface: &Surface,
    path: &FlattenedPath,
    padding: f32,
) -> Option<(usize, usize, usize, usize)> {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut found_point = false;

    for contour in path.contours() {
        for point in contour.points() {
            if !point.x.is_finite() || !point.y.is_finite() {
                return None;
            }

            min_x = min_x.min(point.x);
            min_y = min_y.min(point.y);
            max_x = max_x.max(point.x);
            max_y = max_y.max(point.y);
            found_point = true;
        }
    }

    if !found_point {
        return None;
    }

    if !padding.is_finite() || padding < 0.0 {
        return None;
    }

    let start_x = (min_x - padding).floor().max(0.0) as usize;
    let start_y = (min_y - padding).floor().max(0.0) as usize;
    let end_x = (max_x + padding).ceil().max(0.0).min(surface.width as f32) as usize;
    let end_y = (max_y + padding).ceil().max(0.0).min(surface.height as f32) as usize;

    (start_x < end_x && start_y < end_y).then_some((start_x, end_x, start_y, end_y))
}

fn sample_is_on_stroke(
    path: &FlattenedPath,
    sample: Point,
    radius: f32,
    line_cap: StrokeLineCap,
) -> bool {
    for contour in path.contours() {
        let points = contour.points();

        for segment in points.windows(2) {
            if point_is_inside_butt_segment(sample, segment[0], segment[1], radius) {
                return true;
            }
        }

        if contour.is_closed() && points.len() > 1 {
            if point_is_inside_butt_segment(sample, points[points.len() - 1], points[0], radius) {
                return true;
            }

            continue;
        }

        if points.len() < 2 {
            continue;
        }

        let start = points[0];
        let second = points[1];
        let end = points[points.len() - 1];
        let before_end = points[points.len() - 2];

        match line_cap {
            StrokeLineCap::Butt => {}

            StrokeLineCap::Round => {
                if point_is_inside_round_cap(sample, start, radius)
                    || point_is_inside_round_cap(sample, end, radius)
                {
                    return true;
                }
            }

            StrokeLineCap::Square => {
                if point_is_inside_square_cap(sample, start, second, radius, true)
                    || point_is_inside_square_cap(sample, before_end, end, radius, false)
                {
                    return true;
                }
            }
        }
    }

    false
}

fn point_is_inside_round_cap(point: Point, center: Point, radius: f32) -> bool {
    let delta_x = point.x - center.x;
    let delta_y = point.y - center.y;

    delta_x * delta_x + delta_y * delta_y <= radius * radius
}

fn point_is_inside_square_cap(
    point: Point,
    edge_start: Point,
    edge_end: Point,
    radius: f32,
    at_start: bool,
) -> bool {
    let delta_x = edge_end.x - edge_start.x;
    let delta_y = edge_end.y - edge_start.y;
    let length = (delta_x * delta_x + delta_y * delta_y).sqrt();

    if length <= f32::EPSILON {
        return false;
    }

    let direction_x = delta_x / length;
    let direction_y = delta_y / length;

    let (from, to) = if at_start {
        (
            Point::new(
                edge_start.x - direction_x * radius,
                edge_start.y - direction_y * radius,
            ),
            edge_start,
        )
    } else {
        (
            edge_end,
            Point::new(
                edge_end.x + direction_x * radius,
                edge_end.y + direction_y * radius,
            ),
        )
    };

    point_is_inside_butt_segment(point, from, to, radius)
}

fn point_is_inside_butt_segment(point: Point, from: Point, to: Point, radius: f32) -> bool {
    let delta_x = to.x - from.x;
    let delta_y = to.y - from.y;
    let length_squared = delta_x * delta_x + delta_y * delta_y;

    if length_squared <= f32::EPSILON {
        return false;
    }

    let projection = ((point.x - from.x) * delta_x + (point.y - from.y) * delta_y) / length_squared;

    if !(0.0..=1.0).contains(&projection) {
        return false;
    }

    let cross_product = (point.x - from.x) * delta_y - (point.y - from.y) * delta_x;
    let distance = cross_product.abs() / length_squared.sqrt();

    distance <= radius
}

fn sample_is_inside(path: &FlattenedPath, sample: Point, fill_rule: FillRule) -> bool {
    let mut winding_number = 0_i32;
    let mut crossings = 0_u32;

    for contour in path.contours() {
        let points = contour.points();

        if points.len() < 2 {
            continue;
        }

        for index in 0..points.len() {
            let from = points[index];
            let to = points[(index + 1) % points.len()];

            if !ray_crosses_segment(sample, from, to) {
                continue;
            }

            crossings += 1;

            if to.y > from.y {
                winding_number += 1;
            } else {
                winding_number -= 1;
            }
        }
    }

    match fill_rule {
        FillRule::EvenOdd => crossings % 2 == 1,
        FillRule::NonZero => winding_number != 0,
    }
}

fn ray_crosses_segment(sample: Point, from: Point, to: Point) -> bool {
    let crosses_scanline =
        (from.y <= sample.y && to.y > sample.y) || (to.y <= sample.y && from.y > sample.y);

    if !crosses_scanline {
        return false;
    }

    let crossing_x = from.x + (sample.y - from.y) * (to.x - from.x) / (to.y - from.y);

    crossing_x > sample.x
}

fn blend_source_over(destination: Color, source: Color, coverage: f32) -> Color {
    let source_alpha = source.alpha as f32 / 255.0 * coverage;
    let destination_alpha = destination.alpha as f32 / 255.0;
    let output_alpha = source_alpha + destination_alpha * (1.0 - source_alpha);

    if output_alpha <= f32::EPSILON {
        return Color::TRANSPARENT;
    }

    let red = (source.red as f32 / 255.0 * source_alpha
        + destination.red as f32 / 255.0 * destination_alpha * (1.0 - source_alpha))
        / output_alpha;

    let green = (source.green as f32 / 255.0 * source_alpha
        + destination.green as f32 / 255.0 * destination_alpha * (1.0 - source_alpha))
        / output_alpha;

    let blue = (source.blue as f32 / 255.0 * source_alpha
        + destination.blue as f32 / 255.0 * destination_alpha * (1.0 - source_alpha))
        / output_alpha;

    Color::rgba(
        (red * 255.0).round() as u8,
        (green * 255.0).round() as u8,
        (blue * 255.0).round() as u8,
        (output_alpha * 255.0).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::flatten::flatten_path;
    use crate::graphics::geometry::Path;

    #[test]
    fn fills_a_closed_rectangle() {
        let mut path = Path::new();
        path.move_to(Point::new(1.0, 1.0));
        path.line_to(Point::new(4.0, 1.0));
        path.line_to(Point::new(4.0, 4.0));
        path.line_to(Point::new(1.0, 4.0));

        let flattened = flatten_path(&path, 0.25).unwrap();
        let mut surface = Surface::new(6, 6, Color::TRANSPARENT);

        fill_path(
            &mut surface,
            &flattened,
            FillRule::NonZero,
            Color::rgba(20, 40, 60, 255),
        );

        assert_eq!(surface.pixel(0, 0), Some(Color::TRANSPARENT));
        assert_eq!(surface.pixel(2, 2), Some(Color::rgba(20, 40, 60, 255)));
    }

    #[test]
    fn distinguishes_nonzero_and_evenodd_fill_rules() {
        let mut path = Path::new();

        path.move_to(Point::new(1.0, 1.0));
        path.line_to(Point::new(7.0, 1.0));
        path.line_to(Point::new(7.0, 7.0));
        path.line_to(Point::new(1.0, 7.0));
        path.close();

        path.move_to(Point::new(3.0, 3.0));
        path.line_to(Point::new(5.0, 3.0));
        path.line_to(Point::new(5.0, 5.0));
        path.line_to(Point::new(3.0, 5.0));
        path.close();

        let flattened = flatten_path(&path, 0.25).unwrap();

        let mut nonzero = Surface::new(8, 8, Color::TRANSPARENT);
        fill_path(&mut nonzero, &flattened, FillRule::NonZero, Color::BLACK);

        let mut evenodd = Surface::new(8, 8, Color::TRANSPARENT);
        fill_path(&mut evenodd, &flattened, FillRule::EvenOdd, Color::BLACK);

        assert_eq!(nonzero.pixel(4, 4), Some(Color::BLACK));
        assert_eq!(evenodd.pixel(4, 4), Some(Color::TRANSPARENT));
    }

    #[test]
    fn blits_rgba_surface_onto_xrgb8888_pixels() {
        let mut surface = Surface::new(2, 1, Color::TRANSPARENT);

        assert!(surface.set_pixel(0, 0, Color::rgba(255, 0, 0, 128)));
        assert!(!surface.set_pixel(2, 0, Color::BLACK));

        let mut destination = vec![0x00_00_00_ff, 0x00_ff_ff_ff];

        surface.blit_xrgb8888(&mut destination, 2, 1, 0, 0);

        assert_eq!(destination, vec![0x00_80_00_7f, 0x00_ff_ff_ff]);
    }

    #[test]
    fn strokes_open_path_segments() {
        let mut path = Path::new();
        path.move_to(Point::new(1.0, 3.0));
        path.line_to(Point::new(5.0, 3.0));

        let flattened = flatten_path(&path, 0.25).unwrap();
        let mut surface = Surface::new(7, 7, Color::TRANSPARENT);

        stroke_path(
            &mut surface,
            &flattened,
            StrokeStyle {
                width: 2.0,
                ..StrokeStyle::default()
            },
            Color::BLACK,
        );

        assert_eq!(surface.pixel(3, 3), Some(Color::BLACK));
        assert_eq!(surface.pixel(0, 3), Some(Color::TRANSPARENT));
    }

    #[test]
    fn clips_filled_paths() {
        let mut painted_path = Path::new();
        painted_path.move_to(Point::new(0.0, 0.0));
        painted_path.line_to(Point::new(8.0, 0.0));
        painted_path.line_to(Point::new(8.0, 8.0));
        painted_path.line_to(Point::new(0.0, 8.0));
        painted_path.close();

        let mut clip_path = Path::new();
        clip_path.move_to(Point::new(2.0, 2.0));
        clip_path.line_to(Point::new(6.0, 2.0));
        clip_path.line_to(Point::new(6.0, 6.0));
        clip_path.line_to(Point::new(2.0, 6.0));
        clip_path.close();

        let painted_path = flatten_path(&painted_path, 0.25).unwrap();
        let clip_path = flatten_path(&clip_path, 0.25).unwrap();

        let mut surface = Surface::new(8, 8, Color::TRANSPARENT);

        fill_path_clipped(
            &mut surface,
            &painted_path,
            FillRule::NonZero,
            Color::BLACK,
            &[(clip_path, FillRule::NonZero)],
        );

        assert_eq!(surface.pixel(1, 3), Some(Color::TRANSPARENT));
        assert_eq!(surface.pixel(3, 3), Some(Color::BLACK));
        assert_eq!(surface.pixel(6, 3), Some(Color::TRANSPARENT))
    }

    #[test]
    fn renders_round_line_caps() {
        let mut path = Path::new();
        path.move_to(Point::new(3.0, 4.0));
        path.line_to(Point::new(7.0, 4.0));

        let flattened = flatten_path(&path, 0.25).unwrap();

        let mut butt = Surface::new(10, 10, Color::TRANSPARENT);
        stroke_path(
            &mut butt,
            &flattened,
            StrokeStyle {
                width: 2.0,
                ..StrokeStyle::default()
            },
            Color::BLACK,
        );

        let mut round = Surface::new(10, 10, Color::TRANSPARENT);
        stroke_path(
            &mut round,
            &flattened,
            StrokeStyle {
                width: 2.0,
                line_cap: StrokeLineCap::Round,
            },
            Color::BLACK,
        );

        assert_eq!(butt.pixel(2, 4), Some(Color::TRANSPARENT));
        assert_ne!(round.pixel(2, 4), Some(Color::TRANSPARENT));
    }

    #[test]
    fn renders_square_line_caps() {
        let mut path = Path::new();
        path.move_to(Point::new(4.0, 3.0));
        path.line_to(Point::new(4.0, 7.0));

        let flattened = flatten_path(&path, 0.25).unwrap();

        let mut butt = Surface::new(10, 10, Color::TRANSPARENT);
        stroke_path(
            &mut butt,
            &flattened,
            StrokeStyle {
                width: 2.0,
                ..StrokeStyle::default()
            },
            Color::BLACK,
        );

        let mut square = Surface::new(10, 10, Color::TRANSPARENT);
        stroke_path(
            &mut square,
            &flattened,
            StrokeStyle {
                width: 2.0,
                line_cap: StrokeLineCap::Square,
            },
            Color::BLACK,
        );

        assert_eq!(butt.pixel(4, 7), Some(Color::TRANSPARENT));
        assert_ne!(square.pixel(4, 7), Some(Color::TRANSPARENT));
    }

    #[test]
    fn scales_surfaces_when_blitting() {
        let mut source = Surface::new(2, 1, Color::TRANSPARENT);
        source.set_pixel(0, 0, Color::rgba(255, 0, 0, 255));
        source.set_pixel(1, 0, Color::rgba(0, 255, 0, 255));

        let mut destination = vec![0; 8];

        source.blit_scaled_xrgb8888(&mut destination, 4, 2, 0, 0, 4, 2);

        assert_eq!(destination[0], 0x00_ff_00_00);
        assert_eq!(destination[1], 0x00_ff_00_00);
        assert_eq!(destination[2], 0x00_00_ff_00);
        assert_eq!(destination[3], 0x00_00_ff_00);
    }
}
