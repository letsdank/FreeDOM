use crate::graphics::{
    flatten::{FlattenError, flatten_path},
    raster::{Color, FillRule, Surface, fill_path},
};

use super::layout::TextOutline;

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
}
