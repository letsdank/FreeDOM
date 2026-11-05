//! RGBA8 APNG frame compositing.
//!
//! PNG Third Edition, sections 4.9 and 11.3.6.2:
//! <https://www.w3.org/TR/png-3/#fcTL-chunk>
//!
//! Frames must use the same color encoding. This compositor does not perform
//! color management, 16-bit compositing, playback timing or scheduling.

use super::super::PngDecodeLimits;
use super::{BlendOp, DisposeOp, FrameControl};
use crate::graphics::raster::{Color, blend_source_over};

#[derive(Debug, Eq, PartialEq)]
pub enum CompositeError {
    InvalidDimensions,
    LimitExceeded,
    SizeOverflow,
    InvalidFrameBounds,
    PixelLength { expected: usize, actual: usize },
    AllocationFailed,
}

impl std::fmt::Display for CompositeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "APNG composite error: {self:?}")
    }
}

impl std::error::Error for CompositeError {}

struct PendingDisposal {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    operation: DisposeOp,
    saved: Vec<u8>,
}

/// Unassociated RGBA8 pixels. The canvas starts transparent for every play.
pub struct AnimationCanvas {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
    pending: Option<PendingDisposal>,
    first_frame: bool,
}

impl AnimationCanvas {
    pub fn new(width: u32, height: u32, limits: PngDecodeLimits) -> Result<Self, CompositeError> {
        if width == 0 || height == 0 {
            return Err(CompositeError::InvalidDimensions);
        }

        if width > limits.max_width || height > limits.max_height {
            return Err(CompositeError::LimitExceeded);
        }

        let width = usize::try_from(width).map_err(|_| CompositeError::SizeOverflow)?;
        let height = usize::try_from(height).map_err(|_| CompositeError::SizeOverflow)?;

        let pixels = width
            .checked_mul(height)
            .ok_or(CompositeError::SizeOverflow)?;

        if pixels > limits.max_pixels {
            return Err(CompositeError::LimitExceeded);
        }

        let len = pixels.checked_mul(4).ok_or(CompositeError::SizeOverflow)?;

        let mut data = Vec::new();

        data.try_reserve_exact(len)
            .map_err(|_| CompositeError::AllocationFailed)?;

        data.resize(len, 0);

        Ok(Self {
            width,
            height,
            pixels: data,
            pending: None,
            first_frame: true,
        })
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Clears the output buffer before another animation play.
    pub fn reset(&mut self) {
        self.pixels.fill(0);
        self.pending = None;
        self.first_frame = true;
    }

    /// Draws a frame and leaves it visible until the next apply or reset.
    ///
    /// Validation and temporary allocation complete before canvas mutation.
    pub fn apply_frame(
        &mut self,
        frame: &FrameControl,
        pixels: &[u8],
    ) -> Result<(), CompositeError> {
        let x = usize::try_from(frame.x_offset).map_err(|_| CompositeError::SizeOverflow)?;
        let y = usize::try_from(frame.y_offset).map_err(|_| CompositeError::SizeOverflow)?;
        let width = usize::try_from(frame.width).map_err(|_| CompositeError::SizeOverflow)?;
        let height = usize::try_from(frame.height).map_err(|_| CompositeError::SizeOverflow)?;

        if width == 0
            || height == 0
            || x.checked_add(width).is_none_or(|value| value > self.width)
            || y.checked_add(height)
                .is_none_or(|value| value > self.height)
        {
            return Err(CompositeError::InvalidFrameBounds);
        }

        let row_bytes = width.checked_mul(4).ok_or(CompositeError::SizeOverflow)?;

        let expected = row_bytes
            .checked_mul(height)
            .ok_or(CompositeError::SizeOverflow)?;

        if pixels.len() != expected {
            return Err(CompositeError::PixelLength {
                expected,
                actual: pixels.len(),
            });
        }

        // PREVIOUS on the first frame is treated as BACKGROUND.
        let operation = if self.first_frame && frame.dispose_op == DisposeOp::Previous {
            DisposeOp::Background
        } else {
            frame.dispose_op
        };

        let mut saved = Vec::new();

        if operation == DisposeOp::Previous {
            saved
                .try_reserve_exact(expected)
                .map_err(|_| CompositeError::AllocationFailed)?;
        }

        self.dispose_pending();

        if operation == DisposeOp::Previous {
            for row in 0..height {
                let start = ((y + row) * self.width + x) * 4;

                saved.extend_from_slice(&self.pixels[start..start + row_bytes]);
            }
        }

        for row in 0..height {
            let start = ((y + row) * self.width + x) * 4;
            let source = &pixels[row * row_bytes..(row + 1) * row_bytes];
            let destination = &mut self.pixels[start..start + row_bytes];

            match frame.blend_op {
                BlendOp::Source => {
                    destination.copy_from_slice(source);
                }

                BlendOp::Over => {
                    for (source, destination) in
                        source.chunks_exact(4).zip(destination.chunks_exact_mut(4))
                    {
                        let result = blend_source_over(
                            Color::rgba(
                                destination[0],
                                destination[1],
                                destination[2],
                                destination[3],
                            ),
                            Color::rgba(source[0], source[1], source[2], source[3]),
                            1.0,
                        );

                        destination.copy_from_slice(&[
                            result.red,
                            result.green,
                            result.blue,
                            result.alpha,
                        ]);
                    }
                }
            }
        }

        self.pending = Some(PendingDisposal {
            x,
            y,
            width,
            height,
            operation,
            saved,
        });

        self.first_frame = false;

        Ok(())
    }

    fn dispose_pending(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };

        let row_bytes = pending.width * 4;

        for row in 0..pending.height {
            let start = ((pending.y + row) * self.width + pending.x) * 4;
            let target = &mut self.pixels[start..start + row_bytes];

            match pending.operation {
                DisposeOp::None => {}

                DisposeOp::Background => {
                    target.fill(0);
                }

                DisposeOp::Previous => {
                    let saved = &pending.saved[row * row_bytes..(row + 1) * row_bytes];

                    target.copy_from_slice(saved);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::image::png_format::{AnimationLimits, index_animation, read_ihdr};

    fn frame(
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        dispose_op: DisposeOp,
        blend_op: BlendOp,
    ) -> FrameControl {
        FrameControl {
            sequence_number: 0,
            width,
            height,
            x_offset: x,
            y_offset: y,
            delay_num: 0,
            delay_den: 100,
            dispose_op,
            blend_op,
        }
    }

    #[test]
    fn composites_wpt_frame_pixels_without_using_default_image() {
        // This does not execute the original timed browser reftest.
        let png = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/png/apng/support/033.png"
        ));

        let index = index_animation(png, AnimationLimits::default())
            .unwrap()
            .unwrap();

        let header = read_ihdr(png).unwrap();
        let limits = PngDecodeLimits::default();

        let mut canvas = AnimationCanvas::new(header.width, header.height, limits).unwrap();

        assert!(!index.summary.first_frame_uses_idat);

        for (i, frame) in index.frames.iter().enumerate() {
            let decoded = index.decode_frame_rgba8(i, limits).unwrap();

            canvas.apply_frame(&frame.control, &decoded.pixels).unwrap();

            let expected = if i == 0 {
                [0, 0, 0, 255]
            } else {
                [0, 0, 128, 255]
            };

            assert!(
                canvas
                    .pixels()
                    .chunks_exact(4)
                    .all(|pixel| pixel == expected)
            );
        }

        canvas.reset();

        assert!(canvas.pixels().iter().all(|&byte| byte == 0));
    }

    #[test]
    fn disposes_background_and_restores_previous_region() {
        let limits = PngDecodeLimits::default();

        for disposal in [DisposeOp::Background, DisposeOp::Previous] {
            let mut canvas = AnimationCanvas::new(2, 1, limits).unwrap();

            let base = frame(0, 0, 2, 1, DisposeOp::None, BlendOp::Source);

            canvas
                .apply_frame(&base, &[255, 0, 0, 255, 0, 255, 0, 255])
                .unwrap();

            let overlay = frame(0, 0, 1, 1, disposal, BlendOp::Source);

            canvas.apply_frame(&overlay, &[0, 0, 255, 255]).unwrap();

            assert_eq!(canvas.pixels(), &[0, 0, 255, 255, 0, 255, 0, 255]);

            let next = frame(1, 0, 1, 1, DisposeOp::None, BlendOp::Source);

            canvas.apply_frame(&next, &[0, 0, 0, 0]).unwrap();

            let first = if disposal == DisposeOp::Previous {
                [255, 0, 0, 255]
            } else {
                [0, 0, 0, 0]
            };

            assert_eq!(&canvas.pixels()[..4], &first);
            assert_eq!(&canvas.pixels()[4..], &[0, 0, 0, 0]);
        }
    }

    #[test]
    fn preserves_pending_disposal_when_frame_is_rejected() {
        let mut canvas = AnimationCanvas::new(2, 1, PngDecodeLimits::default()).unwrap();

        let first = frame(0, 0, 1, 1, DisposeOp::Background, BlendOp::Source);

        canvas.apply_frame(&first, &[10, 20, 30, 255]).unwrap();

        let next = frame(1, 0, 1, 1, DisposeOp::None, BlendOp::Source);

        assert_eq!(
            canvas.apply_frame(&next, &[1, 2, 3]),
            Err(CompositeError::PixelLength {
                expected: 4,
                actual: 3
            })
        );

        assert_eq!(&canvas.pixels()[..4], &[10, 20, 30, 255]);

        canvas.apply_frame(&next, &[4, 5, 6, 255]).unwrap();

        assert_eq!(canvas.pixels(), [0, 0, 0, 0, 4, 5, 6, 255]);
    }

    #[test]
    fn handles_first_previous_source_and_over() {
        let mut canvas = AnimationCanvas::new(1, 1, PngDecodeLimits::default()).unwrap();

        let first = frame(0, 0, 1, 1, DisposeOp::Previous, BlendOp::Source);

        canvas.apply_frame(&first, &[255, 0, 0, 255]).unwrap();

        let next = frame(0, 0, 1, 1, DisposeOp::None, BlendOp::Over);

        canvas.apply_frame(&next, &[0, 0, 255, 128]).unwrap();

        assert_eq!(canvas.pixels(), [0, 0, 255, 128]);

        let cover = frame(0, 0, 1, 1, DisposeOp::None, BlendOp::Source);

        canvas.apply_frame(&cover, &[10, 20, 30, 0]).unwrap();

        assert_eq!(canvas.pixels(), [10, 20, 30, 0]);

        canvas.reset();

        assert_eq!(canvas.pixels(), [0, 0, 0, 0]);
    }

    #[test]
    fn respects_dimensions_and_limits() {
        let limits = PngDecodeLimits::default();

        assert!(matches!(
            AnimationCanvas::new(0, 1, limits),
            Err(CompositeError::InvalidDimensions)
        ));

        assert!(matches!(
            AnimationCanvas::new(
                2,
                2,
                PngDecodeLimits {
                    max_pixels: 3,
                    ..limits
                }
            ),
            Err(CompositeError::LimitExceeded)
        ));

        let mut canvas = AnimationCanvas::new(2, 1, limits).unwrap();

        let bad = frame(u32::MAX, 0, 1, 1, DisposeOp::None, BlendOp::Source);

        assert_eq!(
            canvas.apply_frame(&bad, &[0; 4]),
            Err(CompositeError::InvalidFrameBounds)
        );

        assert_eq!(canvas.pixels(), [0; 8]);
    }
}
