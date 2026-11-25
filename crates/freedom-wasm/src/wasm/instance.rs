//! Checked defined functions for direct calls, not a complete Wasm instance.
//!
//! Imports are rejected because they change the function index space. Export
//! names, function indices and memory indices are checked. Other export
//! indices and sections are not fully validated. Active data segments are
//! initialized before the optional start function; imports and passive segments
//! are unsupported.
//!
//! <https://webassembly.github.io/spec/core/binary/modules.html>
//! <https://webassembly.github.io/spec/core/valid/modules.html#start-function>

use std::{collections::HashMap, fmt};

use super::{
    BinaryError,
    binary::FramedModule,
    code::{FunctionCountMismatch, decode_bodies, link_functions},
    data::{DataError, decode_data},
    decode::{decode_declarations, decode_start},
    execution::{ExecutionError, FunctionError, ValidatedFunction, execute_function_set},
    export::{ExportKind, decode_exports},
    global::{Global, GlobalError, decode_globals},
    memory::{LinearMemory, MemoryError, PAGE_SIZE, decode_memory},
};

#[derive(Debug)]
pub(crate) enum FunctionSetError {
    UnsupportedImports,
    InvalidStartFunction {
        index: u32,
        offset: usize,
    },
    InvalidStartSignature {
        index: u32,
        offset: usize,
    },
    StartExecution(ExecutionError),
    Global(GlobalError),
    Data(DataError),
    InvalidDataMemory {
        index: u32,
        offset: usize,
    },
    DataOutOfBounds {
        segment: usize,
        address: u32,
        length: usize,
    },
    Memory(MemoryError),
    Binary(BinaryError),
    FunctionCount(FunctionCountMismatch),
    Function {
        index: usize,
        error: FunctionError,
    },
    DuplicateExportName {
        offset: usize,
    },
    InvalidFunctionExport {
        index: u32,
        offset: usize,
    },
    InvalidMemoryExport {
        index: u32,
        offset: usize,
    },
    InvalidGlobalExport {
        index: u32,
        offset: usize,
    },
    AllocationFailed,
}

pub(crate) struct FunctionSet {
    functions: Vec<ValidatedFunction>,
    exports: HashMap<String, ExportDescriptor>,
    memory: Option<LinearMemory>,
    globals: Vec<Global>,
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
    /// Builds defined functions, rejecting imports; startup has a 1,000,000-step host budget.
    pub(crate) fn from_framed(module: &FramedModule<'_>) -> Result<Self, FunctionSetError> {
        Self::from_framed_with_start_fuel(module, 1_000_000)
    }

