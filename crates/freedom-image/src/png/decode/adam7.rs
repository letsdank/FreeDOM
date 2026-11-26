//! Adam7 pass geometry and filtered-data size calculation.
//!
//! PNG Third Edition, section 8:
//! <https://www.w3.org/TR/png-3/>

use super::PngDecodeError;

pub(super) mod reconstruct;

// (x start, y start, x step, y step), in PNG pass order.
const PATTERN: [(usize, usize, usize, usize); 7] = [
    (0, 0, 8, 8),
    (4, 0, 8, 8),
    (0, 4, 4, 8),
    (2, 0, 4, 4),
    (0, 2, 2, 4),
    (1, 0, 2, 2),
    (0, 1, 1, 2),
];

#[derive(Clone, Copy, Debug)]
pub(super) struct Pass {
    pub x_start: usize,
    pub y_start: usize,

    pub x_step: usize,
    pub y_step: usize,

    pub width: usize,
    pub height: usize,

    pub row_bytes: usize,
    pub filtered_len: usize,
}

#[derive(Debug)]
pub(super) struct Plan {
    pub passes: [Pass; 7],
    pub filtered_len: usize,
}

fn count_samples(size: usize, start: usize, step: usize) -> usize {
    if size <= start {
        0
    } else {
        1 + (size - 1 - start) / step
    }
}

/// Calculates pass sizes; bits_per_pixel must be a valid PNG pixel depth.
pub(super) fn plan(
    width: usize,
    height: usize,
    bits_per_pixel: usize,
) -> Result<Plan, PngDecodeError> {
    let empty = Pass {
        x_start: 0,
        y_start: 0,
        x_step: 0,
        y_step: 0,
        width: 0,
        height: 0,
        row_bytes: 0,
        filtered_len: 0,
    };

    let mut passes = [empty; 7];
    let mut total = 0usize;

    for (pass, &(x_start, y_start, x_step, y_step)) in passes.iter_mut().zip(PATTERN.iter()) {
        let pass_width = count_samples(width, x_start, x_step);
        let pass_height = count_samples(height, y_start, y_step);

        let (row_bytes, filtered_len) = if pass_width == 0 || pass_height == 0 {
            (0, 0)
        } else {
            let bytes = pass_width
                .checked_mul(bits_per_pixel)
                .and_then(|bits| bits.checked_add(7))
                .ok_or(PngDecodeError::SizeOverflow)?
                / 8;

            let len = bytes
                .checked_add(1)
                .and_then(|stride| stride.checked_mul(pass_height))
                .ok_or(PngDecodeError::SizeOverflow)?;

            (bytes, len)
        };

        total = total
            .checked_add(filtered_len)
            .ok_or(PngDecodeError::SizeOverflow)?;

        *pass = Pass {
            x_start,
            y_start,
            x_step,
            y_step,
            width: pass_width,
            height: pass_height,
            row_bytes,
            filtered_len,
        };
    }

    Ok(Plan {
        passes,
        filtered_len: total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_eight_by_eight_at_eight_bits_per_pixel() {
        let plan = plan(8, 8, 8).unwrap();

        let sizes = plan
            .passes
            .map(|pass| (pass.width, pass.height, pass.row_bytes, pass.filtered_len));

        assert_eq!(
            sizes,
            [
                (1, 1, 1, 2),
                (1, 1, 1, 2),
                (2, 1, 2, 3),
                (2, 2, 2, 6),
                (4, 2, 4, 10),
                (4, 4, 4, 20),
                (8, 4, 8, 36),
            ]
        );

        assert_eq!(plan.filtered_len, 79);
    }

    #[test]
    fn omits_empty_passes_for_small_images() {
        let plan = plan(1, 1, 32).unwrap();

        assert_eq!(plan.passes[0].filtered_len, 5);

        assert!(
            plan.passes[1..]
                .iter()
                .all(|pass| { pass.filtered_len == 0 && pass.row_bytes == 0 })
        );

        assert_eq!(plan.filtered_len, 5);
    }

    #[test]
    fn packs_every_pass_row_separately() {
        let plan = plan(7, 5, 1).unwrap();

        let sizes = plan
            .passes
            .map(|pass| (pass.width, pass.height, pass.row_bytes, pass.filtered_len));

        assert_eq!(
            sizes,
            [
                (1, 1, 1, 2),
                (1, 1, 1, 2),
                (2, 1, 1, 2),
                (2, 2, 1, 4),
                (4, 1, 1, 2),
                (3, 3, 1, 6),
                (7, 2, 1, 4),
            ]
        );

        assert_eq!(plan.filtered_len, 22);
    }

    #[test]
    fn covers_each_pixel_once_for_small_dimensions() {
        for height in 1..=17 {
            for width in 1..=17 {
                let plan = plan(width, height, 8).unwrap();
                let mut visits = vec![0u8; width * height];

                for pass in plan.passes {
                    for y in 0..pass.height {
                        for x in 0..pass.width {
                            let px = pass.x_start + x * pass.x_step;
                            let py = pass.y_start + y * pass.y_step;

                            assert!(px < width && py < height);

                            visits[py * width + px] += 1;
                        }
                    }
                }

                assert!(visits.iter().all(|count| *count == 1), "{width}x{height}");
            }
        }
    }

    #[test]
    fn matches_the_png_standard_eight_by_eight_pattern() {
        let plan = plan(8, 8, 8).unwrap();
        let mut grid = [[0u8; 8]; 8];

        for (index, pass) in plan.passes.iter().enumerate() {
            for y in 0..pass.height {
                for x in 0..pass.width {
                    let px = pass.x_start + x * pass.x_step;
                    let py = pass.y_start + y * pass.y_step;

                    grid[py][px] = (index + 1) as u8;
                }
            }
        }

        assert_eq!(
            grid,
            [
                [1, 6, 4, 6, 2, 6, 4, 6],
                [7, 7, 7, 7, 7, 7, 7, 7],
                [5, 6, 5, 6, 5, 6, 5, 6],
                [7, 7, 7, 7, 7, 7, 7, 7],
                [3, 6, 4, 6, 3, 6, 4, 6],
                [7, 7, 7, 7, 7, 7, 7, 7],
                [5, 6, 5, 6, 5, 6, 5, 6],
                [7, 7, 7, 7, 7, 7, 7, 7],
            ]
        );
    }

    #[test]
    fn rejects_size_overflow() {
        assert!(matches!(
            plan(usize::MAX, 1, 64),
            Err(PngDecodeError::SizeOverflow)
        ));

        assert!(matches!(
            plan(usize::MAX, usize::MAX, 1),
            Err(PngDecodeError::SizeOverflow)
        ));
    }
}
