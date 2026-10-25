use crate::graphics::geometry::{Path, Transform};

use super::{sfnt::FontError, truetype::TrueTypeFont};

#[derive(Clone, Debug, PartialEq)]
pub struct TextOutline {
    pub path: Path,
    pub advance_width: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextRun {
    pub glyph_paths: Vec<Path>,
    pub advance_width: f32,
}

pub fn layout_text(
    font: &TrueTypeFont<'_>,
    text: &str,
    font_size: f32,
) -> Result<TextRun, FontError> {
    let scale = text_scale(font, font_size)?;
    let mut glyph_paths = Vec::with_capacity(text.chars().count());

    let advance_width = visit_positioned_glyphs(font, text, scale, |glyph_id, pen_x| {
        let glyph_path = font.glyph_path(glyph_id)?;
        let transform = Transform::scale(scale, -scale).then(Transform::translate(pen_x, 0.0));

        glyph_paths.push(glyph_path.transformed(transform));
        Ok(())
    })?;

    Ok(TextRun {
        glyph_paths,
        advance_width,
    })
}

pub fn text_advance_width(
    font: &TrueTypeFont<'_>,
    text: &str,
    font_size: f32,
) -> Result<f32, FontError> {
    let scale = text_scale(font, font_size)?;

    visit_positioned_glyphs(font, text, scale, |_, _| Ok(()))
}

pub fn outline_text(
    font: &TrueTypeFont<'_>,
    text: &str,
    font_size: f32,
) -> Result<TextOutline, FontError> {
    let run = layout_text(font, text, font_size)?;
    let mut path = Path::new();

    for glyph_path in run.glyph_paths {
        path.append(&glyph_path);
    }

    Ok(TextOutline {
        path,
        advance_width: run.advance_width,
    })
}

fn text_scale(font: &TrueTypeFont<'_>, font_size: f32) -> Result<f32, FontError> {
    if !font_size.is_finite() || font_size <= 0.0 {
        return Err(FontError {
            offset: 0,
            message: "font size must be finite and positive".to_owned(),
        });
    }

    Ok(font_size / f32::from(font.head().units_per_em))
}

fn visit_positioned_glyphs<F>(
    font: &TrueTypeFont<'_>,
    text: &str,
    scale: f32,
    mut visit: F,
) -> Result<f32, FontError>
where
    F: FnMut(u16, f32) -> Result<(), FontError>,
{
    let mut pen_x = 0.0;
    let mut previous_glyph = None;

    for character in text.chars() {
        let glyph_id = font.glyph_id(character);
        let metric = font.metric(glyph_id).ok_or_else(|| FontError {
            offset: 0,
            message: "cmap refers to a glyph outside the font".to_owned(),
        })?;

        if let Some(previous_glyph) = previous_glyph {
            pen_x += f32::from(font.kerning(previous_glyph, glyph_id)) * scale;
        }

        visit(glyph_id, pen_x)?;

        pen_x += f32::from(metric.advance_width) * scale;
        previous_glyph = Some(glyph_id);
    }

    Ok(pen_x)
}
