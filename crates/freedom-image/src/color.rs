//! Color conversion for image surfaces.
//!
//! CSS Color 4, sample color conversion code:
//! <https://www.w3.org/TR/css-color-4/#color-conversion-code>
//!
//! Display P3 and sRGB share the D65 white point and transfer function.
//! Out-of-gamut values are clipped; this is not perceptual gamut mapping.

pub(super) fn display_p3_to_srgb(pixel: [u8; 4]) -> [u8; 4] {
    // Expand 8-bit values exactly to the 16-bit range.
    display_p3_16_to_srgb(pixel.map(|value| u16::from(value) * 257))
}

pub(super) fn display_p3_16_to_srgb(pixel: [u16; 4]) -> [u8; 4] {
    fn to_linear(value: u16) -> f64 {
        let value = f64::from(value) / 65535.0;

        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }

    fn to_srgb(value: f64) -> u8 {
        let value = value.clamp(0.0, 1.0);

        let value = if value <= 0.0031308 {
            12.92 * value
        } else {
            1.055 * value.powf(1.0 / 2.4) - 0.055
        };

        (value * 255.0).round() as u8
    }

    let [r, g, b] = [pixel[0], pixel[1], pixel[2]].map(to_linear);

    // Display P3 -> XYZ D65 -> linear sRGB.
    let red = 1.224_940_176_280_56 * r - 0.224_940_176_280_56 * g;
    let green = -0.042_056_954_709_688 * r + 1.042_056_954_709_688 * g;
    let blue = -0.019_637_554_590_334 * r - 0.078_636_045_550_632 * g + 1.098_273_600_140_966 * b;

    [
        to_srgb(red),
        to_srgb(green),
        to_srgb(blue),
        (pixel[3] >> 8) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::display_p3_to_srgb;

    #[test]
    fn converts_p3_and_preserves_alpha() {
        assert_eq!(display_p3_to_srgb([51, 102, 153, 127]), [27, 104, 157, 127]);
        assert_eq!(display_p3_to_srgb([0, 0, 0, 0]), [0, 0, 0, 0]);

        assert_eq!(
            display_p3_to_srgb([255, 255, 255, 255]),
            [255, 255, 255, 255]
        );

        assert_eq!(display_p3_to_srgb([255, 0, 0, 1]), [255, 0, 0, 1]);
    }
}