    /// Builds a function set with a separate host instruction budget for startup.
    pub(crate) fn from_framed_with_start_fuel(
        module: &FramedModule<'_>,
        start_fuel: u64,
    ) -> Result<Self, FunctionSetError> {
        if module.sections.iter().any(|section| section.id == 2) {
            return Err(FunctionSetError::UnsupportedImports);
        }
        let start = decode_start(module).map_err(FunctionSetError::Binary)?;
        let globals = decode_globals(module).map_err(FunctionSetError::Global)?;
        let memory_type = decode_memory(module).map_err(FunctionSetError::Memory)?;
        let data = decode_data(module).map_err(FunctionSetError::Data)?;
        for (segment_index, segment) in data.iter().enumerate() {
            if segment.memory_index != 0 || memory_type.is_none() {
                return Err(FunctionSetError::InvalidDataMemory {
                    index: segment.memory_index,
                    offset: segment.memory_index_offset,
                });
            }
            let size = u64::from(memory_type.expect("checked memory").minimum) * PAGE_SIZE as u64;
            let end = u64::try_from(segment.bytes.len())
                .ok()
                .and_then(|length| u64::from(segment.address).checked_add(length));
            if end.is_none_or(|end| end > size) {
                return Err(FunctionSetError::DataOutOfBounds {
                    segment: segment_index,
                    address: segment.address,
                    length: segment.bytes.len(),
                });
            }
        }
        let declarations = decode_declarations(module).map_err(FunctionSetError::Binary)?;
        let bodies = decode_bodies(module).map_err(FunctionSetError::Binary)?;
        let linked =
            link_functions(&declarations, &bodies).map_err(FunctionSetError::FunctionCount)?;
        if let Some((index, offset)) = start {
            let type_index = usize::try_from(index)
                .ok()
                .and_then(|index| declarations.function_types.get(index))
                .ok_or(FunctionSetError::InvalidStartFunction { index, offset })?;
            if let Some(ty) = usize::try_from(*type_index)
                .ok()
                .and_then(|index| declarations.types.get(index))
                && (!ty.parameters.is_empty() || !ty.results.is_empty())
            {
                return Err(FunctionSetError::InvalidStartSignature { index, offset });
            }
        }
        let mut functions = Vec::new();
        functions
            .try_reserve_exact(bodies.len())
            .map_err(|_| FunctionSetError::AllocationFailed)?;
        for (index, (type_index, body)) in linked.iter().enumerate() {
            functions.push(
                super::execution::validate_function_with_context(
                    &declarations,
                    type_index,
                    body,
                    memory_type.is_some(),
                    &globals,
                )
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
            if export.kind == ExportKind::Memory && (export.index != 0 || memory_type.is_none()) {
                return Err(FunctionSetError::InvalidMemoryExport {
                    index: export.index,
                    offset: export.index_offset,
                });
            }
            if export.kind == ExportKind::Global
                && usize::try_from(export.index)
                    .ok()
                    .filter(|index| *index < globals.len())
                    .is_none()
            {
                return Err(FunctionSetError::InvalidGlobalExport {
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
        let mut memory = memory_type
            .map(LinearMemory::new)
            .transpose()
            .map_err(FunctionSetError::Memory)?;
        for (index, segment) in data.iter().enumerate() {
            memory
                .as_mut()
                .expect("data memory checked")
                .initialize(segment.address, segment.bytes)
                .ok_or(FunctionSetError::DataOutOfBounds {
                    segment: index,
                    address: segment.address,
                    length: segment.bytes.len(),
                })?;
        }
        let mut set = Self {
            functions,
            exports,
            memory,
            globals,
        };
        // The start function runs after active data segments, before exposure.
        // https://webassembly.github.io/spec/core/exec/modules.html#instantiation
        if let Some((index, _)) = start {
            set.execute(index, &[], start_fuel)
                .map_err(FunctionSetError::StartExecution)?;
        }
        Ok(set)
    }

    /// Invokes a defined function with a shared host instruction budget.
    pub(crate) fn execute(
        &mut self,
        index: u32,
        arguments: &[i32],
        fuel: u64,
    ) -> Result<Option<i32>, ExecutionError> {
        execute_function_set(
            &self.functions,
            &mut self.memory,
            &mut self.globals,
            index,
            arguments,
            fuel,
        )
    }

    /// Invokes a function export by its exact, case-sensitive UTF-8 name.
    pub(crate) fn execute_export(
        &mut self,
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
    fn globals_are_initialized_before_start_and_shared_only_within_an_instance() {
        let bytes = module(
            r#"(module
            (global (mut i32) (i32.const 40))
            (global i32 (i32.const 2))
            (func call 3)
            (func (export "get") (result i32) global.get 0)
            (func (export "increment") global.get 0 i32.const 1 i32.add global.set 0)
            (func global.get 0 global.get 1 i32.add global.set 0)
            (start 0))"#,
        );
        let framed = scan_module(&bytes).unwrap();
        let mut first = FunctionSet::from_framed(&framed).unwrap();
        let mut second = FunctionSet::from_framed(&framed).unwrap();
        assert_eq!(first.execute_export("get", &[], 20).unwrap(), Some(42));
        first.execute_export("increment", &[], 20).unwrap();
        assert_eq!(first.execute_export("get", &[], 20).unwrap(), Some(43));
        assert_eq!(second.execute_export("get", &[], 20).unwrap(), Some(42));
    }

    #[test]
    fn global_access_requires_a_valid_index_and_mutability() {
        for (wat, immutable) in [
            ("(module (func (result i32) global.get 0))", false),
            (
                "(module (global (mut i32) (i32.const 0)) (func i32.const 1 global.set 1))",
                false,
            ),
            (
                "(module (global i32 (i32.const 0)) (func i32.const 1 global.set 0))",
                true,
            ),
        ] {
            let bytes = module(wat);
            let result = FunctionSet::from_framed(&scan_module(&bytes).unwrap());
            assert!(
                if immutable {
                    matches!(
                        result,
                        Err(FunctionSetError::Function {
                            error: FunctionError::ImmutableGlobal { .. },
                            ..
                        })
                    )
                } else {
                    matches!(
                        result,
                        Err(FunctionSetError::Function {
                            error: FunctionError::InvalidGlobalIndex { .. },
                            ..
                        })
                    )
                },
                "{wat}"
            );
        }
        let bytes = module("(module (global (mut i32) (i32.const 0)) (func global.set 0))");
        assert!(matches!(
            FunctionSet::from_framed(&scan_module(&bytes).unwrap()),
            Err(FunctionSetError::Function {
                error: FunctionError::StackUnderflow { .. },
                ..
            })
        ));
        let mut bytes = module(r#"(module (global (export "g") i32 (i32.const -1)))"#);
        let framed = scan_module(&bytes).unwrap();
        let offset = super::decode_exports(&framed).unwrap()[0].index_offset;
        assert!(FunctionSet::from_framed(&framed).is_ok());
        bytes[offset] = 1;
        assert!(matches!(
            FunctionSet::from_framed(&scan_module(&bytes).unwrap()),
            Err(FunctionSetError::InvalidGlobalExport { index: 1, .. })
        ));
    }

    #[test]
    fn start_initializes_memory_after_data_once_per_instance() {
        let bytes = module(
            r#"(module
            (memory 1)
            (data (i32.const 0) "\29\00\00\00")
            (func call 2)
            (func (export "get") (result i32) i32.const 0 i32.load)
            (func i32.const 0 i32.const 0 i32.load i32.const 1 i32.add i32.store)
            (start 0))"#,
        );
        let framed = scan_module(&bytes).unwrap();
        let mut first = FunctionSet::from_framed(&framed).unwrap();
        let mut second = FunctionSet::from_framed(&framed).unwrap();
        for set in [&mut first, &mut second] {
            assert_eq!(set.execute_export("get", &[], 20).unwrap(), Some(42));
            assert_eq!(set.execute_export("get", &[], 20).unwrap(), Some(42));
        }
    }

    #[test]
    fn start_checks_index_signature_and_execution_budget() {
        for wat in ["(module (start 0))", "(module (func) (start 1))"] {
            let bytes = module(wat);
            assert!(
                matches!(
                    FunctionSet::from_framed(&scan_module(&bytes).unwrap()),
                    Err(FunctionSetError::InvalidStartFunction { .. })
                ),
                "{wat}"
            );
        }
        for wat in [
            "(module (func (param i32)) (start 0))",
            "(module (func (result i32) i32.const 1) (start 0))",
        ] {
            let bytes = module(wat);
            assert!(
                matches!(
                    FunctionSet::from_framed(&scan_module(&bytes).unwrap()),
                    Err(FunctionSetError::InvalidStartSignature { .. })
                ),
                "{wat}"
            );
        }
        let bytes = module("(module (func unreachable) (start 0))");
        assert!(matches!(
            FunctionSet::from_framed(&scan_module(&bytes).unwrap()),
            Err(FunctionSetError::StartExecution(
                ExecutionError::Unreachable { .. }
            ))
        ));
        let bytes = module("(module (func loop br 0 end) (start 0))");
        assert!(matches!(
            FunctionSet::from_framed_with_start_fuel(&scan_module(&bytes).unwrap(), 8),
            Err(FunctionSetError::StartExecution(
                ExecutionError::FuelExhausted
            ))
        ));
        let bytes = module("(module)");
        assert!(FunctionSet::from_framed_with_start_fuel(&scan_module(&bytes).unwrap(), 0).is_ok());
    }

    #[test]
    fn calls_function_exports_by_exact_name() {
        let mut set = {
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
        let mut set = FunctionSet::from_framed(&framed).unwrap();
        assert_eq!(
            set.execute_export("memory", &[], 10),
            Err(ExportCallError::NotFunction(ExportKind::Memory))
        );
    }

    #[test]
    fn memory_is_shared_by_calls_and_persists_after_traps() {
        let bytes = module(
            r#"(module
            (memory (export "mem") 1 2)
            (func (export "put") (param i32 i32) local.get 0 local.get 1 i32.store offset=2 align=1)
            (func (export "get") (param i32) (result i32) local.get 0 i32.load offset=2 align=1)
            (func (export "raw") (param i32) (result i32) local.get 0 i32.load align=1)
            (func (export "trap-after-write") (local i32)
                i32.const 0 i32.const 99 i32.store
                i32.const 65536 i32.load local.set 0)
            (func (export "overflow") (result i32) i32.const 1 i32.load offset=4294967295)
            (func (export "indirect") (param i32 i32) local.get 0 local.get 1 call 0))"#,
        );
        let framed = scan_module(&bytes).unwrap();
        let mut set = FunctionSet::from_framed(&framed).unwrap();
        let mut other = FunctionSet::from_framed(&framed).unwrap();
        assert_eq!(set.execute_export("get", &[1], 100), Ok(Some(0)));
        assert_eq!(
            set.execute_export("indirect", &[1, 0x01020304], 100),
            Ok(None)
        );
        assert_eq!(set.execute_export("get", &[1], 100), Ok(Some(0x01020304)));
        assert_eq!(set.execute_export("raw", &[2], 100), Ok(Some(0x02030400)));
        assert_eq!(other.execute_export("get", &[1], 100), Ok(Some(0)));

        assert_eq!(set.execute_export("put", &[65530, 42], 100), Ok(None));
        assert!(matches!(
            set.execute_export("put", &[65531, 7], 100),
            Err(ExportCallError::Execution(
                ExecutionError::MemoryOutOfBounds { .. }
            ))
        ));
        assert_eq!(set.execute_export("raw", &[65532], 100), Ok(Some(42)));
        for (name, args) in [
            ("raw", &[-1][..]),
            ("overflow", &[][..]),
            ("trap-after-write", &[][..]),
        ] {
            assert!(
                matches!(
                    set.execute_export(name, args, 100),
                    Err(ExportCallError::Execution(
                        ExecutionError::MemoryOutOfBounds { .. }
                    ))
                ),
                "{name}"
            );
        }
        assert_eq!(set.execute_export("raw", &[0], 100), Ok(Some(99)));
        assert_eq!(
            set.execute_export("mem", &[], 100),
            Err(ExportCallError::NotFunction(ExportKind::Memory))
        );
    }

    #[test]
    fn executes_branch_tables_across_blocks_loops_and_the_function() {
        let bytes = module(
            r#"(module
            (func (export "choose") (param i32) (result i32)
                block (result i32) block (result i32)
                    i32.const 99 i32.const 40 local.get 0 br_table 0 1 2
                end i32.const 1 i32.add
                end i32.const 1 i32.add)
            (func (export "default") (result i32)
                block (result i32) i32.const 42 i32.const 99 br_table 0 end)
            (func (export "loop") (param i32) (result i32)
                block loop local.get 0 i32.const 1 i32.sub local.tee 0 br_table 1 0 end end
                local.get 0)
            (func (export "unreachable") (result i32) unreachable br_table 0))"#,
        );
        let framed = scan_module(&bytes).unwrap();
        let mut set = FunctionSet::from_framed(&framed).unwrap();
        for (selector, expected) in [(0, 42), (1, 41), (2, 40), (-1, 40), (i32::MIN, 40)] {
            assert_eq!(
                set.execute_export("choose", &[selector], 100),
                Ok(Some(expected))
            );
        }
        assert_eq!(set.execute_export("default", &[], 100), Ok(Some(42)));
        assert_eq!(set.execute_export("loop", &[3], 100), Ok(Some(0)));
        assert!(matches!(
            set.execute_export("unreachable", &[], 100),
            Err(ExportCallError::Execution(
                ExecutionError::Unreachable { .. }
            ))
        ));
    }

    #[test]
    fn validates_every_branch_table_target_and_stack() {
        let cases = [
            ("(module (func i32.const 0 br_table 1 0))", "index"),
            ("(module (func i32.const 0 br_table 0 1))", "index"),
            (
                "(module (func (result i32) block (result i32) block i32.const 0 br_table 0 1 end i32.const 0 end))",
                "arity",
            ),
            ("(module (func br_table 0))", "selector"),
            (
                "(module (func (result i32) i32.const 0 br_table 0))",
                "result",
            ),
        ];
        for (wat, expected) in cases {
            let bytes = module(wat);
            let framed = scan_module(&bytes).unwrap();
            let error = match FunctionSet::from_framed(&framed) {
                Ok(_) => panic!("unexpectedly accepted {wat}"),
                Err(error) => error,
            };
            assert!(
                match expected {
                    "index" => matches!(
                        error,
                        FunctionSetError::Function {
                            error: FunctionError::InvalidLabelIndex { index: 1, .. },
                            ..
                        }
                    ),
                    "arity" => matches!(
                        error,
                        FunctionSetError::Function {
                            error: FunctionError::BranchTableArityMismatch {
                                expected: 1,
                                actual: 0,
                                ..
                            },
                            ..
                        }
                    ),
                    "selector" | "result" => matches!(
                        error,
                        FunctionSetError::Function {
                            error: FunctionError::StackUnderflow {
                                needed: 1,
                                available: 0,
                                ..
                            },
                            ..
                        }
                    ),
                    _ => false,
                },
                "{wat}: {error:?}"
            );
        }
    }

    #[test]
    fn executes_return_nop_and_unreachable() {
        use crate::wasm::{
            code::decode_bodies,
            instructions::{InstructionKind, decode_expression},
        };
        let bytes = module(
            r#"(module
            (func (export "nested") (result i32)
                i32.const 10 block i32.const 20 i32.const 42 return end i32.const 1 i32.add)
            (func (export "void") i32.const 9 return)
            (func (export "call") (result i32) call 0 i32.const 1 i32.add)
            (func (export "trap") (param i32) (result i32)
                local.get 0 if (result i32) unreachable else i32.const 9 end)
            (func (export "nop") (result i32) nop i32.const 7))"#,
        );
        let framed = scan_module(&bytes).unwrap();
        let bodies = decode_bodies(&framed).unwrap();
        let expression = decode_expression(&bodies[3]).unwrap();
        let trap_offset = expression
            .instructions
            .iter()
            .find(|instruction| matches!(instruction.kind, InstructionKind::Unreachable))
            .unwrap()
            .offset;
        let mut set = FunctionSet::from_framed(&framed).unwrap();
        assert_eq!(set.execute_export("nested", &[], 100), Ok(Some(42)));
        assert_eq!(set.execute_export("void", &[], 100), Ok(None));
        assert_eq!(set.execute_export("call", &[], 100), Ok(Some(43)));
        assert_eq!(set.execute_export("trap", &[0], 100), Ok(Some(9)));
        assert_eq!(
            set.execute_export("trap", &[-1], 100),
            Err(ExportCallError::Execution(ExecutionError::Unreachable {
                offset: trap_offset
            }))
        );
        assert_eq!(set.execute_export("nop", &[], 2), Ok(Some(7)));
        assert_eq!(
            set.execute_export("nop", &[], 1),
            Err(ExportCallError::Execution(ExecutionError::FuelExhausted))
        );
    }

    #[test]
    fn executes_drop_and_eager_select() {
        let bytes = module(
            r#"(module
            (func (export "choose") (param i32 i32 i32) (result i32)
                local.get 0 local.get 1 local.get 2 select)
            (func (export "discard") (result i32)
                i32.const 20 i32.const 999 drop i32.const 22 i32.add)
            (func (export "void") i32.const 1 drop)
            (func (export "eager") (result i32)
                i32.const 42 i32.const 1 i32.const 0 i32.div_s i32.const 1 select))"#,
        );
        let framed = scan_module(&bytes).unwrap();
        let mut set = FunctionSet::from_framed(&framed).unwrap();
        for (condition, expected) in [(0, 22), (1, 11), (-1, 11), (i32::MIN, 11)] {
            assert_eq!(
                set.execute_export("choose", &[11, 22, condition], 100),
                Ok(Some(expected))
            );
        }
        assert_eq!(set.execute_export("discard", &[], 100), Ok(Some(42)));
        assert_eq!(set.execute_export("void", &[], 100), Ok(None));
        // Both candidate-producing instructions execute before select.
        assert!(matches!(
            set.execute_export("eager", &[], 100),
            Err(ExportCallError::Execution(
                ExecutionError::IntegerDivideByZero { .. }
            ))
        ));
    }

    #[test]
    fn bitwise_operations_work_with_linear_memory() {
        let byte = module(
            r#"(module (memory 1)
            (data (i32.const 0) "\02")
            (func (export "set-high-bit") (param i32) (result i32)
                local.get 0 local.get 0 i32.load8_u i32.const 128 i32.or i32.store8
                local.get 0 i32.load8_u))"#,
        );
        let framed = scan_module(&byte).unwrap();
        let mut set = FunctionSet::from_framed(&framed).unwrap();
        assert_eq!(set.execute_export("set-high-bit", &[0], 100), Ok(Some(130)));
        assert_eq!(set.execute_export("set-high-bit", &[0], 100), Ok(Some(130)));
    }

    #[test]
    fn reads_and_writes_16_bit_values_with_correct_bounds_and_signs() {
        let bytes = module(
            r#"(module (memory 1 2)
            (data (i32.const 0) "\34\12\00\80\ff\ff\ff\7f")
            (func (export "signed") (param i32) (result i32) local.get 0 i32.load16_s)
            (func (export "unsigned") (param i32) (result i32) local.get 0 i32.load16_u align=1)
            (func (export "offset") (param i32) (result i32) local.get 0 i32.load16_u offset=1)
            (func (export "byte") (param i32) (result i32) local.get 0 i32.load8_u)
            (func (export "store") (param i32 i32) local.get 0 local.get 1 i32.store16 align=1)
            (func (export "grow") (param i32) (result i32) local.get 0 memory.grow)
            (func (export "overflow") (result i32) i32.const 1 i32.load16_u offset=4294967295))"#,
        );
        let framed = scan_module(&bytes).unwrap();
        let mut set = FunctionSet::from_framed(&framed).unwrap();
        for (address, signed, unsigned) in [
            (0, 4660, 4660),
            (2, -32768, 32768),
            (4, -1, 65535),
            (6, 32767, 32767),
        ] {
            assert_eq!(
                set.execute_export("signed", &[address], 100),
                Ok(Some(signed))
            );
            assert_eq!(
                set.execute_export("unsigned", &[address], 100),
                Ok(Some(unsigned))
            );
        }
        assert_eq!(set.execute_export("offset", &[1], 100), Ok(Some(32768)));
        assert_eq!(set.execute_export("store", &[1, 0x1234abcd], 100), Ok(None));
        assert_eq!(set.execute_export("unsigned", &[1], 100), Ok(Some(0xabcd)));
        assert_eq!(set.execute_export("byte", &[0], 100), Ok(Some(0x34)));
        assert_eq!(set.execute_export("byte", &[3], 100), Ok(Some(0x80)));
        assert_eq!(set.execute_export("store", &[65534, 0x1234], 100), Ok(None));
        for (name, arguments) in [
            ("unsigned", &[65535][..]),
            ("signed", &[-1][..]),
            ("offset", &[65534][..]),
            ("store", &[65535, -1][..]),
            ("overflow", &[][..]),
        ] {
            assert!(
                matches!(
                    set.execute_export(name, arguments, 100),
                    Err(ExportCallError::Execution(
                        ExecutionError::MemoryOutOfBounds { .. }
                    ))
                ),
                "{name}"
            );
        }
        assert_eq!(
            set.execute_export("unsigned", &[65534], 100),
            Ok(Some(0x1234))
        );
        assert_eq!(set.execute_export("grow", &[1], 100), Ok(Some(1)));
        assert_eq!(
            set.execute_export("unsigned", &[65535], 100),
            Ok(Some(0x12))
        );
    }

