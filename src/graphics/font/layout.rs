use crate::graphics::geometry::{Path, Transform};

use super::{sfnt::FontError, truetype::TrueTypeFont};

#[derive(Clone, Debug, PartialEq)]
pub struct TextOutline {
    pub path: Path,
    pub advance_width: f32,
}

pub fn outline_text(
    font: &TrueTypeFont<'_>,
    text: &str,
    font_size: f32,
) -> Result<TextOutline, FontError> {
    if !font_size.is_finite() || font_size <= 0.0 {
        return Err(FontError {
            offset: 0,
            message: "font size must be finite and positive".to_owned(),
        });
    }

    let scale = font_size / f32::from(font.head().units_per_em);
    let mut path = Path::new();
    let mut pen_x = 0.0;

    for character in text.chars() {
        let glyph_id = font.glyph_id(character);
        let metric = font.metric(glyph_id).ok_or_else(|| FontError {
            offset: 0,
            message: "cmap refers to a glyph outside the font".to_owned(),
        })?;

        let glyph_path = font.glyph_path(glyph_id)?;
        let transform = Transform::scale(scale, -scale).then(Transform::translate(pen_x, 0.0));

        path.append(&glyph_path.transformed(transform));

        pen_x += f32::from(metric.advance_width) * scale;
    }

    Ok(TextOutline {
        path,
        advance_width: pen_x,
    })
}
