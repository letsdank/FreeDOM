//! A decoder for a small subset of Wasm function expressions.
//!
//! A successful decode only establishes that this subset was decoded; it does
//! not validate operand or result types, nor execute the expression. Nested
//! control instructions outside block, loop, if/else, branches and calls are unsupported.
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
    /// Targets are instruction indices, resolved during decoding.
    Block {
        returns_i32: bool,
        end_target: usize,
    },
    Loop {
        returns_i32: bool,
        end_target: usize,
    },
    If {
        returns_i32: bool,
        false_target: usize,
        end_target: usize,
    },
    Else {
        end_target: usize,
    },
    End,
    Br(u32),
    BrIf(u32),
    Call(u32),
    Drop,
    Select,
    LocalGet(u32),
    LocalSet(u32),
    LocalTee(u32),
    I32Load {
        align: u32,
        offset: u64,
    },
    I32Store {
        align: u32,
        offset: u64,
    },
    I32Load8S {
        align: u32,
        offset: u64,
    },
    I32Load8U {
        align: u32,
        offset: u64,
    },
    I32Store8 {
        align: u32,
        offset: u64,
    },
    I32Load16S {
        align: u32,
        offset: u64,
    },
    I32Load16U {
        align: u32,
        offset: u64,
    },
    I32Store16 {
        align: u32,
        offset: u64,
    },
    MemorySize(u32),
    MemoryGrow(u32),
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
    I32Clz,
    I32Ctz,
    I32Popcnt,
    I32Add,
    I32Sub,
    I32Mul,
    I32DivS,
    I32DivU,
    I32RemS,
    I32RemU,
    I32And,
    I32Or,
    I32Xor,
    I32Shl,
    I32ShrS,
    I32ShrU,
    I32Rotl,
    I32Rotr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Expression {
    pub(crate) instructions: Vec<Instruction>,
    /// The expression's terminating 0x0b is a delimiter, not an instruction.
    pub(crate) end_offset: usize,
}

fn read_memory_immediate(reader: &mut Reader<'_>) -> Result<(u32, u64), BinaryError> {
    let offset = reader.offset();
    let align = reader.read_u32_leb()?;
    if align >= 0x40 {
        return Err(BinaryError::new(
            offset,
            BinaryErrorKind::UnsupportedMemoryImmediate,
        ));
    }
    Ok((align, reader.read_u64_leb()?))
}

