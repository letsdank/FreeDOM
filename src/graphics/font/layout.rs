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

    let advance_width = visit_positioned_glyphs(&[font], text, font_size, |_, glyph_id, pen_x| {
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
    visit_positioned_glyphs(&[font], text, font_size, |_, _, _| Ok(()))
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

// Partial per-character fallback; cluster shaping is not implemented.
// https://www.w3.org/TR/css-fonts-4/#font-matching-algorithm
pub(crate) fn visit_positioned_glyphs<F>(
    fonts: &[&TrueTypeFont<'_>],
    text: &str,
    font_size: f32,
    mut visit: F,
) -> Result<f32, FontError>
where
    F: FnMut(usize, u16, f32) -> Result<(), FontError>,
{
    let first = fonts.first().ok_or_else(|| FontError {
        offset: 0,
        message: "font list is empty".to_owned(),
    })?;

    text_scale(first, font_size)?;

    let mut pen_x = 0.0;
    let mut previous_glyph: Option<(usize, u16)> = None;

    for character in text.chars() {
        let (index, glyph_id) = fonts
            .iter()
            .enumerate()
            .find_map(|(index, font)| {
                let glyph = font.glyph_id(character);
                (glyph != 0).then_some((index, glyph))
            })
            .unwrap_or((0, 0));

        let font = fonts[index];
        let scale = text_scale(font, font_size)?;

        let metric = font.metric(glyph_id).ok_or_else(|| FontError {
            offset: 0,
            message: "cmap refers to a glyph outside the font".to_owned(),
        })?;

        if let Some((previous_font, previous_glyph)) = previous_glyph {
            if previous_font == index {
                pen_x += f32::from(font.kerning(previous_glyph, glyph_id)) * scale;
            }
        }

        visit(index, glyph_id, pen_x)?;

        pen_x += f32::from(metric.advance_width) * scale;
        previous_glyph = Some((index, glyph_id));
    }

    Ok(pen_x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::font::normalize_font_container;

    #[test]
    fn chooses_font_per_character_and_positions_glyphs() {
        let read = |path: &str| {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);

            normalize_font_container(std::fs::read(path).expect("font fixture must exist")).unwrap()
        };

        let data_a = read("testdata/fonts/noto/nko-hmtx.woff2");
        let data_b = read("testdata/wpt/fonts/noto/NotoSansMongolian-regular.woff2");

        let a = TrueTypeFont::parse(&data_a).unwrap();
        let b = TrueTypeFont::parse(&data_b).unwrap();

        assert_ne!(a.glyph_id('ߊ'), 0);
        assert_eq!(a.glyph_id('ᠠ'), 0);
        assert_ne!(b.glyph_id('ᠠ'), 0);

        let mut positions = Vec::new();

        let width = visit_positioned_glyphs(&[&a, &b], "ߊᠠߊ", 32.0, |index, glyph, x| {
            positions.push((index, glyph, x));
            Ok(())
        })
        .unwrap();

        let first = a.text_advance_width("ߊ", 32.0).unwrap();
        let second = b.text_advance_width("ᠠ", 32.0).unwrap();

        assert_eq!(
            positions
                .iter()
                .map(|position| position.0)
                .collect::<Vec<_>>(),
            [0, 1, 0]
        );

        assert_eq!(positions[1].2, first);
        assert_eq!(positions[2].2, first + second);
        assert_eq!(width, first + second + first);

        assert!(visit_positioned_glyphs(&[], "a", 32.0, |_, _, _| Ok(())).is_err());
    }
}
