//! Validation and execution of a small, straight-line function subset.
//!
//! Only functions with i32 parameters and locals, returning either
//! nothing or one i32, are supported. An accepted function is not evidence
//! that its containing module is valid or instantiated.
//!
//! <https://webassembly.github.io/spec/core/valid/instructions.html#expressions>
//! <https://webassembly.github.io/spec/core/exec/instructions.html>
//! <https://webassembly.github.io/spec/core/exec/numerics.html#op-iadd>

use std::fmt;

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
                if u64::from(index) >= local_index_count {
                    return Err(FunctionError::InvalidLocalIndex {
                        index,
                        offset: instruction.offset,
                    });
                }
                depth += 1;
                max_stack_depth = max_stack_depth.max(depth);
            }
            InstructionKind::I32Const(_) => {
                depth += 1;
                max_stack_depth = max_stack_depth.max(depth);
            }
            InstructionKind::I32Add => {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExecutionError {
    AllocationFailed,
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
            Self::AllocationFailed => write!(formatter, "cannot allocate operand stack"),
            Self::ArgumentCountMismatch { expected, actual } => write!(
                formatter,
                "incorrect argument count: expected {expected}, found {actual}"
            ),
            Self::InvalidValidatedStack => write!(formatter, "invalid validated operand stack"),
            Self::InvalidValidatedLocal => write!(formatter, "invalid validated local stack"),
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
    for instruction in &function.expression.instructions {
        match instruction.kind {
            InstructionKind::LocalGet(index) => {
                if u64::from(index) >= function.local_index_count {
                    return Err(ExecutionError::InvalidValidatedLocal);
                }
                // Unmodified i32 locals are implicitly zero; only parameters
                // need storage until local mutation instructions are supported.
                let value = usize::try_from(index)
                    .ok()
                    .and_then(|index| arguments.get(index))
                    .copied()
                    .unwrap_or(0);
                stack.push(value);
            }
            InstructionKind::I32Const(value) => stack.push(value),
            InstructionKind::I32Add => {
                let right = stack.pop().ok_or(ExecutionError::InvalidValidatedStack)?;
                let left = stack.pop().ok_or(ExecutionError::InvalidValidatedStack)?;
                stack.push(left.wrapping_add(right));
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
    use crate::wasm::{
        BinaryErrorKind,
        binary::scan_module,
        code::{FunctionBody, LocalGroup, decode_bodies, link_functions},
        decode::{Declarations, FunctionType, ValueType, decode_declarations},
    };

    #[test]
    fn decodes_links_checks_and_executes_a_binary_function() {
        // (module (func (export "f") (result i32) i32.const 42))
        let bytes=b"\0asm\x01\0\0\0\x01\x05\x01\x60\x00\x01\x7f\x03\x02\x01\x00\x07\x05\x01\x01f\x00\x00\x0a\x06\x01\x04\x00\x41\x2a\x0b";
        let module = scan_module(bytes).unwrap();
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
        let minus_one = body(&[0x41, 0x7f, 0x0b]);
        assert_eq!(
            execute_function(
                &validate_function(&declarations, 0, &minus_one).unwrap(),
                &[]
            )
            .unwrap(),
            Some(-1)
        );
        let empty = body(&[0x0b]);
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
        let excess = body(&[0x41, 0x01, 0x41, 0x02, 0x0b]);
        assert!(matches!(
            validate_function(&declarations, 0, &excess),
            Err(FunctionError::ResultStackMismatch {
                expected: 1,
                actual: 2,
                offset: 24,
            })
        ));
        let unsupported = body(&[0x6b, 0x0b]);
        assert!(matches!(
            validate_function(&declarations, 0, &unsupported),
            Err(FunctionError::Decode(error)) if error.kind == BinaryErrorKind::UnsupportedOpcode(0x6b)
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
        // (module (func (export "f") (result i32) i32.const 20 i32.const 22 i32.add))
        let bytes=b"\0asm\x01\0\0\0\x01\x05\x01\x60\x00\x01\x7f\x03\x02\x01\x00\x07\x05\x01\x01f\x00\x00\x0a\x09\x01\x07\x00\x41\x14\x41\x16\x6a\x0b";
        let module = scan_module(bytes).unwrap();
        let declarations = decode_declarations(&module).unwrap();
        let bodies = decode_bodies(&module).unwrap();
        let linked = link_functions(&declarations, &bodies).unwrap();
        let (type_index, body) = linked.iter().next().unwrap();
        let function = validate_function(&declarations, type_index, body).unwrap();
        assert_eq!(execute_function(&function, &[]).unwrap(), Some(42));

        let overflow = FunctionBody {
            locals: vec![],
            instructions_offset: 20,
            instructions: &[0x41, 0xff, 0xff, 0xff, 0xff, 0x07, 0x41, 0x01, 0x6a, 0x0b],
        };
        assert_eq!(
            execute_function(
                &validate_function(&declarations, 0, &overflow).unwrap(),
                &[]
            )
            .unwrap(),
            Some(i32::MIN)
        );
        let underflow = FunctionBody {
            locals: vec![],
            instructions_offset: 20,
            instructions: &[0x41, 0x01, 0x6a, 0x0b],
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
    fn executes_function_parameters_and_checks_local_indices() {
        // (func (export "f") (param i32 i32) (result i32) local.get 0 local.get 1 i32.add)
        let bytes=b"\0asm\x01\0\0\0\x01\x07\x01\x60\x02\x7f\x7f\x01\x7f\x03\x02\x01\x00\x07\x05\x01\x01f\x00\x00\x0a\x09\x01\x07\x00\x20\x00\x20\x01\x6a\x0b";
        let module = scan_module(bytes).unwrap();
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

        let bad_index = FunctionBody {
            locals: vec![],
            instructions_offset: 20,
            instructions: &[0x20, 0x02, 0x0b],
        };
        assert!(matches!(
            validate_function(&declarations, type_index, &bad_index),
            Err(FunctionError::InvalidLocalIndex {
                index: 2,
                offset: 20,
            })
        ));
        let huge_index = FunctionBody {
            locals: vec![],
            instructions_offset: 20,
            instructions: &[0x20, 0xff, 0xff, 0xff, 0xff, 0x0f, 0x0b],
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
        // One i32 parameter, two i32 locals, return parameter + second local.
        let bytes=b"\0asm\x01\0\0\0\x01\x06\x01\x60\x01\x7f\x01\x7f\x03\x02\x01\x00\x07\x05\x01\x01f\x00\x00\x0a\x0b\x01\x09\x01\x02\x7f\x20\x00\x20\x02\x6a\x0b";
        let module = scan_module(bytes).unwrap();
        let declarations = decode_declarations(&module).unwrap();
        let bodies = decode_bodies(&module).unwrap();
        let linked = link_functions(&declarations, &bodies).unwrap();
        let (type_index, body) = linked.iter().next().unwrap();
        let function = validate_function(&declarations, type_index, body).unwrap();
        assert_eq!(execute_function(&function, &[57]).unwrap(), Some(57));
        assert_eq!(execute_function(&function, &[-5]).unwrap(), Some(-5));

        let bad = FunctionBody {
            locals: vec![LocalGroup {
                count: 2,
                value_type: ValueType::I32,
            }],
            instructions_offset: 20,
            instructions: &[0x20, 0x03, 0x0b],
        };
        assert!(matches!(
            validate_function(&declarations, type_index, &bad),
            Err(FunctionError::InvalidLocalIndex {
                index: 3,
                offset: 20,
            })
        ));

        let many_locals = FunctionBody {
            locals: vec![LocalGroup {
                count: 1_000_000_000,
                value_type: ValueType::I32,
            }],
            instructions_offset: 20,
            instructions: &[0x20, 0x80, 0x94, 0xeb, 0xdc, 0x03, 0x0b],
        };
        let function = validate_function(&declarations, type_index, &many_locals).unwrap();
        assert_eq!(execute_function(&function, &[57]).unwrap(), Some(0));
    }
}
