//! Partial CSS color parsing for HTML text.
//!
//! Specification: <https://www.w3.org/TR/css-color-4/#hex-notation>
//!
//! Alpha and the full CSS color grammar are not supported here yet.

pub(super) fn parse_color(value: &str) -> Option<u32> {
    let value = value.trim().to_ascii_lowercase();

    match value.as_str() {
        "black" => return Some(0x00_00_00_00),
        "white" => return Some(0x00_FF_FF_FF),
        "red" => return Some(0x00_FF_00_00),
        "green" => return Some(0x00_00_80_00),
        "blue" => return Some(0x00_00_00_FF),
        "yellow" => return Some(0x00_FF_FF_00),
        "gray" | "grey" => return Some(0x00_80_80_80),
        "purple" => return Some(0x00_80_00_80),
        _ => {}
    }

    if let Some(hex) = value.strip_prefix('#') {
        if !hex.is_ascii() {
            return None;
        }

        return match hex.len() {
            3 => {
                let red = u8::from_str_radix(&hex[0..1], 16).ok()?;
                let green = u8::from_str_radix(&hex[1..2], 16).ok()?;
                let blue = u8::from_str_radix(&hex[2..3], 16).ok()?;

                Some(((red as u32 * 17) << 16) | ((green as u32 * 17) << 8) | (blue as u32 * 17))
            }

            6 => Some(u32::from_str_radix(hex, 16).ok()?),

            _ => None,
        };
    }

    let rgb = value.strip_prefix("rgb(")?.strip_suffix(')')?;

    let channels: Vec<u8> = rgb
        .split(',')
        .map(|channel| {
            channel
                .trim()
                .parse::<i64>()
                .map(|value| value.clamp(0, 255) as u8)
        })
        .collect::<std::result::Result<_, _>>()
        .ok()?;

    if channels.len() == 3 {
        return Some(
            ((channels[0] as u32) << 16) | ((channels[1] as u32) << 8) | channels[2] as u32,
        );
    }

    None
}

#[cfg(test)]
mod tests {
    use super::parse_color;

    #[test]
    fn rejects_non_ascii_hex_without_panicking() {
        for input in ["#éa", "#aé", "#é", "#💡"] {
            assert_eq!(parse_color(input), None, "{input}");
        }

        assert_eq!(parse_color("#aBc"), Some(0x00_AA_BB_CC));
        assert_eq!(parse_color("#a1b2c3"), Some(0x00_A1_B2_C3));
    }

    #[test]
    fn clamps_legacy_integer_rgb_channels() {
        assert_eq!(parse_color("rgb(500, 0, 0)"), Some(0x00_FF_00_00));
        assert_eq!(parse_color("rgb(-500, 64, 128)"), Some(0x00_00_40_80));
        assert_eq!(parse_color("rgb(+10, 20, 30)"), Some(0x00_0A_14_1E));
        assert_eq!(parse_color("rgb(2, wrong, 5)"), None);
    }
}
