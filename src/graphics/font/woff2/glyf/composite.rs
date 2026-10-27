//! Reading WOFF2 composite-glyph component records.
//!
//! Specifications:
//! <https://www.w3.org/TR/WOFF2/#glyf_table_format>
//! <https://learn.microsoft.com/en-us-typography/opentype/spec/glyf>

use super::{CompositeComponents, FontError, FontReader, error};

const ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
const WE_HAVE_A_SCALE: u16 = 0x0008;
const MORE_COMPONENTS: u16 = 0x0020;
const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 0x0040;
const WE_HAVE_A_TWO_BY_TWO: u16 = 0x0080;
const WE_HAVE_INSTRUCTIONS: u16 = 0x0100;

/// Reads one composite glyph's component records.
///
/// Returns the original component bytes and their total encoded length.
pub(super) fn read(
    data: &[u8],
    start: usize,
    num_glyphs: u16,
) -> Result<(CompositeComponents<'_>, usize), FontError> {
    let reader = FontReader::new(data, 0);
    let mut cursor = start;
    let mut has_instructions = false;
    let last_flags_offset;

    loop {
        let header = reader.bytes(cursor, 4)?;

        let flags = u16::from_be_bytes([header[0], header[1]]);
        let glyph_id = u16::from_be_bytes([header[2], header[3]]);

        if glyph_id >= num_glyphs {
            return Err(error(
                cursor,
                "composite component glyph index is out of bounds",
            ));
        }

        let transform = flags & (WE_HAVE_A_SCALE | WE_HAVE_AN_X_AND_Y_SCALE | WE_HAVE_A_TWO_BY_TWO);

        if transform.count_ones() > 1 {
            return Err(error(
                cursor,
                "composite component has incompatible transform flags",
            ));
        }

        let argument_length = if flags & ARG_1_AND_2_ARE_WORDS != 0 {
            4
        } else {
            2
        };

        let transform_length = match transform {
            WE_HAVE_A_SCALE => 2,
            WE_HAVE_AN_X_AND_Y_SCALE => 4,
            WE_HAVE_A_TWO_BY_TWO => 8,
            _ => 0,
        };

        let length = 4 + argument_length + transform_length;

        // Validate the complete record before advancing.
        reader.bytes(cursor, length)?;

        has_instructions |= flags & WE_HAVE_INSTRUCTIONS != 0;

        if flags & MORE_COMPONENTS == 0 {
            last_flags_offset = cursor - start;
            cursor += length;
            break;
        }

        cursor += length;
    }

    let length = cursor - start;
    let bytes = reader.bytes(start, length)?;

    Ok((
        CompositeComponents {
            bytes,
            has_instructions,
            last_flags_offset,
        },
        length,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_component_chain_and_instructions_flag() {
        let data = [
            0x99, // unrelated prefix
            0x01, 0x22, 0, 0, 0xfe, 3, 0, 0x0b, 0, 1, 0, 10, 0xff, 0xfb, 0x40, 0,
        ];

        let (components, used) = read(&data, 1, 2).unwrap();

        assert_eq!(used, 16);
        assert_eq!(components.bytes, &data[1..]);
        assert_eq!(components.last_flags_offset, 6);

        // The instructions flag is present on the first component.
        assert!(components.has_instructions);

        for end in 0..data.len() {
            assert!(read(&data[..end], 1, 2).is_err(), "truncated at {end}");
        }
    }

    #[test]
    fn reads_argument_and_transform_sizes() {
        let cases = [
            (0x0002_u16, 2_usize), // byte arguments
            (0x0003, 4),           // word arguments
            (0x000a, 4),           // byte arguments + scale
            (0x0042, 6),           // byte arguments + X/Y scales
            (0x0082, 10),          // byte arguments + 2x2 matrix
        ];

        for (flags, extra_length) in cases {
            let mut data = Vec::from(flags.to_be_bytes());

            data.extend_from_slice(&0_u16.to_be_bytes());
            data.extend(vec![0_u8; extra_length]);

            let (_, used) = read(&data, 0, 1).unwrap();

            assert_eq!(used, 4 + extra_length);
        }
    }

    #[test]
    fn rejects_invalid_component_header() {
        // Both scale flags are set.
        let conflicting_flags = [0, 0x48, 0, 0, 0, 0, 0, 0, 0];

        let error = read(&conflicting_flags, 0, 1).unwrap_err();

        assert_eq!(
            error.message,
            "composite component has incompatible transform flags"
        );

        // Glyph ID 1 is outside a font containing one glyph.
        let bad_glyph_id = [0, 2, 0, 1, 0, 0];

        let error = read(&bad_glyph_id, 0, 1).unwrap_err();

        assert_eq!(
            error.message,
            "composite component glyph index is out of bounds"
        );
    }
}
