//! Checked defined functions for direct calls, not a complete Wasm instance.
//!
//! Imports are rejected because they change the function index space. Other
//! sections (including exports and start) are not validated or instantiated.
//!
//! <https://webassembly.github.io/spec/core/binary/modules.html>

use super::{
    BinaryError,
    binary::FramedModule,
    code::{FunctionCountMismatch, decode_bodies, link_functions},
    decode::decode_declarations,
    execution::{ExecutionError, FunctionError, ValidatedFunction, execute_function_set},
};

#[derive(Debug)]
pub(crate) enum FunctionSetError {
    UnsupportedImports,
    Binary(BinaryError),
    FunctionCount(FunctionCountMismatch),
    Function { index: usize, error: FunctionError },
    AllocationFailed,
}

pub(crate) struct FunctionSet {
    functions: Vec<ValidatedFunction>,
}

impl FunctionSet {
    /// Builds and checks defined functions only, rejecting any import section.
    pub(crate) fn from_framed(module: &FramedModule<'_>) -> Result<Self, FunctionSetError> {
        if module.sections.iter().any(|section| section.id == 2) {
            return Err(FunctionSetError::UnsupportedImports);
        }
        let declarations = decode_declarations(module).map_err(FunctionSetError::Binary)?;
        let bodies = decode_bodies(module).map_err(FunctionSetError::Binary)?;
        let linked =
            link_functions(&declarations, &bodies).map_err(FunctionSetError::FunctionCount)?;
        let mut functions = Vec::new();
        functions
            .try_reserve_exact(bodies.len())
            .map_err(|_| FunctionSetError::AllocationFailed)?;
        for (index, (type_index, body)) in linked.iter().enumerate() {
            functions.push(
                super::execution::validate_function(&declarations, type_index, body)
                    .map_err(|error| FunctionSetError::Function { index, error })?,
            );
        }
        Ok(Self { functions })
    }

    /// Invokes a defined function with a shared host instruction budget.
    pub(crate) fn execute(
        &self,
        index: u32,
        arguments: &[i32],
        fuel: u64,
    ) -> Result<Option<i32>, ExecutionError> {
        execute_function_set(&self.functions, index, arguments, fuel)
    }
}

#[cfg(test)]
mod tests {
    use super::{FunctionSet, FunctionSetError};
    use crate::wasm::{
        binary::scan_module,
        execution::{ExecutionError, FunctionError, execute_function_with_fuel},
        test_support::{expression, module},
    };

    #[test]
    fn calls_functions_with_isolated_arguments_and_returns() {
        assert_eq!(expression("call 128"), [0x10, 0x80, 0x01, 0x0b]);
        let bytes = module(
            "(module
            (func (param i32 i32) (result i32) local.get 0 local.get 1 i32.sub)
            (func (param i32 i32) (result i32) local.get 0 local.get 1 call 0)
            (func (param i32) i32.const 0 local.set 0)
            (func (result i32) i32.const 42 i32.const 7 call 2)
            (func (param i32) (result i32) local.get 0 call 2 local.get 0))",
        );
        let framed = scan_module(&bytes).unwrap();
        let set = FunctionSet::from_framed(&framed).unwrap();
        assert_eq!(set.execute(1, &[9, 4], 100), Ok(Some(5)));
        assert_eq!(set.execute(1, &[4, 9], 100), Ok(Some(-5)));
        assert_eq!(set.execute(3, &[], 100), Ok(Some(42)));
        assert_eq!(set.execute(4, &[9], 100), Ok(Some(9)));
        assert!(matches!(
            set.execute(1, &[1], 100),
            Err(ExecutionError::ArgumentCountMismatch {
                expected: 2,
                actual: 1
            })
        ));
        assert_eq!(
            set.execute(5, &[], 100),
            Err(ExecutionError::InvalidFunctionIndex(5))
        );
    }

    #[test]
    fn recursive_calls_share_fuel_and_do_not_use_the_native_stack() {
        let bytes = module(
            "(module (func (param i32) (result i32)
            local.get 0 i32.eqz if (result i32) i32.const 0 else
                local.get 0 local.get 0 i32.const 1 i32.sub call 0 i32.add
            end))",
        );
        let framed = scan_module(&bytes).unwrap();
        let set = FunctionSet::from_framed(&framed).unwrap();
        assert_eq!(set.execute(0, &[5], 1000), Ok(Some(15)));
        assert_eq!(set.execute(0, &[5], 10), Err(ExecutionError::FuelExhausted));
        assert_eq!(
            set.execute(0, &[2000], 100_000),
            Err(ExecutionError::CallDepthExceeded)
        );
    }

    #[test]
    fn call_propagates_traps_and_uses_the_shared_budget() {
        let bytes = module(
            "(module
            (func (result i32) i32.const 42)
            (func (result i32) call 0)
            (func (result i32) i32.const 1 i32.const 0 i32.div_s)
            (func (result i32) call 2))",
        );
        let framed = scan_module(&bytes).unwrap();
        let set = FunctionSet::from_framed(&framed).unwrap();
        assert_eq!(set.execute(1, &[], 2), Ok(Some(42)));
        assert_eq!(set.execute(1, &[], 1), Err(ExecutionError::FuelExhausted));
        assert_eq!(
            execute_function_with_fuel(&set.functions[1], &[], 2),
            Err(ExecutionError::CallRequiresFunctionSet)
        );
        assert!(matches!(
            set.execute(3, &[], 10),
            Err(ExecutionError::IntegerDivideByZero { .. })
        ));
    }

    #[test]
    fn rejects_invalid_calls_and_import_sections() {
        let cases = [
            ("(module (func call 1))", true),
            ("(module (func (param i32)) (func call 0))", false),
        ];
        for (wat, invalid_index) in cases {
            let bytes = module(wat);
            let framed = scan_module(&bytes).unwrap();
            let error = match FunctionSet::from_framed(&framed) {
                Ok(_) => panic!("unexpectedly accepted {wat}"),
                Err(error) => error,
            };
            assert!(
                if invalid_index {
                    matches!(
                        error,
                        FunctionSetError::Function {
                            error: FunctionError::InvalidFunctionIndex { index: 1, .. },
                            ..
                        }
                    )
                } else {
                    matches!(
                        error,
                        FunctionSetError::Function {
                            error: FunctionError::StackUnderflow {
                                needed: 1,
                                available: 0,
                                ..
                            },
                            ..
                        }
                    )
                },
                "{wat}: {error:?}"
            );
        }

        let mut bytes = module("(module (func))");
        let framed = scan_module(&bytes).unwrap();
        let function_section_offset = framed
            .sections
            .iter()
            .find(|section| section.id == 3)
            .unwrap()
            .offset;
        // Even an empty import section is outside this partial function-set API.
        bytes.splice(function_section_offset..function_section_offset, [2, 1, 0]);
        let framed = scan_module(&bytes).unwrap();
        assert!(matches!(
            FunctionSet::from_framed(&framed),
            Err(FunctionSetError::UnsupportedImports)
        ));
    }
}