    #[test]
    fn validates_16_bit_memory_instructions() {
        let cases = [
            (
                "(module (func (result i32) i32.const 0 i32.load16_u))",
                "missing",
            ),
            (
                "(module (memory 1) (func (result i32) i32.const 0 i32.load16_s align=4))",
                "alignment",
            ),
            (
                "(module (memory 1) (func i32.const 0 i32.const 0 i32.store16 align=4))",
                "alignment",
            ),
            (
                "(module (memory 1) (func i32.const 0 i32.store16))",
                "underflow",
            ),
            (
                "(module (memory 1) (func (result i32) i32.const 0 i32.load16_u offset=4294967296))",
                "offset",
            ),
        ];
        for (wat, expected) in cases {
            let bytes = module(wat);
            let framed = scan_module(&bytes).unwrap();
            let error = match FunctionSet::from_framed(&framed) {
                Ok(_) => panic!("unexpectedly accepted {wat}"),
                Err(error) => error,
            };
            assert!(
                match expected {
                    "missing" => matches!(
                        error,
                        FunctionSetError::Function {
                            error: FunctionError::MissingMemory { .. },
                            ..
                        }
                    ),
                    "alignment" => matches!(
                        error,
                        FunctionSetError::Function {
                            error: FunctionError::InvalidMemoryAlignment { align: 2, .. },
                            ..
                        }
                    ),
                    "underflow" => matches!(
                        error,
                        FunctionSetError::Function {
                            error: FunctionError::StackUnderflow {
                                needed: 2,
                                available: 1,
                                ..
                            },
                            ..
                        }
                    ),
                    "offset" => matches!(
                        error,
                        FunctionSetError::Function {
                            error: FunctionError::InvalidMemoryOffset {
                                value: 4294967296,
                                ..
                            },
                            ..
                        }
                    ),
                    _ => false,
                },
                "{wat}: {error:?}"
            );
        }
    }

