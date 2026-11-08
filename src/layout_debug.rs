//! Visualization of the current FreeDOM layout blocks.
//!
//! These are diagnostic rectangles, not yet CSS box-model boundaries.

use crate::page_layout::{CONTENT_TOP, LayoutRect, PageLayout};

const OUTLINE: u32 = 0x00_E8_3D_3D;

pub(crate) fn paint_layout_overlay(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    layout: &PageLayout,
    scroll_y: i32,
) {
    for rect in layout.block_rects() {
        draw_outline(pixels, width, height, rect, scroll_y);
    }
}

fn draw_outline(pixels: &mut [u32], width: usize, height: usize, rect: LayoutRect, scroll_y: i32) {
    if rect.width <= 0
        || rect.height <= 0
        || width
            .checked_mul(height)
            .is_none_or(|needed| pixels.len() < needed)
    {
        return;
    }

    let (Ok(viewport_width), Ok(viewport_height)) = (i64::try_from(width), i64::try_from(height))
    else {
        return;
    };

    let left = i64::from(rect.x);
    let right = left + i64::from(rect.width) - 1;

    let top = i64::from(CONTENT_TOP) + i64::from(rect.y) - i64::from(scroll_y);
    let bottom = top + i64::from(rect.height) - 1;
    let visible_top = i64::from(CONTENT_TOP).min(viewport_height);

    let x_start = left.max(0).min(viewport_width) as usize;
    let x_end = (right + 1).max(0).min(viewport_width) as usize;

    // Draw only real horizontal edges, not the viewport clipping boundary.
    for y in [top, bottom] {
        if y >= visible_top && y < viewport_height {
            let row = y as usize * width;

            pixels[row + x_start..row + x_end].fill(OUTLINE);
        }
    }

    let y_start = top.max(visible_top).min(viewport_height) as usize;
    let y_end = (bottom + 1).max(visible_top).min(viewport_height) as usize;

    for x in [left, right] {
        if x >= 0 && x < viewport_width {
            for y in y_start..y_end {
                pixels[y * width + x as usize] = OUTLINE;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_edges_without_filling_block() {
        let (width, height) = (12, 100);
        let mut pixels = vec![0; width * height];

        let rect = LayoutRect {
            x: 2,
            y: 0,
            width: 5,
            height: 4,
        };

        draw_outline(&mut pixels, width, height, rect, 0);

        let pixel = |x, y| pixels[y * width + x];

        assert_eq!(pixel(2, 90), OUTLINE);
        assert_eq!(pixel(6, 93), OUTLINE);
        assert_eq!(pixel(4, 91), 0);
        assert_eq!(pixel(2, 89), 0);
    }

    #[test]
    fn clips_to_content_viewport_without_inventing_an_edge() {
        let (width, height) = (12, 100);
        let mut pixels = vec![0; width * height];

        let rect = LayoutRect {
            x: 2,
            y: 0,
            width: 5,
            height: 4,
        };

        draw_outline(&mut pixels, width, height, rect, 1);

        assert_eq!(pixels[90 * width + 2], OUTLINE);
        assert_eq!(pixels[90 * width + 4], 0);
        assert_eq!(pixels[92 * width + 4], OUTLINE);

        assert!(pixels[..90 * width].iter().all(|&color| color == 0));
    }

    #[test]
    fn tolerates_offscreen_and_invalid_buffers() {
        let mut pixels = vec![0; 10];

        draw_outline(
            &mut pixels,
            10,
            100,
            LayoutRect {
                x: i32::MIN,
                y: i32::MAX,
                width: i32::MAX,
                height: i32::MAX,
            },
            i32::MIN,
        );

        assert_eq!(pixels, vec![0; 10]);
    }
}
