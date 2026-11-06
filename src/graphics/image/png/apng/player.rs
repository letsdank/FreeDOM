//! Deterministic APNG playback without timers or window-system integration.
//!
//! PNG Third Edition, sections 11.3.6.1 and 11.3.6.2:
//! <https://www.w3.org/TR/png-3/#acTL-chunk>
//! <https://www.w3.org/TR/png-3/#fcTL-chunk>

use std::time::Duration;

use super::super::{PngDecodeError, PngDecodeLimits, PngError, read_ihdr};
use super::{
    AnimationCanvas, AnimationError, AnimationIndex, AnimationLimits, CompositeError, FrameControl,
    index_animation,
};

// Viewer policy: avoid unbounded work for frames with very short delays.
const MIN_FRAME_DELAY: Duration = Duration::from_millis(10);

#[derive(Debug)]
pub enum PlaybackError {
    NotAnimated,
    Png(PngError),
    Animation(AnimationError),
    Decode(PngDecodeError),
    Composite(CompositeError),
    TimeWentBackwards,
    DeadlineOverflow,
}

impl std::fmt::Display for PlaybackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "APNG playback error: {self:?}")
    }
}

impl std::error::Error for PlaybackError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlaybackUpdate {
    FrameChanged,
    Waiting,
    Finished,
}

/// Borrows the original PNG; output pixels are RGBA8 in the file's encoding.
///
/// The caller drives playback with nondecreasing elapsed times. At most one
/// transition is processed per call. Repeated calls with the same time catch up.
pub struct AnimationPlayer<'a> {
    index: AnimationIndex<'a>,
    canvas: AnimationCanvas,
    limits: PngDecodeLimits,
    frame: Option<usize>,
    plays_started: u32,
    deadline: Option<Duration>,
    last_time: Option<Duration>,
    finished: bool,
}

impl<'a> AnimationPlayer<'a> {
    pub fn new(
        input: &'a [u8],
        animation_limits: AnimationLimits,
        limits: PngDecodeLimits,
    ) -> Result<Self, PlaybackError> {
        let header = read_ihdr(input).map_err(PlaybackError::Png)?;

        let index = index_animation(input, animation_limits)
            .map_err(PlaybackError::Animation)?
            .ok_or(PlaybackError::NotAnimated)?;

        let canvas = AnimationCanvas::new(header.width, header.height, limits)
            .map_err(PlaybackError::Composite)?;

        Ok(Self {
            index,
            canvas,
            limits,
            frame: None,
            plays_started: 0,
            deadline: None,
            last_time: None,
            finished: false,
        })
    }

    pub fn canvas(&self) -> &AnimationCanvas {
        &self.canvas
    }

    pub fn current_frame(&self) -> Option<usize> {
        self.frame
    }

    pub fn next_deadline(&self) -> Option<Duration> {
        self.deadline
    }

    /// First call displays frame zero and uses `now` as the start time.
    pub fn advance(&mut self, now: Duration) -> Result<PlaybackUpdate, PlaybackError> {
        if self.last_time.is_some_and(|last| now < last) {
            return Err(PlaybackError::TimeWentBackwards);
        }

        if self.finished {
            self.last_time = Some(now);

            return Ok(PlaybackUpdate::Finished);
        }

        let (next, base, restart) = match self.frame {
            None => (0, now, true),

            Some(current) => {
                let deadline = self.deadline.expect("active playback has a deadline");

                if now < deadline {
                    self.last_time = Some(now);

                    return Ok(PlaybackUpdate::Waiting);
                }

                if current + 1 < self.index.frames.len() {
                    (current + 1, deadline, false)
                } else {
                    let plays = self.index.summary.control.num_plays;

                    if plays != 0 && self.plays_started >= plays {
                        self.finished = true;
                        self.deadline = None;
                        self.last_time = Some(now);

                        return Ok(PlaybackUpdate::Finished);
                    }

                    (0, deadline, true)
                }
            }
        };

        let control = self.index.frames[next].control;

        let deadline = base
            .checked_add(frame_delay(&control))
            .ok_or(PlaybackError::DeadlineOverflow)?;

        let image = self
            .index
            .decode_frame_rgba8(next, self.limits)
            .map_err(PlaybackError::Decode)?;

        if restart {
            self.canvas.reset();
        }

        self.canvas
            .apply_frame(&control, &image.pixels)
            .map_err(PlaybackError::Composite)?;

        if restart {
            self.plays_started = self.plays_started.saturating_add(1);
        }

        self.frame = Some(next);
        self.deadline = Some(deadline);
        self.last_time = Some(now);

        Ok(PlaybackUpdate::FrameChanged)
    }
}