    #[test]
    fn reads_and_writes_single_bytes_with_correct_bounds_and_signs() {
        let bytes = module(
            r#"(module (memory 1)
            (data (i32.const 0) "\80\ff\7f")
            (func (export "signed") (param i32) (result i32) local.get 0 i32.load8_s)
            (func (export "unsigned") (param i32) (result i32) local.get 0 i32.load8_u)
            (func (export "offset") (param i32) (result i32) local.get 0 i32.load8_u offset=1)
            (func (export "store") (param i32 i32) local.get 0 local.get 1 i32.store8)
            (func (export "overflow") (result i32) i32.const 1 i32.load8_u offset=4294967295))"#,
        );
        let framed = scan_module(&bytes).unwrap();
        let mut set = FunctionSet::from_framed(&framed).unwrap();
        for (address, signed, unsigned) in [(0, -128, 128), (1, -1, 255), (2, 127, 127)] {
            assert_eq!(
                set.execute_export("signed", &[address], 100),
                Ok(Some(signed))
            );
            assert_eq!(
                set.execute_export("unsigned", &[address], 100),
                Ok(Some(unsigned))
            );
        }
        assert_eq!(set.execute_export("offset", &[0], 100), Ok(Some(255)));
        assert_eq!(set.execute_export("store", &[65535, -2], 100), Ok(None));
        assert_eq!(set.execute_export("signed", &[65535], 100), Ok(Some(-2)));
        assert_eq!(set.execute_export("unsigned", &[65535], 100), Ok(Some(254)));
        assert_eq!(set.execute_export("unsigned", &[65534], 100), Ok(Some(0)));
        for (name, arguments) in [
            ("unsigned", &[65536][..]),
            ("signed", &[-1][..]),
            ("offset", &[65535][..]),
            ("store", &[65536, 1][..]),
            ("overflow", &[][..]),
        ] {
            assert!(
                matches!(
                    set.execute_export(name, arguments, 100),
                    Err(ExportCallError::Execution(
                        ExecutionError::MemoryOutOfBounds { .. }
                    ))
                ),
                "{name}"
            );
        }
        assert_eq!(set.execute_export("unsigned", &[65535], 100), Ok(Some(254)));
    }

    #[test]
    fn validates_byte_memory_instructions() {
        let cases = [
            (
                "(module (func (result i32) i32.const 0 i32.load8_u))",
                "missing",
            ),
            (
                "(module (memory 1) (func (result i32) i32.const 0 i32.load8_s align=2))",
                "alignment",
            ),
            (
                "(module (memory 1) (func i32.const 0 i32.const 0 i32.store8 align=2))",
                "alignment",
            ),
            (
                "(module (memory 1) (func i32.const 0 i32.store8))",
                "underflow",
            ),
            (
                "(module (memory 1) (func (result i32) i32.const 0 i32.load8_u offset=4294967296))",
                "offset",
            ),
        ];
        for (wat, expected) in cases {
            let bytes = module(wat);
            let framed = scan_module(&bytes).unwrap();
            let error = match FunctionSet::from_framed(&framed) {
                Ok(_) => panic!("unexpectedly accepted {wat}"),
                Err(error) => error,
            };
            assert!(
                match expected {
                    "missing" => matches!(
                        error,
                        FunctionSetError::Function {
                            error: FunctionError::MissingMemory { .. },
                            ..
                        }
                    ),
                    "alignment" => matches!(
                        error,
                        FunctionSetError::Function {
                            error: FunctionError::InvalidMemoryAlignment { align: 1, .. },
                            ..
                        }
                    ),
                    "underflow" => matches!(
                        error,
                        FunctionSetError::Function {
                            error: FunctionError::StackUnderflow {
                                needed: 2,
                                available: 1,
                                ..
                            },
                            ..
                        }
                    ),
                    "offset" => matches!(
                        error,
                        FunctionSetError::Function {
                            error: FunctionError::InvalidMemoryOffset {
                                value: 4294967296,
                                ..
                            },
                            ..
                        }
                    ),
                    _ => false,
                },
                "{wat}: {error:?}"
            );
        }
    }

    #[test]
    fn grows_memory_without_losing_contents_or_exceeding_limits() {
        let bytes = module(
            r#"(module
            (memory 1 2)
            (data (i32.const 0) "\44\33\22\11")
            (func (export "size") (result i32) memory.size)
            (func (export "grow") (param i32) (result i32) local.get 0 memory.grow)
            (func (export "load") (param i32) (result i32) local.get 0 i32.load))"#,
        );
        let framed = scan_module(&bytes).unwrap();
        let mut set = FunctionSet::from_framed(&framed).unwrap();
        let mut other = FunctionSet::from_framed(&framed).unwrap();
        assert_eq!(set.execute_export("size", &[], 100), Ok(Some(1)));
        assert_eq!(set.execute_export("grow", &[0], 100), Ok(Some(1)));
        assert!(matches!(
            set.execute_export("load", &[65536], 100),
            Err(ExportCallError::Execution(
                ExecutionError::MemoryOutOfBounds { .. }
            ))
        ));
        assert_eq!(set.execute_export("grow", &[1], 100), Ok(Some(1)));
        assert_eq!(set.execute_export("size", &[], 100), Ok(Some(2)));
        assert_eq!(set.execute_export("load", &[0], 100), Ok(Some(0x11223344)));
        assert_eq!(set.execute_export("load", &[65536], 100), Ok(Some(0)));
        assert_eq!(other.execute_export("size", &[], 100), Ok(Some(1)));
        for delta in [1, -1] {
            assert_eq!(set.execute_export("grow", &[delta], 100), Ok(Some(-1)));
            assert_eq!(set.execute_export("size", &[], 100), Ok(Some(2)));
            assert_eq!(set.execute_export("load", &[0], 100), Ok(Some(0x11223344)));
        }
    }

    #[test]
    fn memory_growth_enforces_host_cap_and_validates_index() {
        let bytes = module(
            r#"(module (memory 0)
            (func (export "size") (result i32) memory.size)
            (func (export "grow") (param i32) (result i32) local.get 0 memory.grow))"#,
        );
        let framed = scan_module(&bytes).unwrap();
        let mut set = FunctionSet::from_framed(&framed).unwrap();
        assert_eq!(set.execute_export("grow", &[1025], 100), Ok(Some(-1)));
        assert_eq!(set.execute_export("size", &[], 100), Ok(Some(0)));
        assert_eq!(set.execute_export("grow", &[1], 100), Ok(Some(0)));
        assert_eq!(set.execute_export("size", &[], 100), Ok(Some(1)));

        let bytes = module(
            r#"(module (memory 0 0)
            (func (export "grow") (param i32) (result i32) local.get 0 memory.grow))"#,
        );
        let framed = scan_module(&bytes).unwrap();
        let mut zero = FunctionSet::from_framed(&framed).unwrap();
        assert_eq!(zero.execute_export("grow", &[0], 100), Ok(Some(0)));
        assert_eq!(zero.execute_export("grow", &[1], 100), Ok(Some(-1)));

        for (wat, missing) in [
            ("(module (func (result i32) memory.size))", true),
            (
                "(module (memory 1) (func (result i32) memory.size 1))",
                false,
            ),
            (
                "(module (memory 1) (func (result i32) i32.const 0 memory.grow 1))",
                false,
            ),
        ] {
            let bytes = module(wat);
            let framed = scan_module(&bytes).unwrap();
            let result = FunctionSet::from_framed(&framed);
            assert!(
                if missing {
                    matches!(
                        result,
                        Err(FunctionSetError::Function {
                            error: FunctionError::MissingMemory { .. },
                            ..
                        })
                    )
                } else {
                    matches!(
                        result,
                        Err(FunctionSetError::Function {
                            error: FunctionError::InvalidMemoryIndex { index: 1, .. },
                            ..
                        })
                    )
                },
                "{wat}"
            );
        }
        let bytes = module("(module (memory 1) (func (result i32) memory.grow))");
        let framed = scan_module(&bytes).unwrap();
        assert!(matches!(
            FunctionSet::from_framed(&framed),
            Err(FunctionSetError::Function {
                error: FunctionError::StackUnderflow {
                    needed: 1,
                    available: 0,
                    ..
                },
                ..
            })
        ));
    }

    #[test]
    fn initializes_active_data_in_order_and_keeps_instances_independent() {
        let bytes = module(
            r#"(module (memory 1)
            (data (i32.const 1) "\01\02" "\03\04")
            (data (memory 0) (i32.const 3) "\aa\bb")
            (data (i32.const 65536) "")
            (func (export "load") (param i32) (result i32) local.get 0 i32.load align=1)
            (func (export "store") (param i32 i32) local.get 0 local.get 1 i32.store align=1))"#,
        );
        let framed = scan_module(&bytes).unwrap();
        let mut first = FunctionSet::from_framed(&framed).unwrap();
        let mut second = FunctionSet::from_framed(&framed).unwrap();
        let expected = i32::from_le_bytes([1, 2, 0xaa, 0xbb]);
        assert_eq!(first.execute_export("load", &[1], 100), Ok(Some(expected)));
        assert_eq!(second.execute_export("load", &[1], 100), Ok(Some(expected)));
        assert_eq!(second.execute_export("load", &[1], 100), Ok(Some(expected)));
    }

    #[test]
    fn rejects_data_with_missing_memory_or_out_of_bounds_initialization() {
        for wat in [
            r#"(module (data (i32.const 0) "a"))"#,
            r#"(module (memory 1) (data (memory 1) (i32.const 0) "a"))"#,
        ] {
            let bytes = module(wat);
            let framed = scan_module(&bytes).unwrap();
            assert!(
                matches!(
                    FunctionSet::from_framed(&framed),
                    Err(FunctionSetError::InvalidDataMemory { .. })
                ),
                "{wat}"
            );
        }
        for wat in [
            r#"(module (memory 1) (data (i32.const 65534) "abc"))"#,
            r#"(module (memory 1) (data (i32.const 65537) ""))"#,
            r#"(module (memory 1) (data (i32.const -1) ""))"#,
        ] {
            let bytes = module(wat);
            let framed = scan_module(&bytes).unwrap();
            assert!(
                matches!(
                    FunctionSet::from_framed(&framed),
                    Err(FunctionSetError::DataOutOfBounds { .. })
                ),
                "{wat}"
            );
        }
    }

    #[test]
    fn rejects_invalid_memory_use_and_unsupported_initialization() {
        let cases = [
            ("(module (func (result i32) i32.const 0 i32.load))", true),
            (
                "(module (memory 1) (func (result i32) i32.const 0 i32.load align=8))",
                false,
            ),
        ];
        for (wat, missing) in cases {
            let bytes = module(wat);
            let framed = scan_module(&bytes).unwrap();
            let error = match FunctionSet::from_framed(&framed) {
                Ok(_) => panic!("unexpectedly accepted {wat}"),
                Err(error) => error,
            };
            assert!(
                if missing {
                    matches!(
                        error,
                        FunctionSetError::Function {
                            error: FunctionError::MissingMemory { .. },
                            ..
                        }
                    )
                } else {
                    matches!(
                        error,
                        FunctionSetError::Function {
                            error: FunctionError::InvalidMemoryAlignment { align: 3, .. },
                            ..
                        }
                    )
                },
                "{wat}: {error:?}"
            );
        }
        let bytes = module(
            "(module (memory 1) (func (result i32) i32.const 0 i32.load offset=4294967296))",
        );
        let framed = scan_module(&bytes).unwrap();
        assert!(matches!(
            FunctionSet::from_framed(&framed),
            Err(FunctionSetError::Function {
                error: FunctionError::InvalidMemoryOffset {
                    value: 4294967296,
                    ..
                },
                ..
            })
        ));
        let mut bytes = module("(module (memory 1))");
        // An empty data section has no effect.
        bytes.extend_from_slice(&[11, 1, 0]);
        let framed = scan_module(&bytes).unwrap();
        assert!(FunctionSet::from_framed(&framed).is_ok());

        let mut bytes = module(r#"(module (memory (export "mem") 1))"#);
        let framed = scan_module(&bytes).unwrap();
        let offset = super::decode_exports(&framed).unwrap()[0].index_offset;
        bytes[offset] = 1;
        let framed = scan_module(&bytes).unwrap();
        assert!(matches!(
            FunctionSet::from_framed(&framed),
            Err(FunctionSetError::InvalidMemoryExport { index: 1, .. })
        ));
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
        let mut set = FunctionSet::from_framed(&framed).unwrap();
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
        let mut set = FunctionSet::from_framed(&framed).unwrap();
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
        let mut set = FunctionSet::from_framed(&framed).unwrap();
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
