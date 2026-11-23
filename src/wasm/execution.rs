//! Validation and execution of a small, straight-line function subset.
//!
//! Only functions with i32 parameters and locals, returning either
//! nothing or one i32, are supported. An accepted function is not evidence
//! that its containing module is valid or instantiated.
//!
//! <https://webassembly.github.io/spec/core/valid/instructions.html#expressions>
//! <https://webassembly.github.io/spec/core/exec/instructions.html>
//! <https://webassembly.github.io/spec/core/exec/numerics.html#op-iadd>
//! <https://webassembly.github.io/spec/core/exec/numerics.html#integer-operations>
//! <https://webassembly.github.io/spec/core/binary/instructions.html#numeric-instructions>

use std::{collections::HashMap, fmt};

use super::{
    BinaryError,
    code::FunctionBody,
    decode::{Declarations, ValueType},
    instructions::{Expression, InstructionKind, decode_expression},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FunctionError {
    UnknownTypeIndex(u32),
    /// The signature can be valid Wasm even though it is not supported here.
    UnsupportedSignature,
    UnsupportedLocals,
    Decode(BinaryError),
    InvalidLocalIndex {
        index: u32,
        offset: usize,
    },
    StackUnderflow {
        offset: usize,
        needed: usize,
        available: usize,
    },
    ResultStackMismatch {
        expected: usize,
        actual: usize,
        offset: usize,
    },
}

impl fmt::Display for FunctionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTypeIndex(index) => {
                write!(formatter, "unknown function type index {index}")
            }
            Self::UnsupportedSignature => write!(formatter, "unsupported function signature"),
            Self::UnsupportedLocals => write!(formatter, "unsupported function locals"),
            Self::Decode(error) => write!(formatter, "{error}"),
            Self::InvalidLocalIndex { index, offset } => {
                write!(formatter, "unknown local index {index} at byte {offset}")
            }
            Self::StackUnderflow {
                offset,
                needed,
                available,
            } => write!(
                formatter,
                "operand stack underflow at byte {offset}: needed {needed}, found {available}"
            ),
            Self::ResultStackMismatch {
                expected,
                actual,
                offset,
            } => write!(
                formatter,
                "function result stack mismatch at byte {offset}: expected {expected}, found {actual}"
            ),
        }
    }
}

impl std::error::Error for FunctionError {}

/// A decoded function checked against the subset above, not a validated module.
pub(crate) struct ValidatedFunction {
    expression: Expression,
    returns_i32: bool,
    max_stack_depth: usize,
    parameter_count: usize,
    local_index_count: u64,
}