fn frame_delay(frame: &FrameControl) -> Duration {
    let denominator = u64::from(frame.effective_delay_denominator());

    let nanos = u64::from(frame.delay_num) * 1_000_000_000 / denominator;

    Duration::from_nanos(nanos).max(MIN_FRAME_DELAY)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WPT: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/testdata/wpt/png/apng/support/033.png"
    ));

    fn player() -> AnimationPlayer<'static> {
        AnimationPlayer::new(WPT, AnimationLimits::default(), PngDecodeLimits::default()).unwrap()
    }

    #[test]
    fn plays_wpt_frames_at_their_deadlines_and_keeps_the_last_frame() {
        let mut player = player();

        assert_eq!(
            player.advance(Duration::ZERO).unwrap(),
            PlaybackUpdate::FrameChanged
        );

        assert_eq!(player.current_frame(), Some(0));

        assert!(
            player
                .canvas()
                .pixels()
                .chunks_exact(4)
                .all(|pixel| pixel == [0, 0, 0, 255])
        );

        let first_end = player.next_deadline().unwrap();

        assert!(first_end > Duration::ZERO);

        assert_eq!(
            player.advance(first_end - Duration::from_nanos(1)).unwrap(),
            PlaybackUpdate::Waiting
        );

        assert_eq!(
            player.advance(first_end).unwrap(),
            PlaybackUpdate::FrameChanged
        );

        assert_eq!(player.current_frame(), Some(1));

        assert!(
            player
                .canvas
                .pixels()
                .chunks_exact(4)
                .all(|pixel| pixel == [0, 0, 128, 255])
        );

        let end = player.next_deadline().unwrap();

        assert_eq!(player.advance(end).unwrap(), PlaybackUpdate::Finished);

        assert_eq!(player.next_deadline(), None);

        assert!(
            player
                .canvas
                .pixels()
                .chunks_exact(4)
                .all(|pixel| pixel == [0, 0, 128, 255])
        );
    }

    #[test]
    fn late_calls_catch_up_without_shifting_deadlines() {
        let mut player = player();

        player.advance(Duration::ZERO).unwrap();

        let first_end = player.next_deadline().unwrap();
        let late = Duration::from_secs(10);

        assert_eq!(player.advance(late).unwrap(), PlaybackUpdate::FrameChanged);

        assert!(player.next_deadline().unwrap() < late);
        assert!(player.next_deadline().unwrap() > first_end);

        assert_eq!(player.advance(late).unwrap(), PlaybackUpdate::Finished);
    }

    #[test]
    fn restarts_finite_and_infinite_animations() {
        for count in [0, 2] {
            let mut player = player();

            player.index.summary.control.num_plays = count;

            player.advance(Duration::ZERO).unwrap();
            player.advance(player.next_deadline().unwrap()).unwrap();

            assert_eq!(player.current_frame(), Some(1));

            assert_eq!(
                player.advance(player.next_deadline().unwrap()).unwrap(),
                PlaybackUpdate::FrameChanged
            );

            assert_eq!(player.current_frame(), Some(0));

            assert!(
                player
                    .canvas
                    .pixels()
                    .chunks_exact(4)
                    .all(|pixel| pixel == [0, 0, 0, 255])
            );

            player.advance(player.next_deadline().unwrap()).unwrap();

            let update = player.advance(player.next_deadline().unwrap()).unwrap();

            assert_eq!(
                update,
                if count == 0 {
                    PlaybackUpdate::FrameChanged
                } else {
                    PlaybackUpdate::Finished
                }
            );
        }
    }

    #[test]
    fn rejects_backwards_time_and_static_png() {
        let mut player = player();

        player.advance(Duration::from_secs(1)).unwrap();

        assert!(matches!(
            player.advance(Duration::ZERO),
            Err(PlaybackError::TimeWentBackwards)
        ));

        assert_eq!(player.current_frame(), Some(0));

        let png = super::super::super::encode_rgba8(1, 1, &[0; 4]).unwrap();

        assert!(matches!(
            AnimationPlayer::new(&png, AnimationLimits::default(), PngDecodeLimits::default()),
            Err(PlaybackError::NotAnimated)
        ));
    }

    #[test]
    fn uses_default_denominator_and_minimum_delay() {
        let index = index_animation(WPT, AnimationLimits::default())
            .unwrap()
            .unwrap();

        let mut frame = index.frames[0].control;

        frame.delay_num = 25;
        frame.delay_den = 0;

        assert_eq!(frame_delay(&frame), Duration::from_millis(250));

        frame.delay_num = 0;

        assert_eq!(frame_delay(&frame), MIN_FRAME_DELAY);
    }
}
