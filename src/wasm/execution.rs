//! Validation and execution of a small, straight-line function subset.
//!
//! Only functions without parameters or nonempty locals, returning either
//! nothing or one i32, are supported. An accepted function is not evidence
//! that its containing module is valid or instantiated.
//!
//! <https://webassembly.github.io/spec/core/valid/instructions.html#expressions>
//! <https://webassembly.github.io/spec/core/exec/instructions.html>

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
            Self::UnsupportedLocals => write!(formatter, "function locals are not supported"),
            Self::Decode(error) => write!(formatter, "{error}"),
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
    let returns_i32 = match (
        signature.parameters.as_slice(),
        signature.results.as_slice(),
    ) {
        ([], []) => false,
        ([], [ValueType::I32]) => true,
        _ => return Err(FunctionError::UnsupportedSignature),
    };
    if body.locals.iter().any(|group| group.count != 0) {
        return Err(FunctionError::UnsupportedLocals);
    }
    let expression = decode_expression(body).map_err(FunctionError::Decode)?;
    // Every supported instruction pushes exactly one i32; none consume values.
    let expected = usize::from(returns_i32);
    let actual = expression.instructions.len();
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
    })
}

/// Executes a previously checked function. `None` denotes a void result.
pub(crate) fn execute_function(function: &ValidatedFunction) -> Option<i32> {
    let mut top = None;
    for instruction in &function.expression.instructions {
        match instruction.kind {
            InstructionKind::I32Const(value) => top = Some(value),
        }
    }
    if function.returns_i32 { top } else { None }
}

#[cfg(test)]
mod tests {
    use super::{FunctionError, execute_function, validate_function};
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
        assert_eq!(execute_function(&function), Some(42));
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
                    parameters: vec![ValueType::I32],
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
            execute_function(&validate_function(&declarations, 0, &minus_one).unwrap()),
            Some(-1)
        );
        let empty = body(&[0x0b]);
        assert_eq!(
            execute_function(&validate_function(&declarations, 1, &empty).unwrap()),
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
        let unsupported = body(&[0x6a, 0x0b]);
        assert!(matches!(
            validate_function(&declarations, 0, &unsupported),
            Err(FunctionError::Decode(error)) if error.kind == BinaryErrorKind::UnsupportedOpcode(0x6a)
        ));
        let local_body = FunctionBody {
            locals: vec![LocalGroup {
                count: 1,
                value_type: ValueType::I32,
            }],
            ..empty
        };
        assert!(matches!(
            validate_function(&declarations, 1, &local_body),
            Err(FunctionError::UnsupportedLocals)
        ));
    }
}
