//! Code-section framing and association with defined-function declarations.
//!
//! Instruction bytes are borrowed as-is; neither expression boundaries nor
//! instructions are validated here. Locals remain grouped rather than being
//! expanded, so a large encoded count cannot cause a proportional allocation.
//!
//! <https://webassembly.github.io/spec/core/binary/modules.html#code-section>

use super::{
    BinaryError, BinaryErrorKind,
    binary::FramedModule,
    decode::{Declarations, ValueType, read_value_type},
    reader::Reader,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LocalGroup {
    pub(crate) count: u32,
    pub(crate) value_type: ValueType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FunctionBody<'a> {
    pub(crate) locals: Vec<LocalGroup>,
    /// Offset and uninterpreted bytes after the local declarations.
    pub(crate) instructions_offset: usize,
    pub(crate) instructions: &'a [u8],
}

/// Reads code-section bodies without interpreting or validating instructions.
pub(crate) fn decode_bodies<'a>(
    module: &FramedModule<'a>,
) -> Result<Vec<FunctionBody<'a>>, BinaryError> {
    let Some(section) = module.sections.iter().find(|section| section.id == 10) else {
        return Ok(Vec::new());
    };
    let mut reader = Reader::new(section.payload, section.payload_offset);
    let count = reader.read_u32_leb()?;
    let mut bodies = Vec::new();
    for _ in 0..count {
        let length_offset = reader.offset();
        let length = usize::try_from(reader.read_u32_leb()?)
            .map_err(|_| BinaryError::new(length_offset, BinaryErrorKind::SizeOverflow))?;
        let body_offset = reader.offset();
        let bytes = reader.read_bytes(length)?;
        let mut body_reader = Reader::new(bytes, body_offset);
        let group_count = body_reader.read_u32_leb()?;
        let mut locals = Vec::new();
        let mut total = 0_u32;
        for _ in 0..group_count {
            let group_offset = body_reader.offset();
            let count = body_reader.read_u32_leb()?;
            let value_type = read_value_type(&mut body_reader)?;
            total = total
                .checked_add(count)
                .ok_or_else(|| BinaryError::new(group_offset, BinaryErrorKind::TooManyLocals))?;
            locals
                .try_reserve(1)
                .map_err(|_| BinaryError::new(group_offset, BinaryErrorKind::AllocationFailed))?;
            locals.push(LocalGroup { count, value_type });
        }
        let body = FunctionBody {
            locals,
            instructions_offset: body_reader.offset(),
            instructions: body_reader.remaining(),
        };
        bodies
            .try_reserve(1)
            .map_err(|_| BinaryError::new(body_offset, BinaryErrorKind::AllocationFailed))?;
        bodies.push(body);
    }
    if !reader.is_empty() {
        return Err(BinaryError::new(
            reader.offset(),
            BinaryErrorKind::TrailingSectionData(10),
        ));
    }
    Ok(bodies)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FunctionCountMismatch {
    pub(crate) declarations: usize,
    pub(crate) bodies: usize,
}

/// A matched view of defined functions only; imports are not included.
pub(crate) struct DefinedFunctions<'a, 'bytes> {
    type_indices: &'a [u32],
    bodies: &'a [FunctionBody<'bytes>],
}

impl<'a, 'bytes> DefinedFunctions<'a, 'bytes> {
    pub(crate) fn iter(
        &self,
    ) -> impl ExactSizeIterator<Item = (u32, &'a FunctionBody<'bytes>)> + '_ {
        self.type_indices.iter().copied().zip(self.bodies.iter())
    }
}

/// Matches function and code entries by position. Type indices remain unresolved.
pub(crate) fn link_functions<'a, 'bytes>(
    declarations: &'a Declarations,
    bodies: &'a [FunctionBody<'bytes>],
) -> Result<DefinedFunctions<'a, 'bytes>, FunctionCountMismatch> {
    if declarations.function_types.len() != bodies.len() {
        return Err(FunctionCountMismatch {
            declarations: declarations.function_types.len(),
            bodies: bodies.len(),
        });
    }
    Ok(DefinedFunctions {
        type_indices: &declarations.function_types,
        bodies,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        BinaryErrorKind, FunctionCountMismatch, LocalGroup, ValueType, decode_bodies,
        link_functions,
    };
    use crate::wasm::{binary::scan_module, decode::decode_declarations};

    #[test]
    fn connects_multiple_function_bodies_without_expanding_locals() {
        // (func (result i32) i32.const 42), followed by a function with locals.
        let bytes=b"\0asm\x01\0\0\0\x01\x05\x01\x60\x00\x01\x7f\x03\x03\x02\x00\x00\x0a\x0f\x02\x04\x00\x41\x2a\x0b\x08\x02\x03\x7f\x02\x7e\x41\x00\x0b";
        let module = scan_module(bytes).unwrap();
        let declarations = decode_declarations(&module).unwrap();
        let bodies = decode_bodies(&module).unwrap();
        let linked = link_functions(&declarations, &bodies).unwrap();
        let functions = linked.iter().collect::<Vec<_>>();
        assert_eq!(functions.len(), 2);
        assert_eq!(functions[0].0, 0);
        assert_eq!(functions[0].1.instructions, [0x41, 42, 0x0b]);
        assert_eq!(
            functions[1].1.locals,
            [
                LocalGroup {
                    count: 3,
                    value_type: ValueType::I32,
                },
                LocalGroup {
                    count: 2,
                    value_type: ValueType::I64,
                }
            ]
        );
        assert_eq!(functions[1].1.instructions, [0x41, 0, 0x0b]);
        assert_eq!(functions[1].1.instructions_offset, bytes.len() - 3);

        // Instruction bytes are deliberately uninterpreted, even without end.
        let incomplete = b"\0asm\x01\0\0\0\x0a\x04\x01\x02\x00\x41";
        let bodies = decode_bodies(&scan_module(incomplete).unwrap()).unwrap();
        assert_eq!(bodies[0].instructions, [0x41]);
    }

    #[test]
    fn checks_boundaries_local_counts_and_function_counts() {
        let cases: &[(&[u8], BinaryErrorKind)] = &[
            (
                b"\0asm\x01\0\0\0\x0a\x02\x01\x01",
                BinaryErrorKind::UnexpectedEnd,
            ),
            (
                b"\0asm\x01\0\0\0\x0a\x02\x00\x00",
                BinaryErrorKind::TrailingSectionData(10),
            ),
            (
                b"\0asm\x01\0\0\0\x0a\x05\x01\x03\x01\x01\x70",
                BinaryErrorKind::UnsupportedValueType(0x70),
            ),
            (
                b"\0asm\x01\0\0\0\x0a\x0b\x01\x09\x02\xff\xff\xff\xff\x0f\x7f\x01\x7f",
                BinaryErrorKind::TooManyLocals,
            ),
        ];
        for (bytes, kind) in cases {
            assert_eq!(
                &decode_bodies(&scan_module(bytes).unwrap())
                    .unwrap_err()
                    .kind,
                kind
            );
        }
        let bytes = b"\0asm\x01\0\0\0\x03\x02\x01\x00";
        let module = scan_module(bytes).unwrap();
        let declarations = decode_declarations(&module).unwrap();
        let bodies = decode_bodies(&module).unwrap();
        assert!(matches!(
            link_functions(&declarations, &bodies),
            Err(FunctionCountMismatch {
                declarations: 1,
                bodies: 0
            })
        ))
    }
}
