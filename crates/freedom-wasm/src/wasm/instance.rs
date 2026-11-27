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
#[path = "instance/tests.rs"]
mod tests;
