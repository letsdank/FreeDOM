//! Checked defined functions for direct calls, not a complete Wasm instance.
//!
//! Imports are rejected because they change the function index space. Export
//! names and function indices are checked; non-function export indices and
//! other sections, including start, are not validated or instantiated.
//!
//! <https://webassembly.github.io/spec/core/binary/modules.html>

use std::{collections::HashMap, fmt};

use super::{
    BinaryError,
    binary::FramedModule,
    code::{FunctionCountMismatch, decode_bodies, link_functions},
    decode::decode_declarations,
    execution::{ExecutionError, FunctionError, ValidatedFunction, execute_function_set},
    export::{ExportKind, decode_exports},
};

#[derive(Debug)]
pub(crate) enum FunctionSetError {
    UnsupportedImports,
    Binary(BinaryError),
    FunctionCount(FunctionCountMismatch),
    Function { index: usize, error: FunctionError },
    DuplicateExportName { offset: usize },
    InvalidFunctionExport { index: u32, offset: usize },
    AllocationFailed,
}

pub(crate) struct FunctionSet {
    functions: Vec<ValidatedFunction>,
    exports: HashMap<String, ExportDescriptor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExportDescriptor {
    kind: ExportKind,
    index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExportCallError {
    UnknownExport,
    NotFunction(ExportKind),
    Execution(ExecutionError),
}

impl fmt::Display for ExportCallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownExport => write!(formatter, "unknown export"),
            Self::NotFunction(kind) => write!(formatter, "export is not a function: {kind:?}"),
            Self::Execution(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for ExportCallError {}

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
        let mut exports = HashMap::new();
        for export in decode_exports(module).map_err(FunctionSetError::Binary)? {
            if exports.contains_key(export.name) {
                return Err(FunctionSetError::DuplicateExportName {
                    offset: export.name_offset,
                });
            }
            if export.kind == ExportKind::Function
                && usize::try_from(export.index)
                    .ok()
                    .filter(|index| *index < functions.len())
                    .is_none()
            {
                return Err(FunctionSetError::InvalidFunctionExport {
                    index: export.index,
                    offset: export.index_offset,
                });
            }
            exports
                .try_reserve(1)
                .map_err(|_| FunctionSetError::AllocationFailed)?;
            let mut name = String::new();
            name.try_reserve_exact(export.name.len())
                .map_err(|_| FunctionSetError::AllocationFailed)?;
            name.push_str(export.name);
            exports.insert(
                name,
                ExportDescriptor {
                    kind: export.kind,
                    index: export.index,
                },
            );
        }
        Ok(Self { functions, exports })
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

    /// Invokes a function export by its exact, case-sensitive UTF-8 name.
    pub(crate) fn execute_export(
        &self,
        name: &str,
        arguments: &[i32],
        fuel: u64,
    ) -> Result<Option<i32>, ExportCallError> {
        let export = self
            .exports
            .get(name)
            .ok_or(ExportCallError::UnknownExport)?;
        if export.kind != ExportKind::Function {
            return Err(ExportCallError::NotFunction(export.kind));
        }
        self.execute(export.index, arguments, fuel)
            .map_err(ExportCallError::Execution)
    }
}

#[cfg(test)]
mod tests {
    use super::{ExportCallError, FunctionSet, FunctionSetError};
    use crate::wasm::{
        binary::scan_module,
        execution::{ExecutionError, FunctionError, execute_function_with_fuel},
        export::ExportKind,
        test_support::{expression, module},
    };

    #[test]
    fn calls_function_exports_by_exact_name() {
        let set = {
            let bytes = module(
                r#"(module
                (func (result i32) i32.const 2)
                (func (export "sum") (export "Сумма") (export "") (param i32) (result i32)
                    local.get 0 call 0 i32.add))"#,
            );
            FunctionSet::from_framed(&scan_module(&bytes).unwrap()).unwrap()
        };
        for name in ["sum", "Сумма", ""] {
            assert_eq!(set.execute_export(name, &[40], 10), Ok(Some(42)));
            assert_eq!(
                set.execute_export("SUM", &[40], 10),
                Err(ExportCallError::UnknownExport)
            );
            assert_eq!(
                set.execute_export("sum", &[], 10),
                Err(ExportCallError::Execution(
                    ExecutionError::ArgumentCountMismatch {
                        expected: 1,
                        actual: 0
                    }
                ))
            );
            assert_eq!(
                set.execute_export("sum", &[40], 0),
                Err(ExportCallError::Execution(ExecutionError::FuelExhausted))
            );
        }
    }

    #[test]
    fn rejects_duplicate_and_invalid_function_exports() {
        let bytes = module(r#"(module (func (export "f")) (func (export "f")))"#);
        let framed = scan_module(&bytes).unwrap();
        assert!(matches!(
            FunctionSet::from_framed(&framed),
            Err(FunctionSetError::DuplicateExportName { .. })
        ));

        let mut bytes = module(r#"(module (func (export "f")))"#);
        let framed = scan_module(&bytes).unwrap();
        let export = super::decode_exports(&framed).unwrap()[0];
        let offset = export.index_offset;
        bytes[offset] = 1;
        let framed = scan_module(&bytes).unwrap();
        assert!(matches!(
            FunctionSet::from_framed(&framed),
            Err(FunctionSetError::InvalidFunctionExport {
                index: 1,
                offset: found
            }) if found == offset
        ));
    }

    #[test]
    fn distinguishes_non_function_exports() {
        let mut bytes = module(r#"(module (func (export "memory")))"#);
        let framed = scan_module(&bytes).unwrap();
        let offset = framed
            .sections
            .iter()
            .find(|section| section.id == 7)
            .unwrap()
            .offset;
        // Add a real memory section, then change the export descriptor to memory.
        bytes.splice(offset..offset, [5, 3, 1, 0, 1]);
        let framed = scan_module(&bytes).unwrap();
        let export = super::decode_exports(&framed).unwrap()[0];
        let kind_offset = export.index_offset - 1;
        bytes[kind_offset] = 2;
        let framed = scan_module(&bytes).unwrap();
        let set = FunctionSet::from_framed(&framed).unwrap();
        assert_eq!(
            set.execute_export("memory", &[], 10),
            Err(ExportCallError::NotFunction(ExportKind::Memory))
        );
    }

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
