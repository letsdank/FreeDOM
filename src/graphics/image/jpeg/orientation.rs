//! EXIF orientation from the primary image IFD.
//!
//! CIPA DC-008-2012, section 4.6.4 and figure 12:
//! <https://www.cipa.jp/std/documents/e/DC-008-2012_E.pdf>
//!
//! Invalid or unsupported metadata is ignored.

use super::HeaderSegments;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Orientation(u16);

impl Orientation {
    pub(crate) fn dimensions(self, width: usize, height: usize) -> (usize, usize) {
        if self.0 >= 5 {
            (height, width)
        } else {
            (width, height)
        }
    }

    /// Maps an output pixel to its position in the unrotated image.
    pub(crate) fn source_position(
        self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
    ) -> (usize, usize) {
        match self.0 {
            2 => (width - 1 - x, y),
            3 => (width - 1 - x, height - 1 - y),
            4 => (x, height - 1 - y),
            5 => (y, x),
            6 => (y, height - 1 - x),
            7 => (width - 1 - y, height - 1 - x),
            8 => (width - 1 - y, x),
            _ => (x, y),
        }
    }
}

/// Reads the first valid orientation before the first JPEG scan.
pub(crate) fn read_orientation(input: &[u8]) -> Orientation {
    let Ok(mut segments) = HeaderSegments::new(input) else {
        return Orientation(1);
    };

    while let Ok(Some(segment)) = segments.next_segment() {
        if segment.marker == 0xe1 && segment.data.starts_with(b"Exif\0\0") {
            if let Some(orientation) = parse_tiff(&segment.data[6..]) {
                return orientation;
            }
        }
    }

    Orientation(1)
}

#[derive(Clone, Copy)]
enum ByteOrder {
    Little,
    Big,
}

impl ByteOrder {
    fn read_u16(self, bytes: &[u8], offset: usize) -> Option<u16> {
        let bytes: [u8; 2] = bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?;

        Some(match self {
            Self::Little => u16::from_le_bytes(bytes),
            Self::Big => u16::from_be_bytes(bytes),
        })
    }

    fn read_u32(self, bytes: &[u8], offset: usize) -> Option<u32> {
        let bytes: [u8; 4] = bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?;

        Some(match self {
            Self::Little => u32::from_le_bytes(bytes),
            Self::Big => u32::from_be_bytes(bytes),
        })
    }
}

fn parse_tiff(tiff: &[u8]) -> Option<Orientation> {
    let order = match tiff.get(..2)? {
        b"II" => ByteOrder::Little,
        b"MM" => ByteOrder::Big,
        _ => return None,
    };

    if order.read_u16(tiff, 2)? != 42 {
        return None;
    }

    let ifd = usize::try_from(order.read_u32(tiff, 4)?).ok()?;

    if ifd < 8 {
        return None;
    }

    let count = usize::from(order.read_u16(tiff, ifd)?);
    let start = ifd.checked_add(2)?;
    let end = start.checked_add(count.checked_mul(12)?)?;

    // IFD0 ends with an offset to the next IFD.
    tiff.get(start..end.checked_add(4)?)?;

    for index in 0..count {
        let entry = start + index * 12;

        if order.read_u16(tiff, entry)? != 0x0112 {
            continue;
        }

        if order.read_u16(tiff, entry + 2)? != 3 || order.read_u32(tiff, entry + 4)? != 1 {
            return None;
        }

        let value = order.read_u16(tiff, entry + 8)?;

        return (1..=8).contains(&value).then_some(Orientation(value));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiff(orientation: u16, little: bool) -> Vec<u8> {
        let mut output = Vec::new();

        output.extend_from_slice(if little { b"II" } else { b"MM" });

        let put16 = |out: &mut Vec<u8>, n: u16| {
            out.extend_from_slice(&if little {
                n.to_le_bytes()
            } else {
                n.to_be_bytes()
            });
        };

        let put32 = |out: &mut Vec<u8>, n: u32| {
            out.extend_from_slice(&if little {
                n.to_le_bytes()
            } else {
                n.to_be_bytes()
            });
        };

        put16(&mut output, 42);
        put32(&mut output, 8);

        // IFD0 entry count.
        put16(&mut output, 1);

        // Orientation tag, SHORT, one value.
        put16(&mut output, 0x0112);
        put16(&mut output, 3);
        put32(&mut output, 1);
        put16(&mut output, orientation);
        put16(&mut output, 0);

        // Next IFD offset.
        put32(&mut output, 0);

        output
    }

    #[test]
    fn parses_both_byte_orders_and_rejects_invalid_data() {
        for little in [false, true] {
            for value in 1..=8 {
                let data = tiff(value, little);

                assert_eq!(parse_tiff(&data), Some(Orientation(value)));

                for length in 0..data.len() {
                    assert_eq!(parse_tiff(&data[..length]), None);
                }
            }

            assert_eq!(parse_tiff(&tiff(9, little)), None);

            let mut invalid = tiff(2, little);
            invalid[4..8].copy_from_slice(&[0xff; 4]);

            assert_eq!(parse_tiff(&invalid), None);
        }

        assert_eq!(read_orientation(b"invalid"), Orientation(1));
    }

    #[test]
    fn maps_all_eight_orientations() {
        let expected: [&[u8]; 8] = [
            &[1, 2, 3, 4, 5, 6],
            &[3, 2, 1, 6, 5, 4],
            &[6, 5, 4, 3, 2, 1],
            &[4, 5, 6, 1, 2, 3],
            &[1, 4, 2, 5, 3, 6],
            &[4, 1, 5, 2, 6, 3],
            &[6, 3, 5, 2, 4, 1],
            &[3, 6, 2, 5, 1, 4],
        ];

        for value in 1..=8 {
            let orientation = Orientation(value);
            let (width, height) = orientation.dimensions(3, 2);

            assert_eq!((width, height), if value < 5 { (3, 2) } else { (2, 3) });

            let mut actual = Vec::new();

            for y in 0..height {
                for x in 0..width {
                    let (sx, sy) = orientation.source_position(x, y, 3, 2);

                    actual.push((sy * 3 + sx + 1) as u8);
                }
            }

            assert_eq!(
                actual,
                expected[usize::from(value - 1)],
                "orientation {value}"
            );
        }
    }

    #[test]
    fn reads_orientation_from_wpt_jpeg_asset() {
        // This tests an asset from WPT, not the original WPT browser test.
        let data = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/images/arrow-oriented-upright.jpg"
        ));

        assert_eq!(read_orientation(data), Orientation(6));
    }
}
