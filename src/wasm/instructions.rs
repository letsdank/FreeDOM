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
    I32Const(i32),
    I32Add,
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
            0x41 => InstructionKind::I32Const(reader.read_i32_leb()?),
            0x6a => InstructionKind::I32Add,
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
    use crate::wasm::{binary::scan_module, code::decode_bodies};

    #[test]
    fn decodes_real_function_body_and_preserves_offsets() {
        let bytes=b"\0asm\x01\0\0\0\x01\x05\x01\x60\x00\x01\x7f\x03\x02\x01\x00\x07\x05\x01\x01f\x00\x00\x0a\x06\x01\x04\x00\x41\x2a\x0b";
        let module = scan_module(bytes).unwrap();
        let bodies = decode_bodies(&module).unwrap();
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
            (&[0x6b, 0x0b], 10, BinaryErrorKind::UnsupportedOpcode(0x6b)),
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
