//! WebAssembly binary envelope scanning.
//!
//! Only the preamble, section envelopes/order, and custom-section names are
//! checked. Standard-section payloads are intentionally left uninterpreted;
//! success does not establish that the module is valid or executable.
//!
//! <https://webassembly.github.io/spec/core/binary/modules.html>

use super::{BinaryError, BinaryErrorKind, reader::Reader};

const MAGIC: &[u8; 4] = b"\0asm";
const VERSION: [u8; 4] = [1, 0, 0, 0];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FramedModule<'a> {
    pub(crate) sections: Vec<Section<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Section<'a> {
    pub(crate) id: u8,
    /// Offset of the section id in the original module.
    pub(crate) offset: usize,
    /// Offset and bytes of the complete section payload, excluding its header.
    pub(crate) payload_offset: usize,
    pub(crate) payload: &'a [u8],
    pub(crate) custom: Option<CustomSection<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CustomSection<'a> {
    pub(crate) name: &'a str,
    pub(crate) data: &'a [u8],
}

/// Scans a version-1 core binary's outer structure, not its semantic validity.
pub(crate) fn scan_module(bytes: &[u8]) -> Result<FramedModule<'_>, BinaryError> {
    let mut reader = Reader::new(bytes, 0);
    if reader.read_bytes(4)? != MAGIC {
        return Err(BinaryError::new(0, BinaryErrorKind::InvalidMagic));
    }
    let version: [u8; 4] = reader.read_bytes(4)?.try_into().expect("four-byte version");
    if version != VERSION {
        return Err(BinaryError::new(
            4,
            BinaryErrorKind::UnsupportedVersion(version),
        ));
    }

    let mut sections = Vec::new();
    let mut previous_rank = 0;
    let mut seen = 0_u16;

    while !reader.is_empty() {
        let offset = reader.offset();
        let id = reader.read_byte()?;
        let rank = section_rank(id)
            .ok_or_else(|| BinaryError::new(offset, BinaryErrorKind::InvalidSectionId(id)))?;
        if id != 0 {
            let bit = 1_u16 << id;
            if seen & bit != 0 {
                return Err(BinaryError::new(
                    offset,
                    BinaryErrorKind::DuplicateSection(id),
                ));
            }
            if rank < previous_rank {
                return Err(BinaryError::new(
                    offset,
                    BinaryErrorKind::OutOfOrderSection(id),
                ));
            }
            seen |= bit;
            previous_rank = rank;
        }

        let length_offset = reader.offset();
        let length = usize::try_from(reader.read_u32_leb()?)
            .map_err(|_| BinaryError::new(length_offset, BinaryErrorKind::SizeOverflow))?;
        let payload_offset = reader.offset();
        let payload = reader.read_bytes(length)?;
        let custom = if id == 0 {
            let mut custom_reader = Reader::new(payload, payload_offset);
            let name_length_offset = custom_reader.offset();
            let name_length = usize::try_from(custom_reader.read_u32_leb()?)
                .map_err(|_| BinaryError::new(name_length_offset, BinaryErrorKind::SizeOverflow))?;
            let name_offset = custom_reader.offset();
            let name_bytes = custom_reader.read_bytes(name_length)?;
            let name = std::str::from_utf8(name_bytes).map_err(|error| {
                BinaryError::new(
                    name_offset + error.valid_up_to(),
                    BinaryErrorKind::InvalidNameUtf8,
                )
            })?;
            Some(CustomSection {
                name,
                data: custom_reader.remaining(),
            })
        } else {
            None
        };

        sections
            .try_reserve(1)
            .map_err(|_| BinaryError::new(offset, BinaryErrorKind::AllocationFailed))?;
        sections.push(Section {
            id,
            offset,
            payload_offset,
            payload,
            custom,
        });
    }

    Ok(FramedModule { sections })
}

