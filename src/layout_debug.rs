//! Visualization of the current FreeDOM layout blocks.
//!
//! These are diagnostic rectangles, not yet CSS box-model boundaries.

use crate::{
    css::BackgroundColor,
    layout_preview::{PreviewBox, PreviewLayout},
    page_layout::{CONTENT_TOP, LayoutRect, PageLayout},
    render::BACKGROUND,
};

const OUTLINE: u32 = 0x00_E8_3D_3D;
const PREVIEW_OUTLINE: u32 = 0x00_2B_85_E8;

pub(crate) fn paint_preview_overlay(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    layout: &PreviewLayout,
    scroll_y: i32,
) {
    for item in &layout.boxes {
        draw_outline_color(pixels, width, height, item.rect, scroll_y, PREVIEW_OUTLINE);
    }
}

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
    draw_outline_color(pixels, width, height, rect, scroll_y, OUTLINE);
}

fn draw_outline_color(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    rect: LayoutRect,
    scroll_y: i32,
    color: u32,
) {
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

            pixels[row + x_start..row + x_end].fill(color);
        }
    }

    let y_start = top.max(visible_top).min(viewport_height) as usize;
    let y_end = (bottom + 1).max(visible_top).min(viewport_height) as usize;

    for x in [left, right] {
        if x >= 0 && x < viewport_width {
            for y in y_start..y_end {
                pixels[y * width + x as usize] = color;
            }
        }
    }
}

/// Paints backgrounds only; this is not the page renderer.
pub(crate) fn paint_preview_backgrounds(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    layout: &PreviewLayout,
    scroll_y: i32,
) {
    if !clear_preview_content(pixels, width, height) {
        return;
    }

    for item in &layout.boxes {
        paint_preview_box(pixels, width, height, item, scroll_y);
    }
}

pub(crate) fn clear_preview_content(pixels: &mut [u32], width: usize, height: usize) -> bool {
    let Some(needed) = width.checked_mul(height) else {
        return false;
    };

    if pixels.len() < needed {
        return false;
    }

    let content_top = (CONTENT_TOP as usize).min(height);
    pixels[content_top * width..needed].fill(BACKGROUND);

    true
}

pub(crate) fn paint_preview_box(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    item: &PreviewBox,
    scroll_y: i32,
) {
    if let BackgroundColor::Opaque(color) = item.background_color {
        fill_preview_rect(pixels, width, height, item.rect, scroll_y, color);
    }

    if item.border_top.width > 0 {
        let rect = LayoutRect {
            height: item.border_top.width.min(item.rect.height),
            ..item.rect
        };

        fill_preview_rect(pixels, width, height, rect, scroll_y, item.border_top.color);
    }

    if item.border_bottom.width > 0 {
        let border_height = item.border_bottom.width.min(item.rect.height);

        if let Some(y) = item
            .rect
            .y
            .checked_add(item.rect.height)
            .and_then(|value| value.checked_sub(border_height))
        {
            let rect = LayoutRect {
                y,
                height: border_height,
                ..item.rect
            };

            fill_preview_rect(
                pixels,
                width,
                height,
                rect,
                scroll_y,
                item.border_bottom.color,
            );
        }
    }
}

fn fill_preview_rect(
    pixels: &mut [u32],
    width: usize,
    height: usize,
    rect: LayoutRect,
    scroll_y: i32,
    color: u32,
) {
    if rect.width <= 0 || rect.height <= 0 {
        return;
    }

    let (Ok(viewport_width), Ok(viewport_height)) = (i64::try_from(width), i64::try_from(height))
    else {
        return;
    };

    let left = i64::from(rect.x);
    let top = i64::from(CONTENT_TOP) + i64::from(rect.y) - i64::from(scroll_y);

    let x_start = left.clamp(0, viewport_width) as usize;
    let x_end = (left + i64::from(rect.width)).clamp(0, viewport_width) as usize;

    let content_top = i64::from(CONTENT_TOP).min(viewport_height);

    let y_start = top.clamp(content_top, viewport_height) as usize;
    let y_end = (top + i64::from(rect.height)).clamp(content_top, viewport_height) as usize;

    for y in y_start..y_end {
        let row = y * width;
        pixels[row + x_start..row + x_end].fill(color);
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

    #[test]
    fn paints_preview_backgrounds_in_tree_order_and_clips_to_viewport() {
        use crate::{layout_preview::PreviewBox, styled_tree::SourcePath};

        let box_for = |rect, background_color| PreviewBox {
            source: SourcePath::from_indices(&[]),
            rect,
            background_color,
            border_top: crate::css::BorderEdge::default(),
            border_bottom: crate::css::BorderEdge::default(),
        };

        let layout = PreviewLayout {
            boxes: vec![
                box_for(
                    LayoutRect {
                        x: 1,
                        y: 0,
                        width: 6,
                        height: 8,
                    },
                    BackgroundColor::Opaque(0x00_FF_00_00),
                ),
                box_for(
                    LayoutRect {
                        x: 3,
                        y: 2,
                        width: 2,
                        height: 3,
                    },
                    BackgroundColor::Opaque(0x00_00_00_FF),
                ),
                box_for(
                    LayoutRect {
                        x: 3,
                        y: 2,
                        width: 2,
                        height: 3,
                    },
                    BackgroundColor::Transparent,
                ),
            ],
            display_list: Vec::new(),
        };

        let (width, height) = (10, 100);
        let mut pixels = vec![0x00_12_34_56; width * height];

        paint_preview_backgrounds(&mut pixels, width, height, &layout, 1);

        assert_eq!(pixels[89 * width + 1], 0x00_12_34_56);
        assert_eq!(pixels[90 * width + 1], 0x00_FF_00_00);
        assert_eq!(pixels[91 * width + 3], 0x00_00_00_FF);
        assert_eq!(pixels[90 * width], BACKGROUND);
        assert_eq!(pixels[97 * width + 1], BACKGROUND);
    }

    #[test]
    fn preview_backgrounds_tolerate_invalid_and_small_buffers() {
        let layout = PreviewLayout {
            boxes: Vec::new(),
            display_list: Vec::new(),
        };

        let mut pixels = vec![7; 10];

        paint_preview_backgrounds(&mut pixels, 10, 100, &layout, 0);
        paint_preview_backgrounds(&mut pixels, usize::MAX, 2, &layout, 0);

        assert_eq!(pixels, vec![7; 10]);
    }
}
