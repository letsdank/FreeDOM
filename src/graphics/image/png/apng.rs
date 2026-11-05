//! APNG animation control metadata.
//!
//! PNG Third Edition, section 11.3.6.1:
//! <https://www.w3.org/TR/png-3/#acTL-chunk>

use super::Chunk;

/// Declared animation control values, not proof that the frames are valid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnimationControl {
    pub num_frames: u32,

    /// Zero means the animation repeats indefinitely.
    pub num_plays: u32,
}

pub(super) fn parse_animation_control(chunk: &Chunk<'_>) -> Option<AnimationControl> {
    if chunk.kind != *b"acTL" || !chunk.crc_matches() {
        return None;
    }

    let [a, b, c, d, e, f, g, h] = chunk.data else {
        return None;
    };

    let num_frames = u32::from_be_bytes([*a, *b, *c, *d]);
    let num_plays = u32::from_be_bytes([*e, *f, *g, *h]);

    // PNG four-byte unsigned integers are limited to 2^31 - 1.
    if num_frames == 0 || num_frames > 0x7fff_ffff || num_plays > 0x7fff_ffff {
        return None;
    }

    Some(AnimationControl {
        num_frames,
        num_plays,
    })
}