// The numeric IDs are not the section order: tag precedes global, while
// data-count precedes code. Custom sections can occur anywhere.
fn section_rank(id: u8) -> Option<u8> {
    match id {
        0 => Some(0),
        1..=5 => Some(id),
        13 => Some(6),
        6..=9 => Some(id + 1),
        12 => Some(11),
        10 => Some(12),
        11 => Some(13),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{BinaryErrorKind, scan_module};

    const HEADER: &[u8] = b"\0asm\x01\0\0\0";

    #[test]
    fn scans_a_real_function_module_and_custom_sections() {
        // module (func (export "f") (result i32) i32.const 42))
        let bytes = [
            0, 0x61, 0x73, 0x6d, 1, 0, 0, 0, 0, 3, 1, b'x', 0xff, 1, 5, 1, 0x60, 0, 1, 0x7f, 3, 2,
            1, 0, 7, 5, 1, 1, b'f', 0, 0, 10, 6, 1, 4, 0, 0x41, 42, 0x0b, 0, 1, 0,
        ];
        let module = scan_module(&bytes).unwrap();
        assert_eq!(
            module
                .sections
                .iter()
                .map(|section| section.id)
                .collect::<Vec<_>>(),
            [0, 1, 3, 7, 10, 0]
        );
        let custom = module.sections[0].custom.as_ref().unwrap();
        assert_eq!((custom.name, custom.data), ("x", &[0xff][..]));
        assert_eq!(module.sections[0].offset, 8);
        assert_eq!(module.sections[0].payload_offset, 10);
        assert_eq!(
            module
                .sections
                .last()
                .unwrap()
                .custom
                .as_ref()
                .unwrap()
                .name,
            ""
        );
        assert!(scan_module(HEADER).unwrap().sections.is_empty());
    }

    #[test]
    fn observes_section_order_including_tag_and_data_count() {
        let mut bytes = HEADER.to_vec();
        bytes.extend_from_slice(&[5, 1, 0, 13, 1, 0, 6, 1, 0, 12, 1, 0, 10, 1, 0, 11, 1, 0]);
        assert!(scan_module(&bytes).is_ok());

        let mut wrong = HEADER.to_vec();
        wrong.extend_from_slice(&[10, 1, 0, 12, 1, 0]);
        assert_eq!(
            scan_module(&wrong).unwrap_err().kind,
            BinaryErrorKind::OutOfOrderSection(12)
        );
    }

    #[test]
    fn rejects_bad_envelopes_without_claiming_payload_validation() {
        let mut bytes = HEADER.to_vec();
        bytes.extend_from_slice(&[1, 0]);
        // The scanner checks framing only; an empty type payload is not decoded.
        assert!(scan_module(&bytes).is_ok());

        let cases: &[(&[u8], BinaryErrorKind)] = &[
            (b"wasm\x01\0\0\0", BinaryErrorKind::InvalidMagic),
            (
                b"\0asm\x02\0\0\0",
                BinaryErrorKind::UnsupportedVersion([2, 0, 0, 0]),
            ),
            (b"\0asm\x01\0\0", BinaryErrorKind::UnexpectedEnd),
            (
                b"\0asm\x01\0\0\0\x0e\x00",
                BinaryErrorKind::InvalidSectionId(14),
            ),
            (
                b"\0asm\x01\0\0\0\x01\x00\x01\x00",
                BinaryErrorKind::DuplicateSection(1),
            ),
            (
                b"\0asm\x01\0\0\0\x01\x02\x00",
                BinaryErrorKind::UnexpectedEnd,
            ),
            (b"\0asm\x01\0\0\0\x00\x00", BinaryErrorKind::UnexpectedEnd),
            (
                b"\0asm\x01\0\0\0\x00\x02\x01\xff",
                BinaryErrorKind::InvalidNameUtf8,
            ),
            (
                b"\0asm\x01\0\0\0\x01\xff\xff\xff\xff\x10",
                BinaryErrorKind::InvalidUnsignedLeb128,
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(&scan_module(input).unwrap_err().kind, expected, "{input:?}");
        }
    }
}
