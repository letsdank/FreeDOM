//! Mutable views of XRGB8888 pixel buffers, such as a window frame buffer.

/// A mutable, row-major XRGB8888 pixel buffer of a known size.
///
/// Each pixel stores red, green and blue in bits 16..24, 8..16 and 0..8; the
/// high byte is ignored. The width and height always fit in `i32`, so
/// painting code may convert them to signed coordinates without checks.
pub struct XrgbFrame<'a> {
    pixels: &'a mut [u32],
    width: usize,
    height: usize,
}

impl<'a> XrgbFrame<'a> {
    /// Wraps the first `width * height` pixels of `pixels`.
    ///
    /// Returns `None` when `pixels` is too short, or when a dimension or the
    /// pixel count does not fit the supported coordinate range.
    pub fn new(pixels: &'a mut [u32], width: usize, height: usize) -> Option<Self> {
        i32::try_from(width).ok()?;
        i32::try_from(height).ok()?;
        let pixel_count = width.checked_mul(height)?;
        let pixels = pixels.get_mut(..pixel_count)?;

        Some(Self {
            pixels,
            width,
            height,
        })
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Returns exactly `width * height` pixels.
    pub fn pixels(&self) -> &[u32] {
        self.pixels
    }

    /// Returns exactly `width * height` pixels.
    pub fn pixels_mut(&mut self) -> &mut [u32] {
        self.pixels
    }

    /// Borrows the whole frame for a shorter lifetime.
    pub fn reborrow(&mut self) -> XrgbFrame<'_> {
        XrgbFrame {
            pixels: self.pixels,
            width: self.width,
            height: self.height,
        }
    }

    /// Borrows the rows from `top` to the bottom edge as a separate frame,
    /// whose row 0 is row `top` of this frame. `top` is clamped to the height.
    pub fn rows_from(&mut self, top: usize) -> XrgbFrame<'_> {
        let top = top.min(self.height);

        XrgbFrame {
            pixels: &mut self.pixels[top * self.width..],
            width: self.width,
            height: self.height - top,
        }
    }

    /// Fills a rectangle with `color`, clipped to the frame.
    pub fn fill_rect(&mut self, x: i32, y: i32, width: i32, height: i32, color: u32) {
        let start_x = x.max(0) as usize;
        let start_y = y.max(0) as usize;
        let end_x = x.saturating_add(width).min(self.width as i32).max(0) as usize;
        let end_y = y.saturating_add(height).min(self.height as i32).max(0) as usize;

        if start_x >= end_x {
            return;
        }

        for row in start_y..end_y {
            let start = row * self.width;
            self.pixels[start + start_x..start + end_x].fill(color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_buffers_that_do_not_match_the_size() {
        let mut pixels = vec![0; 10];

        assert!(XrgbFrame::new(&mut pixels, 10, 100).is_none());
        assert!(XrgbFrame::new(&mut pixels, usize::MAX, 2).is_none());
        assert!(XrgbFrame::new(&mut pixels, 0, i32::MAX as usize + 1).is_none());

        let frame = XrgbFrame::new(&mut pixels, 3, 3).unwrap();
        assert_eq!(frame.pixels().len(), 9);
    }

    #[test]
    fn rows_from_offsets_and_clamps() {
        let mut pixels = vec![0; 6];
        let mut frame = XrgbFrame::new(&mut pixels, 2, 3).unwrap();

        frame.rows_from(1).fill_rect(0, 0, 1, 1, 7);
        assert_eq!(frame.rows_from(5).height(), 0);
        assert_eq!(pixels, vec![0, 0, 7, 0, 0, 0]);
    }

    #[test]
    fn fill_rect_clips_to_the_frame() {
        let mut pixels = vec![0; 9];
        let mut frame = XrgbFrame::new(&mut pixels, 3, 3).unwrap();

        frame.fill_rect(-1, 2, 3, i32::MAX, 5);
        frame.fill_rect(i32::MAX, i32::MIN, i32::MAX, i32::MAX, 9);
        assert_eq!(pixels, vec![0, 0, 0, 0, 0, 0, 5, 5, 0]);
    }
}
