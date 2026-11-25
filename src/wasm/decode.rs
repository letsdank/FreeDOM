//! Decoding of function signatures, defined-function type indices and start.
//!
//! Only function types with numeric value types are supported so far. Other
//! type forms and value types are reported as unsupported, not as invalid.
//! Decoding does not validate indices, imports, code bodies, or the module.
//!
//! <https://webassembly.github.io/spec/core/binary/modules.html#type-section>
//! <https://webassembly.github.io/spec/core/binary/modules.html#function-section>
//! <https://webassembly.github.io/spec/core/binary/types.html#function-types>
//! <https://webassembly.github.io/spec/core/binary/modules.html#start-section>

use super::{
    BinaryError, BinaryErrorKind,
    binary::{FramedModule, Section},
    reader::Reader,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValueType {
    I32,
    I64,
    F32,
    F64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FunctionType {
    pub(crate) parameters: Vec<ValueType>,
    pub(crate) results: Vec<ValueType>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Declarations {
    pub(crate) types: Vec<FunctionType>,
    /// Type indices of defined functions; imported functions are not included.
    pub(crate) function_types: Vec<u32>,
}

/// Decodes selected sections of an already framed module, without validation.
pub(crate) fn decode_declarations(module: &FramedModule<'_>) -> Result<Declarations, BinaryError> {
    let mut declarations = Declarations::default();
    for section in &module.sections {
        match section.id {
            1 => declarations.types = read_section(section, read_function_type)?,
            3 => {
                declarations.function_types = read_section(section, |reader| reader.read_u32_leb())?
            }
            _ => {}
        }
    }
    Ok(declarations)
}

/// Reads the optional start function index without validating its signature.
pub(crate) fn decode_start(module: &FramedModule<'_>) -> Result<Option<(u32, usize)>, BinaryError> {
    let Some(section) = module.sections.iter().find(|section| section.id == 8) else {
        return Ok(None);
    };
    let mut reader = Reader::new(section.payload, section.payload_offset);
    let offset = reader.offset();
    let index = reader.read_u32_leb()?;
    if !reader.is_empty() {
        return Err(BinaryError::new(
            reader.offset(),
            BinaryErrorKind::TrailingSectionData(8),
        ));
    }
    Ok(Some((index, offset)))
}

fn read_section<T>(
    section: &Section<'_>,
    read_item: impl FnMut(&mut Reader<'_>) -> Result<T, BinaryError>,
) -> Result<Vec<T>, BinaryError> {
    let mut reader = Reader::new(section.payload, section.payload_offset);
    let items = read_vector(&mut reader, read_item)?;
    if !reader.is_empty() {
        return Err(BinaryError::new(
            reader.offset(),
            BinaryErrorKind::TrailingSectionData(section.id),
        ));
    }
    Ok(items)
}

fn read_vector<T>(
    reader: &mut Reader<'_>,
    mut read_item: impl FnMut(&mut Reader<'_>) -> Result<T, BinaryError>,
) -> Result<Vec<T>, BinaryError> {
    let count = reader.read_u32_leb()?;
    let mut items = Vec::new();
    for _ in 0..count {
        let item = read_item(reader)?;
        items
            .try_reserve(1)
            .map_err(|_| BinaryError::new(reader.offset(), BinaryErrorKind::AllocationFailed))?;
        items.push(item);
    }
    Ok(items)
}

fn read_function_type(reader: &mut Reader<'_>) -> Result<FunctionType, BinaryError> {
    let offset = reader.offset();
    let form = reader.read_byte()?;
    if form != 0x60 {
        return Err(BinaryError::new(
            offset,
            BinaryErrorKind::UnsupportedTypeForm(form),
        ));
    }
    Ok(FunctionType {
        parameters: read_vector(reader, read_value_type)?,
        results: read_vector(reader, read_value_type)?,
    })
}

pub(super) fn read_value_type(reader: &mut Reader<'_>) -> Result<ValueType, BinaryError> {
    let offset = reader.offset();
    Ok(match reader.read_byte()? {
        0x7f => ValueType::I32,
        0x7e => ValueType::I64,
        0x7d => ValueType::F32,
        0x7c => ValueType::F64,
        byte => {
            return Err(BinaryError::new(
                offset,
                BinaryErrorKind::UnsupportedValueType(byte),
            ));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{BinaryErrorKind, FunctionType, ValueType, decode_declarations, decode_start};
    use crate::wasm::binary::scan_module;
    use crate::wasm::test_support::module;

    #[test]
    fn decodes_start_section_and_rejects_malformed_payloads() {
        let bytes = module("(module (func) (start 0))");
        let framed = scan_module(&bytes).unwrap();
        let section = framed
            .sections
            .iter()
            .find(|section| section.id == 8)
            .unwrap();
        assert_eq!(
            decode_start(&framed).unwrap(),
            Some((0, section.payload_offset))
        );
        for (payload, kind, relative_offset) in [
            (&[][..], BinaryErrorKind::UnexpectedEnd, 0),
            (&[0, 0][..], BinaryErrorKind::TrailingSectionData(8), 1),
            (
                &[0xff, 0xff, 0xff, 0xff, 0x10][..],
                BinaryErrorKind::InvalidUnsignedLeb128,
                0,
            ),
        ] {
            let mut bytes = b"\0asm\x01\0\0\0".to_vec();
            bytes.extend([8, payload.len() as u8]);
            bytes.extend_from_slice(payload);
            let error = decode_start(&scan_module(&bytes).unwrap()).unwrap_err();
            assert_eq!(error.kind, kind);
            assert_eq!(error.offset, 10 + relative_offset);
        }
    }

    #[test]
    fn decodes_a_function_module_and_numeric_signatures() {
        let bytes = module(r#"(module (func (export "f") (result i32) i32.const 42))"#);
        let declarations = decode_declarations(&scan_module(&bytes).unwrap()).unwrap();
        assert_eq!(
            declarations.types,
            [FunctionType {
                parameters: vec![],
                results: vec![ValueType::I32],
            }]
        );
        assert_eq!(declarations.function_types, [0]);

        let numeric = module("(module (type (func (param i32 i64 f32 f64) (result i64 f32))))");
        let declarations = decode_declarations(&scan_module(&numeric).unwrap()).unwrap();
        assert_eq!(
            declarations.types[0].parameters,
            [
                ValueType::I32,
                ValueType::I64,
                ValueType::F32,
                ValueType::F64,
            ]
        );
        assert_eq!(
            declarations.types[0].results,
            [ValueType::I64, ValueType::F32]
        );
        assert!(declarations.function_types.is_empty());
    }

    #[test]
    fn reports_offsets_and_does_not_validate_type_indices() {
        let cases: &[(&[u8], usize, BinaryErrorKind)] = &[
            (
                b"\0asm\x01\0\0\0\x01\x00",
                10,
                BinaryErrorKind::UnexpectedEnd,
            ),
            (
                b"\0asm\x01\0\0\0\x01\x02\x01\x4e",
                11,
                BinaryErrorKind::UnsupportedTypeForm(0x4e),
            ),
            (
                b"\0asm\x01\0\0\0\x01\x04\x01\x60\x01\x70",
                13,
                BinaryErrorKind::UnsupportedValueType(0x70),
            ),
            (
                b"\0asm\x01\0\0\0\x03\x02\x00\x00",
                11,
                BinaryErrorKind::TrailingSectionData(3),
            ),
            (
                b"\0asm\x01\0\0\0\x03\x05\xff\xff\xff\xff\x0f",
                15,
                BinaryErrorKind::UnexpectedEnd,
            ),
        ];
        for (bytes, offset, kind) in cases {
            let error = decode_declarations(&scan_module(bytes).unwrap()).unwrap_err();
            assert_eq!((error.offset, &error.kind), (*offset, kind));
        }
        let unresolved = b"\0asm\x01\0\0\0\x03\x02\x01\x64";
        assert_eq!(
            decode_declarations(&scan_module(unresolved).unwrap())
                .unwrap()
                .function_types,
            [100]
        );
    }
}