/// Resolves the type and checks the expression's final operand stack.
pub(crate) fn validate_function(
    declarations: &Declarations,
    type_index: u32,
    body: &FunctionBody<'_>,
) -> Result<ValidatedFunction, FunctionError> {
    let signature = usize::try_from(type_index)
        .ok()
        .and_then(|index| declarations.types.get(index))
        .ok_or(FunctionError::UnknownTypeIndex(type_index))?;
    if signature
        .parameters
        .iter()
        .any(|value_type| *value_type != ValueType::I32)
    {
        return Err(FunctionError::UnsupportedSignature);
    }
    let returns_i32 = match signature.results.as_slice() {
        [] => false,
        [ValueType::I32] => true,
        _ => return Err(FunctionError::UnsupportedSignature),
    };
    if body
        .locals
        .iter()
        .any(|group| group.count != 0 && group.value_type != ValueType::I32)
    {
        return Err(FunctionError::UnsupportedLocals);
    }
    let parameter_count = signature.parameters.len();
    let local_index_count = body.locals.iter().try_fold(
        u64::try_from(parameter_count).map_err(|_| FunctionError::UnsupportedLocals)?,
        |count, group| {
            count
                .checked_add(u64::from(group.count))
                .ok_or(FunctionError::UnsupportedLocals)
        },
    )?;
    let expression = decode_expression(body).map_err(FunctionError::Decode)?;
    // All values in the currently supported subset are i32.
    let mut depth = 0_usize;
    let mut max_stack_depth = 0_usize;
    for instruction in &expression.instructions {
        match instruction.kind {
            InstructionKind::LocalGet(index) => {
                validate_local_index(index, local_index_count, instruction.offset)?;
                depth += 1;
                max_stack_depth = max_stack_depth.max(depth);
            }
            InstructionKind::LocalSet(index) | InstructionKind::LocalTee(index) => {
                validate_local_index(index, local_index_count, instruction.offset)?;
                if depth == 0 {
                    return Err(FunctionError::StackUnderflow {
                        offset: instruction.offset,
                        needed: 1,
                        available: 0,
                    });
                }
                if matches!(instruction.kind, InstructionKind::LocalSet(_)) {
                    depth -= 1;
                }
            }
            InstructionKind::I32Const(_) => {
                depth += 1;
                max_stack_depth = max_stack_depth.max(depth);
            }
            InstructionKind::I32Eqz => {
                if depth == 0 {
                    return Err(FunctionError::StackUnderflow {
                        offset: instruction.offset,
                        needed: 1,
                        available: 0,
                    });
                }
            }
            InstructionKind::I32Add
            | InstructionKind::I32Sub
            | InstructionKind::I32Mul
            | InstructionKind::I32DivS
            | InstructionKind::I32DivU
            | InstructionKind::I32RemS
            | InstructionKind::I32RemU
            | InstructionKind::I32Eq
            | InstructionKind::I32Ne
            | InstructionKind::I32LtS
            | InstructionKind::I32LtU
            | InstructionKind::I32GtS
            | InstructionKind::I32GtU
            | InstructionKind::I32LeS
            | InstructionKind::I32LeU
            | InstructionKind::I32GeS
            | InstructionKind::I32GeU => {
                if depth < 2 {
                    return Err(FunctionError::StackUnderflow {
                        offset: instruction.offset,
                        needed: 2,
                        available: depth,
                    });
                }
                depth -= 1;
            }
        }
    }
    let expected = usize::from(returns_i32);
    let actual = depth;
    if actual != expected {
        return Err(FunctionError::ResultStackMismatch {
            expected,
            actual,
            offset: expression.end_offset,
        });
    }
    Ok(ValidatedFunction {
        expression,
        returns_i32,
        max_stack_depth,
        parameter_count,
        local_index_count,
    })
}

fn validate_local_index(index: u32, count: u64, offset: usize) -> Result<(), FunctionError> {
    if u64::from(index) >= count {
        return Err(FunctionError::InvalidLocalIndex { index, offset });
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExecutionError {
    AllocationFailed,
    IntegerDivideByZero {
        offset: usize,
    },
    IntegerOverflow {
        offset: usize,
    },
    ArgumentCountMismatch {
        expected: usize,
        actual: usize,
    },
    /// Defensive check for inconsistencies between validation and execution.
    InvalidValidatedStack,
    InvalidValidatedLocal,
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AllocationFailed => write!(formatter, "cannot allocate execution state"),
            Self::IntegerDivideByZero { offset } => {
                write!(formatter, "integer divide by zero at byte {offset}")
            }
            Self::IntegerOverflow { offset } => {
                write!(formatter, "integer overflow at byte {offset}")
            }
            Self::ArgumentCountMismatch { expected, actual } => write!(
                formatter,
                "incorrect argument count: expected {expected}, found {actual}"
            ),
            Self::InvalidValidatedStack => write!(formatter, "invalid validated operand stack"),
            Self::InvalidValidatedLocal => write!(formatter, "invalid validated local index"),
        }
    }
}

impl std::error::Error for ExecutionError {}

