//! Decoding of export names and external indices.
//!
//! This checks binary framing only. Name uniqueness and index validity require
//! module context and are not checked by the decoder.
//!
//! <https://webassembly.github.io/spec/core/binary/modules.html#export-section>

use super::{BinaryError, BinaryErrorKind, binary::FramedModule, reader::Reader};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExportKind {
    Function,
    Table,
    Memory,
    Global,
    Tag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Export<'a> {
    pub(crate) name: &'a str,
    pub(crate) name_offset: usize,
    pub(crate) kind: ExportKind,
    pub(crate) index: u32,
    pub(crate) index_offset: usize,
}

/// Decodes the export section without resolving its indices.
pub(crate) fn decode_exports<'a>(
    module: &FramedModule<'a>,
) -> Result<Vec<Export<'a>>, BinaryError> {
    let Some(section) = module.sections.iter().find(|section| section.id == 7) else {
        return Ok(Vec::new());
    };
    let mut reader = Reader::new(section.payload, section.payload_offset);
    let count = reader.read_u32_leb()?;
    let mut exports = Vec::new();
    for _ in 0..count {
        let length_offset = reader.offset();
        let length = usize::try_from(reader.read_u32_leb()?)
            .map_err(|_| BinaryError::new(length_offset, BinaryErrorKind::SizeOverflow))?;
        let name_offset = reader.offset();
        let name = std::str::from_utf8(reader.read_bytes(length)?).map_err(|error| {
            BinaryError::new(
                name_offset + error.valid_up_to(),
                BinaryErrorKind::InvalidNameUtf8,
            )
        })?;
        let kind_offset = reader.offset();
        let kind = match reader.read_byte()? {
            0 => ExportKind::Function,
            1 => ExportKind::Table,
            2 => ExportKind::Memory,
            3 => ExportKind::Global,
            4 => ExportKind::Tag,
            kind => {
                return Err(BinaryError::new(
                    kind_offset,
                    BinaryErrorKind::InvalidExportKind(kind),
                ));
            }
        };
        let index_offset = reader.offset();
        let index = reader.read_u32_leb()?;
        exports
            .try_reserve(1)
            .map_err(|_| BinaryError::new(name_offset, BinaryErrorKind::AllocationFailed))?;
        exports.push(Export {
            name,
            name_offset,
            kind,
            index,
            index_offset,
        });
    }
    if !reader.is_empty() {
        return Err(BinaryError::new(
            reader.offset(),
            BinaryErrorKind::TrailingSectionData(7),
        ));
    }
    Ok(exports)
}

#[cfg(test)]
mod tests {
    use super::{ExportKind, decode_exports};
    use crate::wasm::{
        BinaryErrorKind,
        binary::{FramedModule, Section, scan_module},
        test_support::module,
    };

    #[test]
    fn decodes_function_names_and_external_kinds() {
        let bytes = module(
            r#"(module
            (func (export "") (export "Сумма") (result i32) i32.const 42)
            (func (export "f")))"#,
        );
        let framed = scan_module(&bytes).unwrap();
        let exports = decode_exports(&framed).unwrap();
        assert_eq!(
            exports
                .iter()
                .map(|e| (e.name, e.kind, e.index))
                .collect::<Vec<_>>(),
            [
                ("", ExportKind::Function, 0),
                ("Сумма", ExportKind::Function, 0),
                ("f", ExportKind::Function, 1),
            ]
        );
        for export in exports {
            assert_eq!(
                &bytes[export.name_offset..][..export.name.len()],
                export.name.as_bytes()
            );
            assert_eq!(bytes[export.index_offset], export.index as u8);
        }
        // External descriptors for table, memory, global and tag.
        let payload = [
            4, 1, b't', 1, 0, 1, b'm', 2, 1, 1, b'g', 3, 2, 1, b'e', 4, 3,
        ];
        let module = FramedModule {
            sections: vec![Section {
                id: 7,
                offset: 98,
                payload_offset: 100,
                payload: &payload,
                custom: None,
            }],
        };
        let exports = decode_exports(&module).unwrap();
        assert_eq!(
            exports
                .iter()
                .map(|e| (e.kind, e.index))
                .collect::<Vec<_>>(),
            [
                (ExportKind::Table, 0),
                (ExportKind::Memory, 1),
                (ExportKind::Global, 2),
                (ExportKind::Tag, 3),
            ]
        );
    }

    #[test]
    fn reports_malformed_export_entries_with_absolute_offsets() {
        let cases: &[(&[u8], usize, BinaryErrorKind)] = &[
            (&[], 100, BinaryErrorKind::UnexpectedEnd),
            (&[1, 2, b'a'], 102, BinaryErrorKind::UnexpectedEnd),
            (&[1, 1, 0xff, 0, 0], 102, BinaryErrorKind::InvalidNameUtf8),
            (
                &[1, 1, b'x', 5, 0],
                103,
                BinaryErrorKind::InvalidExportKind(5),
            ),
            (&[1, 1, b'x', 0], 104, BinaryErrorKind::UnexpectedEnd),
            (&[1, 0, 0, 0x80], 104, BinaryErrorKind::UnexpectedEnd),
            (&[0, 0], 101, BinaryErrorKind::TrailingSectionData(7)),
        ];
        for (payload, offset, kind) in cases {
            let module = FramedModule {
                sections: vec![Section {
                    id: 7,
                    offset: 98,
                    payload_offset: 100,
                    payload,
                    custom: None,
                }],
            };
            let error = decode_exports(&module).unwrap_err();
            assert_eq!((error.offset, &error.kind), (*offset, kind), "{payload:?}");
        }
        let empty = FramedModule { sections: vec![] };
        assert!(decode_exports(&empty).unwrap().is_empty());
    }
}
