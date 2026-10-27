//! Rasterization of parsed SVG documents.

use super::{SvgDocument, flattened_path_length, stroke_scale, view_box_transform};

use crate::graphics::{
    flatten::{FlattenError, flatten_path},
    geometry::Transform,
    raster::{
        Color, LinearGradient, Paint, StrokeStyle, Surface, fill_path_clipped,
        fill_path_with_paint_clipped, stroke_path_clipped,
    },
};

fn transform_paint_for_viewport(paint: &Paint, transform: Transform) -> Option<Paint> {
    match paint {
        Paint::Solid(color) => Some(Paint::Solid(*color)),

        Paint::LinearGradient(gradient) => LinearGradient::with_spread(
            transform.apply_point(gradient.start),
            transform.apply_point(gradient.end),
            gradient.stops.clone(),
            gradient.spread,
        )
        .map(Paint::LinearGradient),
    }
}

impl SvgDocument {
    pub fn render(
        &self,
        width: usize,
        height: usize,
        background: Color,
    ) -> Result<Surface, FlattenError> {
        let mut surface = Surface::new(width, height, background);

        if self.view_box_disables_rendering {
            return Ok(surface);
        }

        let viewport_transform = self
            .view_box
            .map(|view_box| {
                view_box_transform(
                    view_box,
                    self.preserve_aspect_ratio,
                    width as f32,
                    height as f32,
                )
            })
            .unwrap_or(Transform::IDENTITY);

        let viewport_stroke_scale = stroke_scale(viewport_transform);

        for shape in &self.shapes {
            let transformed_path = shape.path.transformed(viewport_transform);
            let flattened = flatten_path(&transformed_path, 0.25)?;

            let clip_paths = shape
                .clip_paths
                .iter()
                .map(|clip_path| {
                    flatten_path(&clip_path.path.transformed(viewport_transform), 0.25)
                        .map(|path| (path, clip_path.fill_rule))
                })
                .collect::<Result<Vec<_>, _>>()?;

            if let Some(paint) = &shape.fill_paint {
                let paint = transform_paint_for_viewport(paint, viewport_transform)
                    .expect("validated SVG paint must stay valid after viewport transform");

                fill_path_with_paint_clipped(
                    &mut surface,
                    &flattened,
                    shape.fill_rule,
                    &paint,
                    &clip_paths,
                );
            } else if let Some(fill) = shape.fill {
                fill_path_clipped(&mut surface, &flattened, shape.fill_rule, fill, &clip_paths);
            }

            if let Some(stroke) = shape.stroke {
                let shape_stroke_scale = shape.stroke_transform_scale * viewport_stroke_scale;
                let dash_scale = match shape.path_length {
                    Some(path_length) => {
                        let actual_length = flattened_path_length(&flattened);

                        if actual_length.is_finite() && actual_length > 0.0 {
                            actual_length / path_length
                        } else {
                            0.0
                        }
                    }

                    None => shape_stroke_scale,
                };

                stroke_path_clipped(
                    &mut surface,
                    &flattened,
                    StrokeStyle {
                        width: shape.stroke_width * shape_stroke_scale,
                        line_cap: shape.stroke_line_cap,
                        line_join: shape.stroke_line_join,
                        miter_limit: shape.stroke_miter_limit,
                        dash_array: shape
                            .stroke_dash_array
                            .iter()
                            .map(|length| length * dash_scale)
                            .collect(),
                        dash_offset: shape.stroke_dash_offset * dash_scale,
                    },
                    stroke,
                    &clip_paths,
                );
            }
        }

        Ok(surface)
    }
}