/// Decodes a complete function expression from an already framed body.
pub(crate) fn decode_expression(body: &FunctionBody<'_>) -> Result<Expression, BinaryError> {
    let mut reader = Reader::new(body.instructions, body.instructions_offset);
    let mut instructions: Vec<Instruction> = Vec::new();
    // Each entry tracks the opening control instruction and its optional else.
    let mut control: Vec<(usize, Option<usize>)> = Vec::new();
    loop {
        let offset = reader.offset();
        if reader.is_empty() {
            return Err(BinaryError::new(
                offset,
                BinaryErrorKind::MissingExpressionEnd,
            ));
        }
        let kind = match reader.read_byte()? {
            opcode @ (0x02 | 0x03 | 0x04) => {
                let type_offset = reader.offset();
                let returns_i32 = match reader.read_byte()? {
                    0x40 => false,
                    0x7f => true,
                    other => {
                        return Err(BinaryError::new(
                            type_offset,
                            BinaryErrorKind::UnsupportedBlockType(other),
                        ));
                    }
                };
                control
                    .try_reserve(1)
                    .map_err(|_| BinaryError::new(offset, BinaryErrorKind::AllocationFailed))?;
                control.push((instructions.len(), None));
                match opcode {
                    0x02 => InstructionKind::Block {
                        returns_i32,
                        end_target: usize::MAX,
                    },
                    0x03 => InstructionKind::Loop {
                        returns_i32,
                        end_target: usize::MAX,
                    },
                    _ => InstructionKind::If {
                        returns_i32,
                        false_target: usize::MAX,
                        end_target: usize::MAX,
                    },
                }
            }
            0x05 => {
                let Some((if_index, else_index)) = control.last_mut() else {
                    return Err(BinaryError::new(offset, BinaryErrorKind::UnexpectedElse));
                };
                if else_index.is_some() {
                    return Err(BinaryError::new(offset, BinaryErrorKind::UnexpectedElse));
                }
                if !matches!(instructions[*if_index].kind, InstructionKind::If { .. }) {
                    return Err(BinaryError::new(offset, BinaryErrorKind::UnexpectedElse));
                }
                *else_index = Some(instructions.len());
                let target = instructions.len() + 1;
                if let InstructionKind::If { false_target, .. } = &mut instructions[*if_index].kind
                {
                    *false_target = target;
                }
                InstructionKind::Else {
                    end_target: usize::MAX,
                }
            }
            0x0b if !control.is_empty() => {
                let (if_index, else_index) = control.pop().expect("checked nonempty");
                let target = instructions.len() + 1;
                match &mut instructions[if_index].kind {
                    InstructionKind::Block { end_target, .. }
                    | InstructionKind::Loop { end_target, .. } => *end_target = target,
                    InstructionKind::If {
                        false_target,
                        end_target,
                        ..
                    } => {
                        *end_target = target;
                        if else_index.is_none() {
                            *false_target = target;
                        }
                    }
                    _ => unreachable!("control stack contains only opening instructions"),
                }
                if let Some(index) = else_index {
                    if let InstructionKind::Else { end_target } = &mut instructions[index].kind {
                        *end_target = target;
                    }
                }
                InstructionKind::End
            }
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
            0x0c => InstructionKind::Br(reader.read_u32_leb()?),
            0x0d => InstructionKind::BrIf(reader.read_u32_leb()?),
            0x10 => InstructionKind::Call(reader.read_u32_leb()?),
            0x1a => InstructionKind::Drop,
            0x1b => InstructionKind::Select,
            0x21 => InstructionKind::LocalSet(reader.read_u32_leb()?),
            0x22 => InstructionKind::LocalTee(reader.read_u32_leb()?),
            0x28 => {
                let (align, offset) = read_memory_immediate(&mut reader)?;
                InstructionKind::I32Load { align, offset }
            }
            0x2c => {
                let (align, offset) = read_memory_immediate(&mut reader)?;
                InstructionKind::I32Load8S { align, offset }
            }
            0x2d => {
                let (align, offset) = read_memory_immediate(&mut reader)?;
                InstructionKind::I32Load8U { align, offset }
            }
            0x2e => {
                let (align, offset) = read_memory_immediate(&mut reader)?;
                InstructionKind::I32Load16S { align, offset }
            }
            0x2f => {
                let (align, offset) = read_memory_immediate(&mut reader)?;
                InstructionKind::I32Load16U { align, offset }
            }
            0x36 => {
                let (align, offset) = read_memory_immediate(&mut reader)?;
                InstructionKind::I32Store { align, offset }
            }
            0x3a => {
                let (align, offset) = read_memory_immediate(&mut reader)?;
                InstructionKind::I32Store8 { align, offset }
            }
            0x3b => {
                let (align, offset) = read_memory_immediate(&mut reader)?;
                InstructionKind::I32Store16 { align, offset }
            }
            0x3f => InstructionKind::MemorySize(reader.read_u32_leb()?),
            0x40 => InstructionKind::MemoryGrow(reader.read_u32_leb()?),
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
            0x67 => InstructionKind::I32Clz,
            0x68 => InstructionKind::I32Ctz,
            0x69 => InstructionKind::I32Popcnt,
            0x6a => InstructionKind::I32Add,
            0x6b => InstructionKind::I32Sub,
            0x6c => InstructionKind::I32Mul,
            0x6d => InstructionKind::I32DivS,
            0x6e => InstructionKind::I32DivU,
            0x6f => InstructionKind::I32RemS,
            0x70 => InstructionKind::I32RemU,
            0x71 => InstructionKind::I32And,
            0x72 => InstructionKind::I32Or,
            0x73 => InstructionKind::I32Xor,
            0x74 => InstructionKind::I32Shl,
            0x75 => InstructionKind::I32ShrS,
            0x76 => InstructionKind::I32ShrU,
            0x77 => InstructionKind::I32Rotl,
            0x78 => InstructionKind::I32Rotr,
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
            (
                &[0x1c, 1, 0x7f, 0x0b],
                10,
                BinaryErrorKind::UnsupportedOpcode(0x1c),
            ),
            (&[0x3f], 11, BinaryErrorKind::UnexpectedEnd),
            (&[0x40, 0x80], 12, BinaryErrorKind::UnexpectedEnd),
            (
                &[0x28, 0x40, 0, 0],
                11,
                BinaryErrorKind::UnsupportedMemoryImmediate,
            ),
            (&[0x28, 0x02], 12, BinaryErrorKind::UnexpectedEnd),
            (
                &[0x02, 0x40, 0x05, 0x0b, 0x0b],
                12,
                BinaryErrorKind::UnexpectedElse,
            ),
            (&[0x0c, 0x80], 12, BinaryErrorKind::UnexpectedEnd),
            (&[], 10, BinaryErrorKind::MissingExpressionEnd),
            (&[0x41], 11, BinaryErrorKind::UnexpectedEnd),
            (&[0x20, 0x80], 12, BinaryErrorKind::UnexpectedEnd),
            (&[0x41, 0x00], 12, BinaryErrorKind::MissingExpressionEnd),
            (&[0x79, 0x0b], 10, BinaryErrorKind::UnsupportedOpcode(0x79)),
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

    #[test]
    fn reports_malformed_if_structure_and_unsupported_block_types() {
        let cases: &[(&[u8], usize, BinaryErrorKind)] = &[
            (&[0x05, 0x0b], 10, BinaryErrorKind::UnexpectedElse),
            (&[0x04], 11, BinaryErrorKind::UnexpectedEnd),
            (
                &[0x04, 0x7e],
                11,
                BinaryErrorKind::UnsupportedBlockType(0x7e),
            ),
            (
                &[0x04, 0x40, 0x05, 0x05, 0x0b, 0x0b],
                13,
                BinaryErrorKind::UnexpectedElse,
            ),
            (
                &[0x04, 0x40, 0x0b],
                13,
                BinaryErrorKind::MissingExpressionEnd,
            ),
            (
                &[0x04, 0x40, 0x0b, 0x0b, 0x00],
                14,
                BinaryErrorKind::TrailingExpressionData,
            ),
        ];
        for (bytes, offset, kind) in cases {
            let body = crate::wasm::code::FunctionBody {
                locals: vec![],
                instructions_offset: 10,
                instructions: bytes,
            };
            let error = decode_expression(&body).unwrap_err();
            assert_eq!((error.offset, &error.kind), (*offset, kind), "{bytes:?}");
        }
    }
}
