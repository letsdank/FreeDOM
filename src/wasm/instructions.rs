//! A decoder for a small, straight-line subset of Wasm function expressions.
//!
//! A successful decode only establishes that this subset was decoded; it does
//! not validate operand or result types, nor execute the expression. Nested
//! control instructions and other opcodes are currently unsupported.
//!
//! <https://webassembly.github.io/spec/core/binary/instructions.html>

use super::{BinaryError, BinaryErrorKind, code::FunctionBody, reader::Reader};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Instruction {
    /// Absolute byte offset of the opcode in the original module.
    pub(crate) offset: usize,
    pub(crate) kind: InstructionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstructionKind {
    LocalGet(u32),
    LocalSet(u32),
    LocalTee(u32),
    I32Const(i32),
    I32Eqz,
    I32Eq,
    I32Ne,
    I32LtS,
    I32LtU,
    I32GtS,
    I32GtU,
    I32LeS,
    I32LeU,
    I32GeS,
    I32GeU,
    I32Add,
    I32Sub,
    I32Mul,
    I32DivS,
    I32DivU,
    I32RemS,
    I32RemU,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Expression {
    pub(crate) instructions: Vec<Instruction>,
    /// The expression's terminating 0x0b is a delimiter, not an instruction.
    pub(crate) end_offset: usize,
}

/// Decodes a complete function expression from an already framed body.
pub(crate) fn decode_expression(body: &FunctionBody<'_>) -> Result<Expression, BinaryError> {
    let mut reader = Reader::new(body.instructions, body.instructions_offset);
    let mut instructions = Vec::new();
    loop {
        let offset = reader.offset();
        if reader.is_empty() {
            return Err(BinaryError::new(
                offset,
                BinaryErrorKind::MissingExpressionEnd,
            ));
        }
        let kind = match reader.read_byte()? {
            0x0b => {
                if !reader.is_empty() {
                    return Err(BinaryError::new(
                        reader.offset(),
                        BinaryErrorKind::TrailingExpressionData,
                    ));
                }
                return Ok(Expression {
                    instructions,
                    end_offset: offset,
                });
            }
            0x20 => InstructionKind::LocalGet(reader.read_u32_leb()?),
            0x21 => InstructionKind::LocalSet(reader.read_u32_leb()?),
            0x22 => InstructionKind::LocalTee(reader.read_u32_leb()?),
            0x41 => InstructionKind::I32Const(reader.read_i32_leb()?),
            0x45 => InstructionKind::I32Eqz,
            0x46 => InstructionKind::I32Eq,
            0x47 => InstructionKind::I32Ne,
            0x48 => InstructionKind::I32LtS,
            0x49 => InstructionKind::I32LtU,
            0x4a => InstructionKind::I32GtS,
            0x4b => InstructionKind::I32GtU,
            0x4c => InstructionKind::I32LeS,
            0x4d => InstructionKind::I32LeU,
            0x4e => InstructionKind::I32GeS,
            0x4f => InstructionKind::I32GeU,
            0x6a => InstructionKind::I32Add,
            0x6b => InstructionKind::I32Sub,
            0x6c => InstructionKind::I32Mul,
            0x6d => InstructionKind::I32DivS,
            0x6e => InstructionKind::I32DivU,
            0x6f => InstructionKind::I32RemS,
            0x70 => InstructionKind::I32RemU,
            opcode => {
                return Err(BinaryError::new(
                    offset,
                    BinaryErrorKind::UnsupportedOpcode(opcode),
                ));
            }
        };
        instructions
            .try_reserve(1)
            .map_err(|_| BinaryError::new(offset, BinaryErrorKind::AllocationFailed))?;
        instructions.push(Instruction { offset, kind });
    }
}

#[cfg(test)]
mod tests {
    use super::{BinaryErrorKind, Instruction, InstructionKind, decode_expression};
    use crate::wasm::test_support::module;
    use crate::wasm::{binary::scan_module, code::decode_bodies};

    #[test]
    fn decodes_real_function_body_and_preserves_offsets() {
        let bytes = module(r#"(module (func (export "f") (result i32) i32.const 42))"#);
        let framed = scan_module(&bytes).unwrap();
        let bodies = decode_bodies(&framed).unwrap();
        let expression = decode_expression(&bodies[0]).unwrap();
        assert_eq!(
            expression.instructions,
            [Instruction {
                offset: bytes.len() - 3,
                kind: InstructionKind::I32Const(42)
            }]
        );
        assert_eq!(expression.end_offset, bytes.len() - 1);
    }

    #[test]
    fn distinguishes_unsupported_truncated_and_trailing_input() {
        let cases: &[(&[u8], usize, BinaryErrorKind)] = &[
            (&[], 10, BinaryErrorKind::MissingExpressionEnd),
            (&[0x41], 11, BinaryErrorKind::UnexpectedEnd),
            (&[0x20, 0x80], 12, BinaryErrorKind::UnexpectedEnd),
            (&[0x41, 0x00], 12, BinaryErrorKind::MissingExpressionEnd),
            (&[0x71, 0x0b], 10, BinaryErrorKind::UnsupportedOpcode(0x71)),
            (&[0x0b, 0x00], 11, BinaryErrorKind::TrailingExpressionData),
            (
                &[0x41, 0x80, 0x80, 0x80, 0x80, 0x08, 0x0b],
                11,
                BinaryErrorKind::InvalidSignedLeb128,
            ),
        ];
        for (bytes, offset, kind) in cases {
            let body = crate::wasm::code::FunctionBody {
                locals: vec![],
                instructions_offset: 10,
                instructions: bytes,
            };
            let error = decode_expression(&body).unwrap_err();
            assert_eq!((error.offset, &error.kind), (*offset, kind));
        }
    }
}
