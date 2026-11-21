//! Decoding of function signatures and defined-function type indices.
//!
//! Only function types with numeric value types are supported so far. Other
//! type forms and value types are reported as unsupported, not as invalid.
//! Decoding does not validate indices, imports, code bodies, or the module.
//!
//! <https://webassembly.github.io/spec/core/binary/modules.html#type-section>
//! <https://webassembly.github.io/spec/core/binary/modules.html#function-section>
//! <https://webassembly.github.io/spec/core/binary/types.html#function-types>

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
    use super::{BinaryErrorKind, FunctionType, ValueType, decode_declarations};
    use crate::wasm::binary::scan_module;

    #[test]
    fn decodes_a_function_module_and_numeric_signatures() {
        // (module (func (export "f") (result i32) i32.const 42))
        let module=b"\0asm\x01\0\0\0\x01\x05\x01\x60\x00\x01\x7f\x03\x02\x01\x00\x07\x05\x01\x01f\x00\x00\x0a\x06\x01\x04\x00\x41\x2a\x0b";
        let declarations = decode_declarations(&scan_module(module).unwrap()).unwrap();
        assert_eq!(
            declarations.types,
            [FunctionType {
                parameters: vec![],
                results: vec![ValueType::I32],
            }]
        );
        assert_eq!(declarations.function_types, [0]);

        let numeric = b"\0asm\x01\0\0\0\x01\x0a\x01\x60\x04\x7f\x7e\x7d\x7c\x02\x7e\x7d";
        let declarations = decode_declarations(&scan_module(numeric).unwrap()).unwrap();
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
