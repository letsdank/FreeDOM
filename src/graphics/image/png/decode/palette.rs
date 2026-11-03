//! Indexed-color PNG palette and alpha lookup.
//!
//! PNG Third Edition, sections 11.2.2, 11.3.1.1, and 13.1:
//! <https://www.w3.org/TR/png-3>

use super::PngDecodeError;

#[cfg(test)]
mod tests;

pub(super) struct Palette<'a> {
    colors: &'a [u8],
    alpha: Option<&'a [u8]>,
}

impl<'a> Palette<'a> {
    pub(super) fn new(colors: &'a [u8]) -> Result<Self, PngDecodeError> {
        if colors.is_empty() || colors.len() > 768 || colors.len() % 3 != 0 {
            return Err(PngDecodeError::InvalidPalette);
        }

        Ok(Self {
            colors,
            alpha: None,
        })
    }

    pub(super) fn set_alpha(&mut self, alpha: &'a [u8]) -> Result<(), PngDecodeError> {
        if self.alpha.is_some() || alpha.len() > self.colors.len() / 3 {
            return Err(PngDecodeError::InvalidTransparencyChunk);
        }

        self.alpha = Some(alpha);

        Ok(())
    }

    pub(super) fn rgba(&self, index: u8) -> [u8; 4] {
        let index = usize::from(index);
        let base = index * 3;

        if base + 2 >= self.colors.len() {
            // PNG Third Edition requires opaque black for out-of-range indices.
            return [0, 0, 0, 255];
        }

        let alpha = self
            .alpha
            .and_then(|values| values.get(index))
            .copied()
            .unwrap_or(255);

        [
            self.colors[base],
            self.colors[base + 1],
            self.colors[base + 2],
            alpha,
        ]
    }
}