/// Executes a previously checked function. `None` denotes a void result.
pub(crate) fn execute_function(
    function: &ValidatedFunction,
    arguments: &[i32],
) -> Result<Option<i32>, ExecutionError> {
    if arguments.len() != function.parameter_count {
        return Err(ExecutionError::ArgumentCountMismatch {
            expected: function.parameter_count,
            actual: arguments.len(),
        });
    }
    let mut stack: Vec<i32> = Vec::new();
    stack
        .try_reserve_exact(function.max_stack_depth)
        .map_err(|_| ExecutionError::AllocationFailed)?;
    // Store only modified locals, including modified parameters. Unmodified
    // locals are zero and need no allocation even when a group is very large.
    let mut modified_locals: HashMap<u32, i32> = HashMap::new();
    for instruction in &function.expression.instructions {
        match instruction.kind {
            InstructionKind::LocalGet(index) => {
                if u64::from(index) >= function.local_index_count {
                    return Err(ExecutionError::InvalidValidatedLocal);
                }
                let value = modified_locals.get(&index).copied().unwrap_or_else(|| {
                    usize::try_from(index)
                        .ok()
                        .and_then(|index| arguments.get(index))
                        .copied()
                        .unwrap_or(0)
                });
                stack.push(value);
            }
            InstructionKind::LocalSet(index) | InstructionKind::LocalTee(index) => {
                if u64::from(index) >= function.local_index_count {
                    return Err(ExecutionError::InvalidValidatedLocal);
                }
                let value = *stack.last().ok_or(ExecutionError::InvalidValidatedStack)?;
                if !modified_locals.contains_key(&index) {
                    modified_locals
                        .try_reserve(1)
                        .map_err(|_| ExecutionError::AllocationFailed)?;
                }
                modified_locals.insert(index, value);
                if matches!(instruction.kind, InstructionKind::LocalSet(_)) {
                    let _ = stack.pop();
                }
            }
            InstructionKind::I32Const(value) => stack.push(value),
            InstructionKind::I32Eqz => {
                let value = stack
                    .last_mut()
                    .ok_or(ExecutionError::InvalidValidatedStack)?;
                *value = i32::from(*value == 0);
            }
            InstructionKind::I32Eq
            | InstructionKind::I32Ne
            | InstructionKind::I32LtS
            | InstructionKind::I32LtU
            | InstructionKind::I32GtS
            | InstructionKind::I32GtU
            | InstructionKind::I32LeS
            | InstructionKind::I32LeU
            | InstructionKind::I32GeS
            | InstructionKind::I32GeU => {
                let right = stack.pop().ok_or(ExecutionError::InvalidValidatedStack)?;
                let left = stack.pop().ok_or(ExecutionError::InvalidValidatedStack)?;
                let value = match instruction.kind {
                    InstructionKind::I32Eq => left == right,
                    InstructionKind::I32Ne => left != right,
                    InstructionKind::I32LtS => left < right,
                    InstructionKind::I32LtU => (left as u32) < (right as u32),
                    InstructionKind::I32GtS => left > right,
                    InstructionKind::I32GtU => (left as u32) > (right as u32),
                    InstructionKind::I32LeS => left <= right,
                    InstructionKind::I32LeU => (left as u32) <= (right as u32),
                    InstructionKind::I32GeS => left >= right,
                    InstructionKind::I32GeU => (left as u32) >= (right as u32),
                    _ => return Err(ExecutionError::InvalidValidatedStack),
                };
                stack.push(i32::from(value));
            }
            InstructionKind::I32Add
            | InstructionKind::I32Sub
            | InstructionKind::I32Mul
            | InstructionKind::I32DivS
            | InstructionKind::I32DivU
            | InstructionKind::I32RemS
            | InstructionKind::I32RemU => {
                let right = stack.pop().ok_or(ExecutionError::InvalidValidatedStack)?;
                let left = stack.pop().ok_or(ExecutionError::InvalidValidatedStack)?;
                let offset = instruction.offset;
                let value = match instruction.kind {
                    InstructionKind::I32Add => left.wrapping_add(right),
                    InstructionKind::I32Sub => left.wrapping_sub(right),
                    InstructionKind::I32Mul => left.wrapping_mul(right),
                    InstructionKind::I32DivS
                    | InstructionKind::I32DivU
                    | InstructionKind::I32RemS
                    | InstructionKind::I32RemU
                        if right == 0 =>
                    {
                        return Err(ExecutionError::IntegerDivideByZero { offset });
                    }
                    InstructionKind::I32DivS => left
                        .checked_div(right)
                        .ok_or(ExecutionError::IntegerOverflow { offset })?,
                    InstructionKind::I32DivU => ((left as u32) / (right as u32)) as i32,
                    // MIN % -1 is defined as zero in Wasm, unlike signed division.
                    InstructionKind::I32RemS => left.wrapping_rem(right),
                    InstructionKind::I32RemU => ((left as u32) % (right as u32)) as i32,
                    _ => return Err(ExecutionError::InvalidValidatedStack),
                };
                stack.push(value);
            }
        }
    }
    match (function.returns_i32, stack.as_slice()) {
        (false, []) => Ok(None),
        (true, [value]) => Ok(Some(*value)),
        _ => Err(ExecutionError::InvalidValidatedStack),
    }
}

#[cfg(test)]
mod tests {
    use super::{ExecutionError, FunctionError, execute_function, validate_function};
    use crate::wasm::test_support::{expression, module};
    use crate::wasm::{
        BinaryErrorKind,
        binary::scan_module,
        code::{FunctionBody, LocalGroup, decode_bodies, link_functions},
        decode::{Declarations, FunctionType, ValueType, decode_declarations},
    };

