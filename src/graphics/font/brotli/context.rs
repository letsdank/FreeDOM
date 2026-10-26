//! Context selection for Brotli literals and distances.
//!
//! Specification: RFC 7932, sections 7.1 and 7.2.
//! <https://www.rfc-editor.org/rfc/rfc7932.html#section-7>
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LiteralContextMode {
    Lsb6,
    Msb6,
    Utf8,
    Signed,
}

// RFC 7932, section 7.1: literal context IDs.
pub(super) fn literal_context_id(
    mode: LiteralContextMode,
    previous: u8,
    before_previous: u8,
) -> usize {
    match mode {
        LiteralContextMode::Lsb6 => usize::from(previous & 0x3f),
        LiteralContextMode::Msb6 => usize::from(previous >> 2),
        LiteralContextMode::Utf8 => {
            usize::from(utf8_context_first(previous) | utf8_context_second(before_previous))
        }
        LiteralContextMode::Signed => usize::from(
            (signed_context_value(previous) << 3) | signed_context_value(before_previous),
        ),
    }
}

fn utf8_context_first(byte: u8) -> u8 {
    const ASCII: [u8; 128] = [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 8, 12, 16, 12, 12, 20, 12, 16, 24, 28, 12, 12, 32, 12, 36, 12, 44, 44, 44, 44, 44,
        44, 44, 44, 44, 44, 32, 32, 24, 40, 28, 12, 12, 48, 52, 52, 52, 48, 52, 52, 52, 48, 52, 52,
        52, 52, 52, 48, 52, 52, 52, 52, 52, 48, 52, 52, 52, 52, 52, 24, 12, 28, 12, 12, 12, 56, 60,
        60, 60, 56, 60, 60, 60, 56, 60, 60, 60, 60, 60, 56, 60, 60, 60, 60, 60, 56, 60, 60, 60, 60,
        60, 24, 12, 28, 12, 0,
    ];

    match byte {
        0..=127 => ASCII[usize::from(byte)],
        128..=191 => byte & 1,
        _ => 2 | (byte & 1),
    }
}

fn utf8_context_second(byte: u8) -> u8 {
    match byte {
        0..=32 => 0,
        33..=47 | 58..=64 | 91..=96 | 123..=126 => 1,
        48..=57 | 65..=90 | 224..=255 => 2,
        97..=122 => 3,
        _ => 0,
    }
}

fn signed_context_value(byte: u8) -> u8 {
    match byte {
        0 => 0,
        1..=15 => 1,
        16..=63 => 2,
        64..=127 => 3,
        128..=191 => 4,
        192..=239 => 5,
        240..=254 => 6,
        255 => 7,
    }
}

pub(super) fn distance_context_id(copy_length: usize) -> usize {
    match copy_length {
        2 => 0,
        3 => 1,
        4 => 2,
        _ => 3,
    }
}
