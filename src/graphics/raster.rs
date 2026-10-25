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

#[derive(Clone, Debug, PartialEq)]
pub struct GradientStop {
    pub offset: f32,
    pub color: Color,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GradientSpread {
    Pad,
    Repeat,
    Reflect,
}

impl Default for GradientSpread {
    fn default() -> Self {
        Self::Pad
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LinearGradient {
    pub start: Point,
    pub end: Point,
    pub stops: Vec<GradientStop>,
    pub spread: GradientSpread,
}

impl LinearGradient {
    pub fn new(start: Point, end: Point, stops: Vec<GradientStop>) -> Option<Self> {
        Self::with_spread(start, end, stops, GradientSpread::Pad)
    }

    pub fn with_spread(
        start: Point,
        end: Point,
        mut stops: Vec<GradientStop>,
        spread: GradientSpread,
    ) -> Option<Self> {
        if !start.x.is_finite()
            || !start.y.is_finite()
            || !end.x.is_finite()
            || !end.y.is_finite()
            || stops.is_empty()
            || stops
                .iter()
                .any(|stop| !stop.offset.is_finite() || !(0.0..=1.0).contains(&stop.offset))
        {
            return None;
        }

        stops.sort_by(|left, right| left.offset.total_cmp(&right.offset));

        Some(Self {
            start,
            end,
            stops,
            spread,
        })
    }

    pub fn sample(&self, point: Point) -> Color {
        let delta_x = self.end.x - self.start.x;
        let delta_y = self.end.y - self.start.y;
        let length_squared = delta_x * delta_x + delta_y * delta_y;

        if length_squared <= f32::EPSILON {
            return self.stops.last().expect("gradient has stops").color;
        }

        let position = ((point.x - self.start.x) * delta_x + (point.y - self.start.y) * delta_y)
            / length_squared;

        let position = match self.spread {
            GradientSpread::Pad => position.clamp(0.0, 1.0),
            GradientSpread::Repeat => position.rem_euclid(1.0),
            GradientSpread::Reflect => {
                let position = position.rem_euclid(2.0);

                if position > 1.0 {
                    2.0 - position
                } else {
                    position
                }
            }
        };

        if position <= self.stops[0].offset {
            return self.stops[0].color;
        }

        for stops in self.stops.windows(2) {
            let start = &stops[0];
            let end = &stops[1];

            if position <= end.offset {
                let span = end.offset - start.offset;
                let progress = if span <= f32::EPSILON {
                    1.0
                } else {
                    (position - start.offset) / span
                };

                return interpolate_color(start.color, end.color, progress);
            }
        }

        self.stops.last().expect("gradient has stops").color
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    Solid(Color),
    LinearGradient(LinearGradient),
}

impl Paint {
    fn color_at(&self, point: Point) -> Color {
        match self {
            Self::Solid(color) => *color,
            Self::LinearGradient(gradient) => gradient.sample(point),
        }
    }
}

fn interpolate_color(from: Color, to: Color, progress: f32) -> Color {
    let component = |from: u8, to: u8| {
        (from as f32 + (to as f32 - from as f32) * progress)
            .round()
            .clamp(0.0, 255.0) as u8
    };

    Color::rgba(
        component(from.red, to.red),
        component(from.green, to.green),
        component(from.blue, to.blue),
        component(from.alpha, to.alpha),
    )
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FillRule {
    EvenOdd,

    #[default]
    NonZero,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StrokeStyle {
    pub width: f32,
    pub line_cap: StrokeLineCap,
    pub line_join: StrokeLineJoin,
    pub miter_limit: f32,
    pub dash_array: Vec<f32>,
    pub dash_offset: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StrokeLineCap {
    #[default]
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StrokeLineJoin {
    #[default]
    Miter,
    Round,
    Bevel,
}

impl Default for StrokeStyle {
    fn default() -> Self {
        Self {
            width: 1.0,
            line_cap: StrokeLineCap::Butt,
            line_join: StrokeLineJoin::Miter,
            miter_limit: 4.0,
            dash_array: Vec::new(),
            dash_offset: 0.0,
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
    fill_path_with_paint(surface, path, fill_rule, &Paint::Solid(color));
}

pub fn fill_path_with_paint(
    surface: &mut Surface,
    path: &FlattenedPath,
    fill_rule: FillRule,
    paint: &Paint,
) {
    fill_path_with_paint_clipped(surface, path, fill_rule, paint, &[]);
}

pub fn fill_path_clipped(
    surface: &mut Surface,
    path: &FlattenedPath,
    fill_rule: FillRule,
    color: Color,
    clips: &[(FlattenedPath, FillRule)],
) {
    fill_path_with_paint_clipped(surface, path, fill_rule, &Paint::Solid(color), clips);
}

pub fn fill_path_with_paint_clipped(
    surface: &mut Surface,
    path: &FlattenedPath,
    fill_rule: FillRule,
    paint: &Paint,
    clips: &[(FlattenedPath, FillRule)],
) {
    let Some((start_x, end_x, start_y, end_y)) = pixel_bounds(surface, path, 1.0) else {
        return;
    };

    let fill_edges = FillEdges::from_path(path);
    let mut upper_edges = Vec::new();
    let mut lower_edges = Vec::new();

    for y in start_y..end_y {
        fill_edges.collect_at_scanline(y as f32 + 0.25, &mut upper_edges);
        fill_edges.collect_at_scanline(y as f32 + 0.75, &mut lower_edges);

        for x in start_x..end_x {
            let coverage = SAMPLE_OFFSETS
                .iter()
                .filter(|(offset_x, offset_y)| {
                    let sample = Point::new(x as f32 + offset_x, y as f32 + offset_y);
                    let edges = if *offset_y < 0.5 {
                        &upper_edges
                    } else {
                        &lower_edges
                    };

                    sample_is_inside_edges(edges, sample, fill_rule)
                        && clips
                            .iter()
                            .all(|(clip, fill_rule)| sample_is_inside(clip, sample, *fill_rule))
                })
                .count() as f32
                / SAMPLE_OFFSETS.len() as f32;

            let color = paint.color_at(Point::new(x as f32 + 0.5, y as f32 + 0.5));

            if coverage > 0.0 && color.alpha > 0 {
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

    let dashed_path = dash_path(path, &style.dash_array, style.dash_offset);
    let path = dashed_path.as_ref().unwrap_or(path);
    let radius = style.width * 0.5;
    let miter_limit = sanitized_miter_limit(style.miter_limit);

    let join_padding = match style.line_join {
        StrokeLineJoin::Miter => radius * miter_limit,
        StrokeLineJoin::Round | StrokeLineJoin::Bevel => radius,
    };

    let Some((start_x, end_x, start_y, end_y)) = pixel_bounds(surface, path, join_padding + 1.0)
    else {
        return;
    };

    for y in start_y..end_y {
        for x in start_x..end_x {
            let coverage = SAMPLE_OFFSETS
                .iter()
                .filter(|(offset_x, offset_y)| {
                    let sample = Point::new(x as f32 + offset_x, y as f32 + offset_y);

                    sample_is_on_stroke(
                        path,
                        sample,
                        radius,
                        style.line_cap,
                        style.line_join,
                        miter_limit,
                    ) && clips
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

fn dash_path(path: &FlattenedPath, dash_array: &[f32], dash_offset: f32) -> Option<FlattenedPath> {
    let dash_array = normalized_dash_array(dash_array)?;
    let mut output = Vec::new();

    for contour in path.contours() {
        let points = contour.points();

        if points.len() < 2 {
            continue;
        }

        let mut state = DashState::new(&dash_array, dash_offset);
        let mut visible_contour = Vec::new();
        let segment_count = points.len() - 1 + usize::from(contour.is_closed());

        for index in 0..segment_count {
            let from = points[index];
            let to = points[(index + 1) % points.len()];

            append_dashed_segment(from, to, &mut state, &mut visible_contour, &mut output);
        }

        finish_dash_contour(&mut visible_contour, &mut output);
    }

    Some(FlattenedPath::from_open_contours(output))
}

fn normalized_dash_array(dash_array: &[f32]) -> Option<Vec<f32>> {
    if dash_array.is_empty()
        || dash_array
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
    {
        return None;
    }

    let mut normalized = dash_array.to_vec();

    if normalized.len() % 2 == 1 {
        normalized.extend_from_within(..);
    }

    Some(normalized)
}

struct DashState<'a> {
    pattern: &'a [f32],
    index: usize,
    remaining: f32,
}

impl<'a> DashState<'a> {
    fn new(pattern: &'a [f32], offset: f32) -> Self {
        let total_length: f32 = pattern.iter().sum();
        let mut position = if offset.is_finite() {
            offset.rem_euclid(total_length)
        } else {
            0.0
        };

        let mut index = 0;

        while position >= pattern[index] {
            position -= pattern[index];
            index = (index + 1) % pattern.len();
        }

        Self {
            pattern,
            index,
            remaining: pattern[index] - position,
        }
    }

    fn is_visible(&self) -> bool {
        self.index % 2 == 0
    }

    fn advance(&mut self, amount: f32) {
        self.remaining -= amount;

        if self.remaining > 0.000_01 {
            return;
        }

        self.index = (self.index + 1) % self.pattern.len();
        self.remaining = self.pattern[self.index];
    }
}

fn append_dashed_segment(
    from: Point,
    to: Point,
    state: &mut DashState<'_>,
    visible_contour: &mut Vec<Point>,
    output: &mut Vec<Vec<Point>>,
) {
    let delta_x = to.x - from.x;
    let delta_y = to.y - from.y;
    let length = (delta_x * delta_x + delta_y * delta_y).sqrt();

    if length <= f32::EPSILON {
        return;
    }

    let direction_x = delta_x / length;
    let direction_y = delta_y / length;
    let mut consumed = 0.0;

    while consumed < length {
        let piece_length = (length - consumed).min(state.remaining);
        let piece_start = Point::new(
            from.x + direction_x * consumed,
            from.y + direction_y * consumed,
        );
        let piece_end = Point::new(
            from.x + direction_x * (consumed + piece_length),
            from.y + direction_y * (consumed + piece_length),
        );

        if state.is_visible() {
            if visible_contour.last().copied() != Some(piece_start) {
                finish_dash_contour(visible_contour, output);
                visible_contour.push(piece_start);
            }

            if visible_contour.last().copied() != Some(piece_end) {
                visible_contour.push(piece_end);
            }
        } else {
            finish_dash_contour(visible_contour, output);
        }

        consumed += piece_length;
        state.advance(piece_length);
    }
}

fn finish_dash_contour(contour: &mut Vec<Point>, output: &mut Vec<Vec<Point>>) {
    if contour.len() >= 2 {
        output.push(std::mem::take(contour));
    } else {
        contour.clear();
    }
}

fn sanitized_miter_limit(value: f32) -> f32 {
    if value.is_finite() {
        value.max(1.0)
    } else {
        1.0
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
    line_join: StrokeLineJoin,
    miter_limit: f32,
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

        if points.len() >= 3 {
            if contour.is_closed() {
                for index in 0..points.len() {
                    let previous = points[(index + points.len() - 1) % points.len()];
                    let vertex = points[index];
                    let next = points[(index + 1) % points.len()];

                    if point_is_inside_line_join(
                        sample,
                        previous,
                        vertex,
                        next,
                        radius,
                        line_join,
                        miter_limit,
                    ) {
                        return true;
                    }
                }
            } else {
                for points in points.windows(3) {
                    if point_is_inside_line_join(
                        sample,
                        points[0],
                        points[1],
                        points[2],
                        radius,
                        line_join,
                        miter_limit,
                    ) {
                        return true;
                    }
                }
            }
        }

        if contour.is_closed() || points.len() < 2 {
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

fn point_is_inside_line_join(
    sample: Point,
    previous: Point,
    vertex: Point,
    next: Point,
    radius: f32,
    line_join: StrokeLineJoin,
    miter_limit: f32,
) -> bool {
    let Some((incoming_x, incoming_y)) = unit_direction(previous, vertex) else {
        return false;
    };
    let Some((outgoing_x, outgoing_y)) = unit_direction(vertex, next) else {
        return false;
    };

    let turn = incoming_x * outgoing_y - incoming_y * outgoing_x;

    if turn.abs() <= f32::EPSILON {
        return false;
    }

    if line_join == StrokeLineJoin::Round {
        return point_is_inside_round_cap(sample, vertex, radius);
    }

    let (incoming_normal_x, incoming_normal_y) = outer_normal(incoming_x, incoming_y, turn);
    let (outgoing_normal_x, outgoing_normal_y) = outer_normal(outgoing_x, outgoing_y, turn);

    let outer_from = Point::new(
        vertex.x + incoming_normal_x * radius,
        vertex.y + incoming_normal_y * radius,
    );
    let outer_to = Point::new(
        vertex.x + outgoing_normal_x * radius,
        vertex.y + outgoing_normal_y * radius,
    );

    if line_join == StrokeLineJoin::Bevel {
        return point_is_inside_triangle(sample, vertex, outer_from, outer_to);
    }

    let Some(miter) = line_intersection(
        outer_from,
        (incoming_x, incoming_y),
        outer_to,
        (outgoing_x, outgoing_y),
    ) else {
        return point_is_inside_triangle(sample, vertex, outer_from, outer_to);
    };

    let distance_x = miter.x - vertex.x;
    let distance_y = miter.y - vertex.y;
    let miter_length = (distance_x * distance_x + distance_y * distance_y).sqrt();

    if miter_length > radius * miter_limit {
        return point_is_inside_triangle(sample, vertex, outer_from, outer_to);
    }

    point_is_inside_triangle(sample, outer_from, miter, outer_to)
}

fn unit_direction(from: Point, to: Point) -> Option<(f32, f32)> {
    let delta_x = to.x - from.x;
    let delta_y = to.y - from.y;
    let length = (delta_x * delta_x + delta_y * delta_y).sqrt();

    (length > f32::EPSILON).then_some((delta_x / length, delta_y / length))
}

fn outer_normal(direction_x: f32, direction_y: f32, turn: f32) -> (f32, f32) {
    if turn > 0.0 {
        (direction_y, -direction_x)
    } else {
        (-direction_y, direction_x)
    }
}

fn line_intersection(
    first_point: Point,
    first_direction: (f32, f32),
    second_point: Point,
    second_direction: (f32, f32),
) -> Option<Point> {
    let denominator =
        first_direction.0 * second_direction.1 - first_direction.1 * second_direction.0;

    if denominator.abs() <= f32::EPSILON {
        return None;
    }

    let between_x = second_point.x - first_point.x;
    let between_y = second_point.y - first_point.y;
    let distance = (between_x * second_direction.1 - between_y * second_direction.0) / denominator;

    Some(Point::new(
        first_point.x + first_direction.0 * distance,
        first_point.y + first_direction.1 * distance,
    ))
}

fn point_is_inside_triangle(point: Point, first: Point, second: Point, third: Point) -> bool {
    let first_side = oriented_area(first, second, point);
    let second_side = oriented_area(second, third, point);
    let third_side = oriented_area(third, first, point);

    let has_negative = first_side < 0.0 || second_side < 0.0 || third_side < 0.0;
    let has_positive = first_side > 0.0 || second_side > 0.0 || third_side > 0.0;

    !(has_negative && has_positive)
}

fn oriented_area(from: Point, to: Point, point: Point) -> f32 {
    (to.x - from.x) * (point.y - from.y) - (to.y - from.y) * (point.x - from.x)
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

#[derive(Clone, Copy)]
struct FillEdge {
    from: Point,
    to: Point,
}

struct FillEdges {
    edges: Vec<FillEdge>,
}

impl FillEdges {
    fn from_path(path: &FlattenedPath) -> Self {
        let mut edges = Vec::new();

        for contour in path.contours() {
            let points = contour.points();

            if points.len() < 2 {
                continue;
            }

            for index in 0..points.len() {
                edges.push(FillEdge {
                    from: points[index],
                    to: points[(index + 1) % points.len()],
                });
            }
        }

        Self { edges }
    }

    fn collect_at_scanline(&self, scanline_y: f32, output: &mut Vec<FillEdge>) {
        output.clear();
        output.extend(
            self.edges
                .iter()
                .copied()
                .filter(|edge| segment_crosses_scanline(scanline_y, edge.from, edge.to)),
        );
    }
}

fn sample_is_inside_edges(edges: &[FillEdge], sample: Point, fill_rule: FillRule) -> bool {
    let mut winding_number = 0_i32;
    let mut crossings = 0_u32;

    for edge in edges {
        let crossing_x = edge.from.x
            + (sample.y - edge.from.y) * (edge.to.x - edge.from.x) / (edge.to.y - edge.from.y);

        if crossing_x <= sample.x {
            continue;
        }

        crossings += 1;

        if edge.to.y > edge.from.y {
            winding_number += 1;
        } else {
            winding_number -= 1;
        }
    }

    match fill_rule {
        FillRule::EvenOdd => crossings % 2 == 1,
        FillRule::NonZero => winding_number != 0,
    }
}

fn segment_crosses_scanline(scanline_y: f32, from: Point, to: Point) -> bool {
    (from.y <= scanline_y && to.y > scanline_y) || (to.y <= scanline_y && from.y > scanline_y)
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
    if !segment_crosses_scanline(sample.y, from, to) {
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
                ..StrokeStyle::default()
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
                ..StrokeStyle::default()
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

    #[test]
    fn covers_outer_corner_for_round_and_miter_joins() {
        let previous = Point::new(2.0, 6.0);
        let vertex = Point::new(5.0, 2.0);
        let next = Point::new(8.0, 6.0);
        let sample = Point::new(5.0, 0.5);

        assert!(!point_is_inside_line_join(
            sample,
            previous,
            vertex,
            next,
            2.0,
            StrokeLineJoin::Bevel,
            4.0,
        ));

        assert!(point_is_inside_line_join(
            sample,
            previous,
            vertex,
            next,
            2.0,
            StrokeLineJoin::Round,
            4.0,
        ));

        assert!(point_is_inside_line_join(
            sample,
            previous,
            vertex,
            next,
            2.0,
            StrokeLineJoin::Miter,
            4.0,
        ));
    }

    #[test]
    fn renders_dashed_strokes() {
        let mut path = Path::new();
        path.move_to(Point::new(1.0, 5.0));
        path.line_to(Point::new(15.0, 5.0));

        let flattened = flatten_path(&path, 0.25).unwrap();
        let mut surface = Surface::new(20, 10, Color::TRANSPARENT);

        stroke_path(
            &mut surface,
            &flattened,
            StrokeStyle {
                width: 2.0,
                dash_array: vec![3.0, 2.0],
                ..StrokeStyle::default()
            },
            Color::BLACK,
        );

        assert_ne!(surface.pixel(2, 5), Some(Color::TRANSPARENT));
        assert_eq!(surface.pixel(5, 5), Some(Color::TRANSPARENT));
        assert_ne!(surface.pixel(7, 5), Some(Color::TRANSPARENT));
    }

    #[test]
    fn fills_paths_with_a_linear_gradient() {
        let gradient = LinearGradient::new(
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            vec![
                GradientStop {
                    offset: 0.0,
                    color: Color::BLACK,
                },
                GradientStop {
                    offset: 1.0,
                    color: Color::WHITE,
                },
            ],
        )
        .unwrap();

        assert_eq!(gradient.sample(Point::new(0.0, 0.0)), Color::BLACK);
        assert_eq!(
            gradient.sample(Point::new(5.0, 0.0)),
            Color::rgba(128, 128, 128, 255)
        );
        assert_eq!(gradient.sample(Point::new(10.0, 0.0)), Color::WHITE);

        let mut path = Path::new();
        path.move_to(Point::new(0.0, 0.0));
        path.line_to(Point::new(10.0, 0.0));
        path.line_to(Point::new(10.0, 2.0));
        path.line_to(Point::new(0.0, 2.0));
        path.close();

        let flattened = flatten_path(&path, 0.25).unwrap();
        let mut surface = Surface::new(10, 2, Color::TRANSPARENT);

        fill_path_with_paint(
            &mut surface,
            &flattened,
            FillRule::NonZero,
            &Paint::LinearGradient(gradient),
        );

        let left = surface.pixel(1, 1).unwrap();
        let right = surface.pixel(8, 1).unwrap();

        assert!(left.red < right.red);
        assert_eq!(left.red, left.green);
        assert_eq!(right.green, right.blue);
    }

    #[test]
    fn samples_repeating_and_reflecting_linear_gradients() {
        let stops = vec![
            GradientStop {
                offset: 0.0,
                color: Color::rgba(255, 0, 0, 255),
            },
            GradientStop {
                offset: 1.0,
                color: Color::rgba(0, 0, 255, 255),
            },
        ];

        let repeated = LinearGradient::with_spread(
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            stops.clone(),
            GradientSpread::Repeat,
        )
        .unwrap();

        let reflected = LinearGradient::with_spread(
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            stops,
            GradientSpread::Reflect,
        )
        .unwrap();

        let repeated_color = repeated.sample(Point::new(12.0, 0.0));
        let reflected_color = reflected.sample(Point::new(12.0, 0.0));

        assert!(repeated_color.red > repeated_color.blue);
        assert!(reflected_color.blue > reflected_color.red);
    }

    #[test]
    fn active_edges_match_full_path_tests() {
        let mut path = Path::new();
        path.move_to(Point::new(1.0, 1.0));
        path.line_to(Point::new(9.0, 1.0));
        path.line_to(Point::new(9.0, 9.0));
        path.line_to(Point::new(1.0, 9.0));
        path.close();

        path.move_to(Point::new(3.0, 3.0));
        path.line_to(Point::new(7.0, 3.0));
        path.line_to(Point::new(7.0, 7.0));
        path.line_to(Point::new(3.0, 7.0));
        path.close();

        let flattened = flatten_path(&path, 0.25).unwrap();
        let edges = FillEdges::from_path(&flattened);
        let mut active_edges = Vec::new();

        for y in 0..10 {
            for offset_y in [0.25, 0.75] {
                let sample_y = y as f32 + offset_y;
                edges.collect_at_scanline(sample_y, &mut active_edges);

                for x in 0..10 {
                    let sample = Point::new(x as f32 + 0.25, sample_y);

                    for fill_rule in [FillRule::NonZero, FillRule::EvenOdd] {
                        assert_eq!(
                            sample_is_inside(&flattened, sample, fill_rule),
                            sample_is_inside_edges(&active_edges, sample, fill_rule),
                        );
                    }
                }
            }
        }
    }
}
