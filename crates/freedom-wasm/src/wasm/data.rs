//! Decoding of active data segments and the optional data-count section.
//!
//! Only constant i32 offsets and active segments are supported. Evaluation of
//! other constant expressions and passive-segment operations are not yet
//! implemented. Segment payloads borrow the original module bytes.
//!
//! <https://webassembly.github.io/spec/core/binary/modules.html#data-section>

use super::{BinaryError, BinaryErrorKind, binary::FramedModule, reader::Reader};

#[derive(Debug)]
pub(crate) enum DataError {
    Binary(BinaryError),
    UnsupportedSegmentKind { kind: u32, offset: usize },
    UnsupportedOffsetExpression { opcode: u8, offset: usize },
    InvalidOffsetExpression { offset: usize },
    CountMismatch { declared: u32, actual: usize },
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DataSegment<'a> {
    pub(crate) memory_index: u32,
    pub(crate) memory_index_offset: usize,
    pub(crate) address: u32,
    pub(crate) bytes: &'a [u8],
}

/// Parses active data segments and verifies an optional data count.
pub(crate) fn decode_data<'a>(
    module: &FramedModule<'a>,
) -> Result<Vec<DataSegment<'a>>, DataError> {
    let mut segments = Vec::new();
    if let Some(section) = module.sections.iter().find(|section| section.id == 11) {
        let mut reader = Reader::new(section.payload, section.payload_offset);
        let count = reader.read_u32_leb().map_err(DataError::Binary)?;
        for _ in 0..count {
            let kind_offset = reader.offset();
            let kind = reader.read_u32_leb().map_err(DataError::Binary)?;
            if kind != 0 && kind != 2 {
                return Err(DataError::UnsupportedSegmentKind {
                    kind,
                    offset: kind_offset,
                });
            }
            let memory_index_offset = if kind == 2 {
                reader.offset()
            } else {
                kind_offset
            };
            let memory_index = if kind == 2 {
                reader.read_u32_leb().map_err(DataError::Binary)?
            } else {
                0
            };
            let opcode_offset = reader.offset();
            let opcode = reader.read_byte().map_err(DataError::Binary)?;
            if opcode != 0x41 {
                return Err(DataError::UnsupportedOffsetExpression {
                    opcode,
                    offset: opcode_offset,
                });
            }
            let address = reader.read_i32_leb().map_err(DataError::Binary)? as u32;
            let end_offset = reader.offset();
            if reader.read_byte().map_err(DataError::Binary)? != 0x0b {
                return Err(DataError::InvalidOffsetExpression { offset: end_offset });
            }
            let length_offset = reader.offset();
            let length = usize::try_from(reader.read_u32_leb().map_err(DataError::Binary)?)
                .map_err(|_| {
                    DataError::Binary(BinaryError::new(
                        length_offset,
                        BinaryErrorKind::SizeOverflow,
                    ))
                })?;
            let bytes = reader.read_bytes(length).map_err(DataError::Binary)?;
            segments.try_reserve(1).map_err(|_| {
                DataError::Binary(BinaryError::new(
                    kind_offset,
                    BinaryErrorKind::AllocationFailed,
                ))
            })?;
            segments.push(DataSegment {
                memory_index,
                memory_index_offset,
                address,
                bytes,
            });
        }
        if !reader.is_empty() {
            return Err(DataError::Binary(BinaryError::new(
                reader.offset(),
                BinaryErrorKind::TrailingSectionData(11),
            )));
        }
    }
    if let Some(section) = module.sections.iter().find(|section| section.id == 12) {
        let mut reader = Reader::new(section.payload, section.payload_offset);
        let declared = reader.read_u32_leb().map_err(DataError::Binary)?;
        if !reader.is_empty() {
            return Err(DataError::Binary(BinaryError::new(
                reader.offset(),
                BinaryErrorKind::TrailingSectionData(12),
            )));
        }
        if usize::try_from(declared).ok() != Some(segments.len()) {
            return Err(DataError::CountMismatch {
                declared,
                actual: segments.len(),
            });
        }
    }
    Ok(segments)
}

#[cfg(test)]
mod tests {
    use super::{DataError, decode_data};
    use crate::wasm::{
        BinaryErrorKind,
        binary::{FramedModule, Section, scan_module},
        test_support::module,
    };

    #[test]
    fn decodes_active_segments_and_checks_data_count() {
        let mut bytes = module(
            r#"(module (memory 1)
            (data (i32.const 1) "\01\02" "\03")
            (data (memory 0) (i32.const -1) "\ff"))"#,
        );
        let framed = scan_module(&bytes).unwrap();
        let segments = decode_data(&framed).unwrap();
        assert_eq!(
            (segments[0].address, segments[0].bytes),
            (1, &[1, 2, 3][..])
        );
        assert_eq!(
            (segments[1].address, segments[1].bytes),
            (u32::MAX, &[255][..])
        );
        let offset = framed
            .sections
            .iter()
            .find(|section| section.id == 11)
            .unwrap()
            .offset;
        bytes.splice(offset..offset, [12, 1, 2]);
        assert_eq!(decode_data(&scan_module(&bytes).unwrap()).unwrap().len(), 2);
        bytes[offset + 2] = 1;
        assert!(matches!(
            decode_data(&scan_module(&bytes).unwrap()),
            Err(DataError::CountMismatch {
                declared: 1,
                actual: 2
            })
        ));
    }

    #[test]
    fn rejects_malformed_or_unsupported_segments() {
        let cases: &[(&[u8], &str)] = &[
            (&[], "truncated"),
            (&[1, 1], "kind"),
            (&[1, 3], "kind"),
            (&[1, 0, 0x23], "expression"),
            (&[1, 0, 0x41, 0, 0x41], "end"),
            (&[1, 0, 0x41, 0, 0x0b, 2, 0x42], "truncated"),
            (&[0, 0], "trailing"),
        ];
        for (payload, expected) in cases {
            let module = FramedModule {
                sections: vec![Section {
                    id: 11,
                    offset: 98,
                    payload_offset: 100,
                    payload,
                    custom: None,
                }],
            };
            let error = decode_data(&module).unwrap_err();
            assert!(
                match *expected {
                    "truncated" =>
                        matches!(error, DataError::Binary(ref e) if e.kind == BinaryErrorKind::UnexpectedEnd),
                    "kind" => matches!(error, DataError::UnsupportedSegmentKind { .. }),
                    "expression" => matches!(error, DataError::UnsupportedOffsetExpression { .. }),
                    "end" => matches!(error, DataError::InvalidOffsetExpression { .. }),
                    "trailing" =>
                        matches!(error, DataError::Binary(ref e) if e.kind == BinaryErrorKind::TrailingSectionData(11)),
                    _ => false,
                },
                "{payload:?}: {error:?}"
            );
        }
    }
}
