//! WOFF2 point coordinate decoding.
//!
//! Specification: WOFF 2.0, section 5.2.
//! <https://www.w3.org/TR/WOFF2/>

use super::{FontError, FontReader, PointDelta};

/// Decodes one point and returns the number of coordinate bytes consumed.
pub(super) fn decode(
    flag: u8,
    data: &[u8],
    offset: usize,
) -> Result<(PointDelta, usize), FontError> {
    // The high bit describes whether the point is on the curve.
    let index = flag & 0x7f;

    let length = match index {
        0..=83 => 1,
        84..=119 => 2,
        120..=123 => 3,
        _ => 4,
    };

    let bytes = FontReader::new(data, 0).bytes(offset, length)?;

    let (x, y) = match index {
        0..=9 => {
            let y = i32::from(index / 2) * 256 + i32::from(bytes[0]);

            (0, y)
        }

        10..=19 => {
            let x = i32::from((index - 10) / 2) * 256 + i32::from(bytes[0]);

            (x, 0)
        }

        20..=83 => {
            let kind = index - 20;
            let x = 1 + i32::from(kind / 16) * 16 + i32::from(bytes[0] >> 4);
            let y = 1 + i32::from((kind / 4) % 4) * 16 + i32::from(bytes[0] & 0x0f);

            (x, y)
        }

        84..=119 => {
            let kind = index - 84;
            let x = 1 + i32::from(kind / 12) * 256 + i32::from(bytes[0]);
            let y = 1 + i32::from((kind / 4) % 3) * 256 + i32::from(bytes[1]);

            (x, y)
        }

        120..=123 => {
            let x = i32::from(bytes[0]) * 16 + i32::from(bytes[1] >> 4);
            let y = i32::from(bytes[1] & 0x0f) * 256 + i32::from(bytes[2]);

            (x, y)
        }

        _ => {
            let x = u16::from_be_bytes([bytes[0], bytes[1]]);
            let y = u16::from_be_bytes([bytes[2], bytes[3]]);

            (i32::from(x), i32::from(y))
        }
    };

    let (dx, dy) = if index < 10 {
        (0, signed(y, index & 1 != 0))
    } else if index < 20 {
        (signed(x, index & 1 != 0), 0)
    } else {
        (signed(x, index & 1 != 0), signed(y, index & 2 != 0))
    };

    Ok((
        PointDelta {
            dx,
            dy,
            on_curve: flag & 0x80 == 0,
        },
        length,
    ))
}

fn signed(value: i32, positive: bool) -> i32 {
    if positive { value } else { -value }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_boundaries_and_signs() {
        let cases: &[(u8, &[u8], i32, i32)] = &[
            (0, &[7], 0, -7),
            (9, &[7], 0, 1031),
            (10, &[7], -7, 0),
            (19, &[7], 1031, 0),
            (20, &[0x23], -3, -4),
            (23, &[0x23], 3, 4),
            (83, &[0xff], 64, 64),
            (84, &[2, 3], -3, -4),
            (119, &[255, 255], 768, 768),
            (120, &[0x12, 0x34, 0x56], -0x123, -0x456),
            (123, &[0x12, 0x34, 0x56], 0x123, 0x456),
            (124, &[0xff, 0xff, 0x80, 0x00], -65535, -32768),
            (127, &[0xff, 0xff, 0x80, 0x00], 65535, 32768),
        ];

        for &(flag, data, dx, dy) in cases {
            let (point, used) = decode(flag, data, 0).unwrap();

            assert_eq!(
                point,
                PointDelta {
                    dx,
                    dy,
                    on_curve: true,
                },
                "index {flag}"
            );

            assert_eq!(used, data.len());

            let (off_curve, _) = decode(flag | 0x80, data, 0).unwrap();

            assert_eq!(
                off_curve,
                PointDelta {
                    dx,
                    dy,
                    on_curve: false,
                }
            );
        }
    }

    #[test]
    fn handles_all_flags_and_rejects_short_input() {
        for flag in 0..=255_u8 {
            let bytes = [255_u8; 4];
            let (_, length) = decode(flag, &bytes, 0).unwrap();

            for available in 0..length {
                assert!(
                    decode(flag, &bytes[..available], 0).is_err(),
                    "flag {flag}, available bytes {available}"
                );
            }
        }
    }
}