    #[test]
    fn decodes_links_checks_and_executes_a_binary_function() {
        let bytes = module(r#"(module (func (export "f") (result i32) i32.const 42))"#);
        let module = scan_module(&bytes).unwrap();
        let declarations = decode_declarations(&module).unwrap();
        let bodies = decode_bodies(&module).unwrap();
        let linked = link_functions(&declarations, &bodies).unwrap();
        let (type_index, body) = linked.iter().next().unwrap();
        let function = validate_function(&declarations, type_index, body).unwrap();
        assert_eq!(execute_function(&function, &[]).unwrap(), Some(42));
    }

    #[test]
    fn checks_result_stack_and_reports_unsupported_features() {
        let declarations = Declarations {
            types: vec![
                FunctionType {
                    parameters: vec![],
                    results: vec![ValueType::I32],
                },
                FunctionType {
                    parameters: vec![],
                    results: vec![],
                },
                FunctionType {
                    parameters: vec![ValueType::I64],
                    results: vec![ValueType::I32],
                },
            ],
            function_types: vec![],
        };
        let body = |instructions| FunctionBody {
            locals: vec![],
            instructions_offset: 20,
            instructions,
        };
        let minus_one_instructions = expression("i32.const -1");
        let minus_one = body(&minus_one_instructions);
        assert_eq!(
            execute_function(
                &validate_function(&declarations, 0, &minus_one).unwrap(),
                &[]
            )
            .unwrap(),
            Some(-1)
        );
        let empty_instructions = expression("");
        let empty = body(&empty_instructions);
        assert_eq!(
            execute_function(&validate_function(&declarations, 1, &empty).unwrap(), &[]).unwrap(),
            None
        );
        assert!(matches!(
            validate_function(&declarations, 3, &empty),
            Err(FunctionError::UnknownTypeIndex(3))
        ));
        assert!(matches!(
            validate_function(&declarations, 2, &empty),
            Err(FunctionError::UnsupportedSignature)
        ));
        assert!(matches!(
            validate_function(&declarations, 0, &empty),
            Err(FunctionError::ResultStackMismatch {
                expected: 1,
                actual: 0,
                offset: 20,
            })
        ));
        let excess_instructions = expression("i32.const 1 i32.const 2");
        let excess = body(&excess_instructions);
        assert!(matches!(
            validate_function(&declarations, 0, &excess),
            Err(FunctionError::ResultStackMismatch {
                expected: 1,
                actual: 2,
                offset: 24,
            })
        ));
        let unsupported = body(&[0x71, 0x0b]);
        assert!(matches!(
            validate_function(&declarations, 0, &unsupported),
            Err(FunctionError::Decode(error)) if error.kind == BinaryErrorKind::UnsupportedOpcode(0x71)
        ));
        let local_body = FunctionBody {
            locals: vec![LocalGroup {
                count: 1,
                value_type: ValueType::I64,
            }],
            ..empty
        };
        assert!(matches!(
            validate_function(&declarations, 1, &local_body),
            Err(FunctionError::UnsupportedLocals)
        ));
    }

    #[test]
    fn executes_addition_and_rejects_stack_underflow() {
        let bytes = module(
            r#"(module (func (export "f") (result i32)
            i32.const 20 i32.const 22 i32.add))"#,
        );
        let module = scan_module(&bytes).unwrap();
        let declarations = decode_declarations(&module).unwrap();
        let bodies = decode_bodies(&module).unwrap();
        let linked = link_functions(&declarations, &bodies).unwrap();
        let (type_index, body) = linked.iter().next().unwrap();
        let function = validate_function(&declarations, type_index, body).unwrap();
        assert_eq!(execute_function(&function, &[]).unwrap(), Some(42));

