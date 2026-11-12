use crate::graphics::{
    flatten::{FlattenError, flatten_path},
    raster::{Color, FillRule, Surface, fill_path},
};

use super::layout::TextOutline;

pub(crate) const GLYPH_SUBPIXEL_STEPS: i32 = 8;

pub(crate) fn split_subpixel_position(value: f32) -> Option<(i32, u8)> {
    if !value.is_finite() {
        return None;
    }

    let floor = value.floor();

    if floor < i32::MIN as f32 || floor > i32::MAX as f32 {
        return None;
    }

    let mut base = floor as i32;
    let mut phase = ((value - floor) * GLYPH_SUBPIXEL_STEPS as f32).round() as i32;

    phase = phase.clamp(0, GLYPH_SUBPIXEL_STEPS);

    if phase == GLYPH_SUBPIXEL_STEPS {
        base = base.saturating_add(1);
        phase = 0;
    }

    Some((base, phase as u8))
}

pub fn rasterize_text_outline(
    surface: &mut Surface,
    outline: &TextOutline,
    color: Color,
) -> Result<(), FlattenError> {
    let flattened = flatten_path(&outline.path, 0.25)?;

    fill_path(surface, &flattened, FillRule::NonZero, color);

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::graphics::{
        geometry::{Path, Point},
        raster::Color,
    };

    use super::*;

    #[test]
    fn rasterizes_a_text_outline_with_nonzero_fill() {
        let mut path = Path::new();
        path.move_to(Point::new(1.0, 1.0));
        path.line_to(Point::new(5.0, 1.0));
        path.line_to(Point::new(5.0, 5.0));
        path.line_to(Point::new(1.0, 5.0));
        path.close();

        let outline = TextOutline {
            path,
            advance_width: 4.0,
        };

        let mut surface = Surface::new(7, 7, Color::TRANSPARENT);

        rasterize_text_outline(&mut surface, &outline, Color::BLACK).unwrap();

        assert_eq!(surface.pixel(2, 2), Some(Color::BLACK));
        assert_eq!(surface.pixel(0, 0), Some(Color::TRANSPARENT));
    }

    #[test]
    fn splits_glyph_positions_into_integer_and_subpixel_phase() {
        assert_eq!(split_subpixel_position(10.625), Some((10, 5)));
        assert_eq!(split_subpixel_position(10.99), Some((11, 0)));
        assert_eq!(split_subpixel_position(-0.5), Some((-1, 4)));
        assert_eq!(split_subpixel_position(f32::NAN), None);
        assert_eq!(split_subpixel_position(f32::INFINITY), None);
    }
}
