//! Shared font resolution and positioned runs for inline text layout.
//!
//! This is a partial layout model, not a full CSS inline formatting context.

use crate::{
    css::{FontStyle, FontWeight, TextStyle},
    graphics::font::TrueTypeFont,
    page::{Page, ResolvedFont},
};

pub(crate) struct PositionedTextRun {
    pub(crate) x: i32,
    pub(crate) offset_x: f32,
    pub(crate) y: i32,
    pub(crate) offset_y: f32,
    pub(crate) text: String,
    pub(crate) style: TextStyle,
}

pub(crate) fn same_font(left: &TextStyle, right: &TextStyle) -> bool {
    same_font_except_weight(left, right) && left.font_weight == right.font_weight
}

pub(crate) fn same_font_except_weight(left: &TextStyle, right: &TextStyle) -> bool {
    left.font_size.to_bits() == right.font_size.to_bits()
        && left.font_style == right.font_style
        && left.font_families == right.font_families
}

pub(crate) struct InlineFonts<'style, 'font> {
    entries: Vec<(&'style TextStyle, Vec<ResolvedFont<'font>>)>,
}

impl<'style, 'font> InlineFonts<'style, 'font> {
    pub(crate) fn from_styles(
        page: &'font Page,
        styles: impl IntoIterator<Item = &'style TextStyle>,
    ) -> Self {
        let mut entries: Vec<(&TextStyle, Vec<ResolvedFont<'font>>)> = Vec::new();

        for style in styles {
            if entries
                .iter()
                .any(|(existing, _)| same_font(existing, style))
            {
                continue;
            }

            entries.push((style, page.vector_fonts_for_style(style)));
        }

        Self { entries }
    }

    pub(crate) fn get(&self, style: &TextStyle) -> Option<&TrueTypeFont<'font>> {
        self.entries
            .iter()
            .find(|(existing, _)| same_font(existing, style))
            .and_then(|(_, fonts)| fonts.first().map(|entry| &entry.font))
    }

    pub(crate) fn stack(&self, style: &TextStyle) -> &[ResolvedFont<'font>] {
        self.entries
            .iter()
            .find(|(existing, _)| same_font(existing, style))
            .filter(|(_, fonts)| fonts.len() > 1)
            .map(|(_, fonts)| fonts.as_slice())
            .unwrap_or_default()
    }

    pub(crate) fn synthetic_weight(&self, style: &TextStyle) -> FontWeight {
        self.entries
            .iter()
            .find(|(existing, _)| same_font(existing, style))
            .and_then(|(_, fonts)| fonts.first())
            .map_or(style.font_weight, |entry| entry.synthesis.0)
    }

    pub(crate) fn synthetic_style(&self, style: &TextStyle) -> FontStyle {
        self.entries
            .iter()
            .find(|(existing, _)| same_font(existing, style))
            .and_then(|(_, fonts)| fonts.first())
            .map_or(style.font_style, |entry| entry.synthesis.1)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}