        let overflow_instructions = expression("i32.const 2147483647 i32.const 1 i32.add");
        let overflow = FunctionBody {
            locals: vec![],
            instructions_offset: 20,
            instructions: &overflow_instructions,
        };
        assert_eq!(
            execute_function(
                &validate_function(&declarations, 0, &overflow).unwrap(),
                &[]
            )
            .unwrap(),
            Some(i32::MIN)
        );
        let underflow_instructions = expression("i32.const 1 i32.add");
        let underflow = FunctionBody {
            locals: vec![],
            instructions_offset: 20,
            instructions: &underflow_instructions,
        };
        assert!(matches!(
            validate_function(&declarations, 0, &underflow),
            Err(FunctionError::StackUnderflow {
                offset: 22,
                needed: 2,
                available: 1
            })
        ));
    }

    #[test]
    fn executes_i32_arithmetic_from_binary_and_reports_traps() {
        let template =
            module("(module (func (param i32 i32) (result i32) local.get 0 local.get 1 i32.add))");
        let cases = [
            ("i32.add", i32::MAX, 1, Ok(i32::MIN)),
            ("i32.sub", 3, 7, Ok(-4)),
            ("i32.sub", i32::MIN, 1, Ok(i32::MAX)),
            ("i32.mul", i32::MAX, 2, Ok(-2)),
            ("i32.div_s", -7, 2, Ok(-3)),
            ("i32.div_u", -1, 2, Ok(i32::MAX)),
            ("i32.rem_s", -7, 2, Ok(-1)),
            ("i32.rem_s", i32::MIN, -1, Ok(0)),
            ("i32.rem_u", -1, 2, Ok(1)),
            (
                "i32.div_s",
                i32::MIN,
                -1,
                Err(ExecutionError::IntegerOverflow {
                    offset: template.len() - 2,
                }),
            ),
            (
                "i32.div_s",
                1,
                0,
                Err(ExecutionError::IntegerDivideByZero {
                    offset: template.len() - 2,
                }),
            ),
            (
                "i32.div_u",
                1,
                0,
                Err(ExecutionError::IntegerDivideByZero {
                    offset: template.len() - 2,
                }),
            ),
            (
                "i32.rem_s",
                1,
                0,
                Err(ExecutionError::IntegerDivideByZero {
                    offset: template.len() - 2,
                }),
            ),
            (
                "i32.rem_u",
                1,
                0,
                Err(ExecutionError::IntegerDivideByZero {
                    offset: template.len() - 2,
                }),
            ),
        ];
        for (opcode, left, right, expected) in cases {
            let bytes = module(&format!(
                "(module (func (param i32 i32) (result i32) local.get 0 local.get 1 {opcode}))",
            ));
            let framed = scan_module(&bytes).unwrap();
            let declarations = decode_declarations(&framed).unwrap();
            let bodies = decode_bodies(&framed).unwrap();
            let linked = link_functions(&declarations, &bodies).unwrap();
            let (type_index, body) = linked.iter().next().unwrap();
            let function = validate_function(&declarations, type_index, body).unwrap();
            assert_eq!(
                execute_function(&function, &[left, right]),
                expected.map(Some),
                "instruciton {opcode}, operands {left}, {right}"
            );

            let instructions = expression(&format!("local.get 0 {opcode}"));
            let body = FunctionBody {
                locals: vec![],
                instructions_offset: 20,
                instructions: &instructions,
            };
            assert!(matches!(
                validate_function(&declarations, type_index, &body),
                Err(FunctionError::StackUnderflow {
                    offset: 22,
                    needed: 2,
                    available: 1,
                })
            ));
        }
    }

    #[test]
    fn executes_i32_comparisons_and_checks_stack_arity() {
        // Expected results for (-1, 1), (5, 5), and (1, -1), respectively.
        let cases = [
            ("i32.eq", 0x46, [0, 1, 0]),
            ("i32.ne", 0x47, [1, 0, 1]),
            ("i32.lt_s", 0x48, [1, 0, 0]),
            ("i32.lt_u", 0x49, [0, 0, 1]),
            ("i32.gt_s", 0x4a, [0, 0, 1]),
            ("i32.gt_u", 0x4b, [1, 0, 0]),
            ("i32.le_s", 0x4c, [1, 1, 0]),
            ("i32.le_u", 0x4d, [0, 1, 1]),
            ("i32.ge_s", 0x4e, [0, 1, 1]),
            ("i32.ge_u", 0x4f, [1, 1, 0]),
        ];
        for (op, opcode, expected) in cases {
            assert_eq!(expression(op), [opcode, 0x0b]);
            let bytes = module(&format!(
                "(module (func (param i32 i32) (result i32) local.get 0 local.get 1 {op}))"
            ));
            let framed = scan_module(&bytes).unwrap();
            let declarations = decode_declarations(&framed).unwrap();
            let bodies = decode_bodies(&framed).unwrap();
            let function = validate_function(&declarations, 0, &bodies[0]).unwrap();
            for (arguments, result) in [
                ([-1, 1], expected[0]),
                ([5, 5], expected[1]),
                ([1, -1], expected[2]),
            ] {
                assert_eq!(
                    execute_function(&function, &arguments),
                    Ok(Some(result)),
                    "{op} {arguments:?}"
                );
            }
            for (source, available) in [(op.to_owned(), 0), (format!("local.get 0 {op}"), 1)] {
                let instructions = expression(&source);
                let body = FunctionBody {
                    locals: vec![],
                    instructions_offset: 20,
                    instructions: &instructions,
                };
                assert!(matches!(
                    validate_function(&declarations, 0, &body),
                    Err(FunctionError::StackUnderflow {
                        offset,
                        needed: 2,
                        available: actual
                    }) if offset == 20 + available * 2 && actual == available
                ));
            }
        }

        assert_eq!(expression("i32.eqz"), [0x45, 0x0b]);
        let bytes = module("(module (func (param i32) (result i32) local.get 0 i32.eqz))");
        let framed = scan_module(&bytes).unwrap();
        let declarations = decode_declarations(&framed).unwrap();
        let bodies = decode_bodies(&framed).unwrap();
        let function = validate_function(&declarations, 0, &bodies[0]).unwrap();
        for (argument, expected) in [(0, 1), (-1, 0), (i32::MIN, 0), (i32::MAX, 0)] {
            assert_eq!(execute_function(&function, &[argument]), Ok(Some(expected)));
        }
        let bytes = module(
            "(module (func (param i32) (result i32)
            local.get 0 i32.eqz i32.const 41 i32.add))",
        );
        let framed = scan_module(&bytes).unwrap();
        let composed_declarations = decode_declarations(&framed).unwrap();
        let composed_bodies = decode_bodies(&framed).unwrap();
        let composed = validate_function(&composed_declarations, 0, &composed_bodies[0]).unwrap();
        assert_eq!(execute_function(&composed, &[0]), Ok(Some(42)));
        assert_eq!(execute_function(&composed, &[1]), Ok(Some(41)));
        let instructions = expression("i32.eqz");
        let body = FunctionBody {
            locals: vec![],
            instructions_offset: 20,
            instructions: &instructions,
        };
        assert!(matches!(
            validate_function(&declarations, 0, &body),
            Err(FunctionError::StackUnderflow {
                offset: 20,
                needed: 1,
                available: 0,
            })
        ));
    }

    #[test]
    fn executes_function_parameters_and_checks_local_indices() {
        let bytes = module(
            r#"(module (func (export "f") (param i32 i32) (result i32)
            local.get 0 local.get 1 i32.add))"#,
        );
        let module = scan_module(&bytes).unwrap();
        let declarations = decode_declarations(&module).unwrap();
        let bodies = decode_bodies(&module).unwrap();
        let linked = link_functions(&declarations, &bodies).unwrap();
        let (type_index, body) = linked.iter().next().unwrap();
        let function = validate_function(&declarations, type_index, body).unwrap();
        assert_eq!(execute_function(&function, &[20, 22]).unwrap(), Some(42));
        assert_eq!(
            execute_function(&function, &[i32::MAX, 1]).unwrap(),
            Some(i32::MIN)
        );
        assert_eq!(
            execute_function(&function, &[1]),
            Err(ExecutionError::ArgumentCountMismatch {
                expected: 2,
                actual: 1
            })
        );
        assert_eq!(
            execute_function(&function, &[1, 2, 3]),
            Err(ExecutionError::ArgumentCountMismatch {
                expected: 2,
                actual: 3,
            })
        );

        let bad_index_instructions = expression("local.get 2");
        let bad_index = FunctionBody {
            locals: vec![],
            instructions_offset: 20,
            instructions: &bad_index_instructions,
        };
        assert!(matches!(
            validate_function(&declarations, type_index, &bad_index),
            Err(FunctionError::InvalidLocalIndex {
                index: 2,
                offset: 20,
            })
        ));
        let huge_index_instructions = expression("local.get 4294967295");
        let huge_index = FunctionBody {
            locals: vec![],
            instructions_offset: 20,
            instructions: &huge_index_instructions,
        };
        assert!(matches!(
            validate_function(&declarations, type_index, &huge_index),
            Err(FunctionError::InvalidLocalIndex {
                index: u32::MAX,
                offset: 20,
            })
        ));
    }

    #[test]
    fn reads_zero_initialized_locals_after_parameters() {
        let bytes = module(
            r#"(module (func (export "f") (param i32) (result i32)
            (local i32 i32) local.get 0 local.get 2 i32.add))"#,
        );
        let module = scan_module(&bytes).unwrap();
        let declarations = decode_declarations(&module).unwrap();
        let bodies = decode_bodies(&module).unwrap();
        let linked = link_functions(&declarations, &bodies).unwrap();
        let (type_index, body) = linked.iter().next().unwrap();
        let function = validate_function(&declarations, type_index, body).unwrap();
        assert_eq!(execute_function(&function, &[57]).unwrap(), Some(57));
        assert_eq!(execute_function(&function, &[-5]).unwrap(), Some(-5));

        let bad_instructions = expression("local.get 3");
        let bad = FunctionBody {
            locals: vec![LocalGroup {
                count: 2,
                value_type: ValueType::I32,
            }],
            instructions_offset: 20,
            instructions: &bad_instructions,
        };
        assert!(matches!(
            validate_function(&declarations, type_index, &bad),
            Err(FunctionError::InvalidLocalIndex {
                index: 3,
                offset: 20,
            })
        ));

        let many_local_instructions = expression("local.get 1000000000");
        let many_locals = FunctionBody {
            locals: vec![LocalGroup {
                count: 1_000_000_000,
                value_type: ValueType::I32,
            }],
            instructions_offset: 20,
            instructions: &many_local_instructions,
        };
        let function = validate_function(&declarations, type_index, &many_locals).unwrap();
        assert_eq!(execute_function(&function, &[57]).unwrap(), Some(0));
    }

    #[test]
    fn executes_local_set_and_tee_with_isolated_call_state() {
        let bytes = module(
            r#"(module (func (export "f") (param i32) (result i32) (local i32)
            local.get 0 i32.const 1 i32.add local.tee 1 local.set 0
            local.get 1 local.get 0 i32.add))"#,
        );
        let module = scan_module(&bytes).unwrap();
        let declarations = decode_declarations(&module).unwrap();
        let bodies = decode_bodies(&module).unwrap();
        let linked = link_functions(&declarations, &bodies).unwrap();
        let (index, body) = linked.iter().next().unwrap();
        let function = validate_function(&declarations, index, body).unwrap();
        assert_eq!(execute_function(&function, &[20]).unwrap(), Some(42));
        assert_eq!(execute_function(&function, &[-5]).unwrap(), Some(-8));

        let overwrite_instructions = expression("i32.const 0 local.set 0 local.get 0");
        let overwrite_with_zero = FunctionBody {
            locals: vec![],
            instructions_offset: 0,
            instructions: &overwrite_instructions,
        };
        let zero_function = validate_function(&declarations, index, &overwrite_with_zero).unwrap();
        assert_eq!(execute_function(&zero_function, &[7]).unwrap(), Some(0));

        let huge_instructions =
            expression("i32.const 42 local.set 1000000000 local.get 1000000000");
        let huge = FunctionBody {
            locals: vec![LocalGroup {
                count: 1_000_000_000,
                value_type: ValueType::I32,
            }],
            instructions_offset: 0,
            instructions: &huge_instructions,
        };
        let huge_function = validate_function(&declarations, index, &huge).unwrap();
        assert_eq!(execute_function(&huge_function, &[0]).unwrap(), Some(42));
    }

    #[test]
    fn validates_local_writes_before_execution() {
        let declarations = Declarations {
            types: vec![FunctionType {
                parameters: vec![ValueType::I32],
                results: vec![ValueType::I32],
            }],
            function_types: vec![],
        };
        for opcode in ["local.set", "local.tee"] {
            let instructions = expression(&format!("{opcode} 0"));
            let body = FunctionBody {
                locals: vec![],
                instructions_offset: 20,
                instructions: &instructions,
            };
            assert!(matches!(
                validate_function(&declarations, 0, &body),
                Err(FunctionError::StackUnderflow {
                    offset: 20,
                    needed: 1,
                    available: 0,
                })
            ));
            let instructions = expression(&format!("i32.const 1 {opcode} 1"));
            let body = FunctionBody {
                locals: vec![],
                instructions_offset: 20,
                instructions: &instructions,
            };
            assert!(matches!(
                validate_function(&declarations, 0, &body),
                Err(FunctionError::InvalidLocalIndex {
                    index: 1,
                    offset: 22,
                })
            ));
        }
    }
}
