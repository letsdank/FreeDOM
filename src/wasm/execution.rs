//! Validation and execution of a small function subset with structured control.
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
//! <https://webassembly.github.io/spec/core/appendix/algorithm.html>

use std::{collections::HashMap, fmt};

use super::{
    BinaryError,
    code::FunctionBody,
    decode::{Declarations, ValueType},
    instructions::{Expression, InstructionKind, decode_expression},
    memory::LinearMemory,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FunctionError {
    UnknownTypeIndex(u32),
    /// The signature can be valid Wasm even though it is not supported here.
    UnsupportedSignature,
    UnsupportedLocals,
    AllocationFailed,
    Decode(BinaryError),
    InvalidLocalIndex {
        index: u32,
        offset: usize,
    },
    InvalidLabelIndex {
        index: u32,
        offset: usize,
    },
    InvalidFunctionIndex {
        index: u32,
        offset: usize,
    },
    MissingMemory {
        offset: usize,
    },
    InvalidMemoryIndex {
        index: u32,
        offset: usize,
    },
    InvalidMemoryAlignment {
        align: u32,
        offset: usize,
    },
    InvalidMemoryOffset {
        value: u64,
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
    BranchStackMismatch {
        offset: usize,
        expected: usize,
        actual: usize,
    },
    MissingElseForResult {
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
            Self::AllocationFailed => write!(formatter, "cannot allocate validation state"),
            Self::Decode(error) => write!(formatter, "{error}"),
            Self::InvalidLocalIndex { index, offset } => {
                write!(formatter, "unknown local index {index} at byte {offset}")
            }
            Self::InvalidLabelIndex { index, offset } => {
                write!(formatter, "unknown label index {index} at byte {offset}")
            }
            Self::InvalidFunctionIndex { index, offset } => {
                write!(formatter, "unknown function index {index} at byte {offset}")
            }
            Self::MissingMemory { offset } => {
                write!(formatter, "memory is not defined at byte {offset}")
            }
            Self::InvalidMemoryIndex { index, offset } => {
                write!(formatter, "unknown memory index {index} at byte {offset}")
            }
            Self::InvalidMemoryAlignment { align, offset } => {
                write!(
                    formatter,
                    "invalid i32 memory alignment {align} at byte {offset}"
                )
            }
            Self::InvalidMemoryOffset { value, offset } => {
                write!(
                    formatter,
                    "i32 memory offset {value} is too large at byte {offset}"
                )
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
            Self::BranchStackMismatch {
                offset,
                expected,
                actual,
            } => write!(
                formatter,
                "branch result stack mismatch at byte {offset}: expected {expected}, found {actual}"
            ),
            Self::MissingElseForResult { offset } => {
                write!(
                    formatter,
                    "if with a result requires an else at byte {offset}"
                )
            }
        }
    }
}

impl std::error::Error for FunctionError {}

/// A decoded function checked against the subset above, not a validated module.
pub(crate) struct ValidatedFunction {
    expression: Expression,
    branches: Vec<Option<BranchTarget>>,
    returns_i32: bool,
    max_stack_depth: usize,
    parameter_count: usize,
    local_index_count: u64,
}

#[derive(Clone, Copy)]
struct BranchTarget {
    instruction: usize,
    height: usize,
    results: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ControlKind {
    Function,
    Block,
    Loop,
    If,
}

struct ControlFrame {
    kind: ControlKind,
    height: usize,
    results: usize,
    target: usize,
    has_else: bool,
    unreachable: bool,
}

fn pop_operands(
    depth: &mut usize,
    frame: &ControlFrame,
    needed: usize,
    offset: usize,
) -> Result<(), FunctionError> {
    let available = *depth - frame.height;
    if available < needed && !frame.unreachable {
        return Err(FunctionError::StackUnderflow {
            offset,
            needed,
            available,
        });
    }
    *depth -= available.min(needed);
    Ok(())
}

/// Resolves the type and checks the expression's final operand stack.
pub(crate) fn validate_function(
    declarations: &Declarations,
    type_index: u32,
    body: &FunctionBody<'_>,
) -> Result<ValidatedFunction, FunctionError> {
    validate_function_with_memory(declarations, type_index, body, false)
}

pub(crate) fn validate_function_with_memory(
    declarations: &Declarations,
    type_index: u32,
    body: &FunctionBody<'_>,
    has_memory: bool,
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
    let mut controls = Vec::new();
    controls
        .try_reserve(1)
        .map_err(|_| FunctionError::AllocationFailed)?;
    controls.push(ControlFrame {
        kind: ControlKind::Function,
        height: 0,
        results: usize::from(returns_i32),
        target: expression.instructions.len(),
        has_else: false,
        unreachable: false,
    });
    let mut branches = Vec::new();
    branches
        .try_reserve_exact(expression.instructions.len())
        .map_err(|_| FunctionError::AllocationFailed)?;
    branches.resize(expression.instructions.len(), None);
    for (pc, instruction) in expression.instructions.iter().enumerate() {
        match instruction.kind {
            InstructionKind::If {
                returns_i32,
                end_target,
                ..
            }
            | InstructionKind::Block {
                returns_i32,
                end_target,
            }
            | InstructionKind::Loop {
                returns_i32,
                end_target,
            } => {
                let kind = match instruction.kind {
                    InstructionKind::If { .. } => ControlKind::If,
                    InstructionKind::Loop { .. } => ControlKind::Loop,
                    _ => ControlKind::Block,
                };
                if kind == ControlKind::If {
                    pop_operands(&mut depth, controls.last().unwrap(), 1, instruction.offset)?;
                }
                controls
                    .try_reserve(1)
                    .map_err(|_| FunctionError::AllocationFailed)?;
                controls.push(ControlFrame {
                    kind,
                    height: depth,
                    results: usize::from(returns_i32),
                    target: if kind == ControlKind::Loop {
                        pc + 1
                    } else {
                        end_target
                    },
                    has_else: false,
                    unreachable: false,
                });
            }
            InstructionKind::Else { .. } | InstructionKind::End => {
                let frame = controls
                    .last_mut()
                    .expect("decoder checked control structure");
                let actual = depth - frame.height;
                if actual > frame.results || (actual < frame.results && !frame.unreachable) {
                    return Err(FunctionError::BranchStackMismatch {
                        offset: instruction.offset,
                        expected: frame.results,
                        actual,
                    });
                }
                if matches!(instruction.kind, InstructionKind::Else { .. }) {
                    frame.has_else = true;
                    frame.unreachable = false;
                    depth = frame.height;
                } else {
                    if frame.kind == ControlKind::If && frame.results != 0 && !frame.has_else {
                        return Err(FunctionError::MissingElseForResult {
                            offset: instruction.offset,
                        });
                    }
                    depth = frame.height + frame.results;
                    controls.pop();
                }
            }
            InstructionKind::Br(index) | InstructionKind::BrIf(index) => {
                let position = usize::try_from(index)
                    .ok()
                    .and_then(|index| controls.len().checked_sub(index.checked_add(1)?))
                    .ok_or(FunctionError::InvalidLabelIndex {
                        index,
                        offset: instruction.offset,
                    })?;
                let target = &controls[position];
                let branch = BranchTarget {
                    instruction: target.target,
                    height: target.height,
                    results: if target.kind == ControlKind::Loop {
                        0
                    } else {
                        target.results
                    },
                };
                let frame = controls.last().unwrap();
                if matches!(instruction.kind, InstructionKind::BrIf(_)) {
                    pop_operands(&mut depth, frame, 1, instruction.offset)?;
                }
                pop_operands(&mut depth, frame, branch.results, instruction.offset)?;
                branches[pc] = Some(branch);
                if matches!(instruction.kind, InstructionKind::BrIf(_)) {
                    depth += branch.results;
                } else {
                    let frame = controls.last_mut().unwrap();
                    depth = frame.height;
                    frame.unreachable = true;
                }
            }
            InstructionKind::Call(index) => {
                let type_index = usize::try_from(index)
                    .ok()
                    .and_then(|i| declarations.function_types.get(i))
                    .copied()
                    .ok_or(FunctionError::InvalidFunctionIndex {
                        index,
                        offset: instruction.offset,
                    })?;
                let signature = usize::try_from(type_index)
                    .ok()
                    .and_then(|i| declarations.types.get(i))
                    .ok_or(FunctionError::UnknownTypeIndex(type_index))?;
                if signature.parameters.iter().any(|ty| *ty != ValueType::I32)
                    || !matches!(signature.results.as_slice(), [] | [ValueType::I32])
                {
                    return Err(FunctionError::UnsupportedSignature);
                }
                pop_operands(
                    &mut depth,
                    controls.last().unwrap(),
                    signature.parameters.len(),
                    instruction.offset,
                )?;
                depth += signature.results.len();
            }
            InstructionKind::I32Load { align, offset }
            | InstructionKind::I32Store { align, offset }
            | InstructionKind::I32Load8S { align, offset }
            | InstructionKind::I32Load8U { align, offset }
            | InstructionKind::I32Store8 { align, offset }
            | InstructionKind::I32Load16S { align, offset }
            | InstructionKind::I32Load16U { align, offset }
            | InstructionKind::I32Store16 { align, offset } => {
                if !has_memory {
                    return Err(FunctionError::MissingMemory {
                        offset: instruction.offset,
                    });
                }
                let max_align = match instruction.kind {
                    InstructionKind::I32Load8S { .. }
                    | InstructionKind::I32Load8U { .. }
                    | InstructionKind::I32Store8 { .. } => 0,
                    InstructionKind::I32Load16S { .. }
                    | InstructionKind::I32Load16U { .. }
                    | InstructionKind::I32Store16 { .. } => 1,
                    _ => 2,
                };
                if align > max_align {
                    return Err(FunctionError::InvalidMemoryAlignment {
                        align,
                        offset: instruction.offset,
                    });
                }
                if offset > u64::from(u32::MAX) {
                    return Err(FunctionError::InvalidMemoryOffset {
                        value: offset,
                        offset: instruction.offset,
                    });
                }
                let is_load = matches!(
                    instruction.kind,
                    InstructionKind::I32Load { .. }
                        | InstructionKind::I32Load8S { .. }
                        | InstructionKind::I32Load8U { .. }
                        | InstructionKind::I32Load16S { .. }
                        | InstructionKind::I32Load16U { .. }
                );
                pop_operands(
                    &mut depth,
                    controls.last().unwrap(),
                    if is_load { 1 } else { 2 },
                    instruction.offset,
                )?;
                if is_load {
                    depth += 1;
                }
            }
            InstructionKind::MemorySize(index) | InstructionKind::MemoryGrow(index) => {
                if !has_memory {
                    return Err(FunctionError::MissingMemory {
                        offset: instruction.offset,
                    });
                }
                if index != 0 {
                    return Err(FunctionError::InvalidMemoryIndex {
                        index,
                        offset: instruction.offset,
                    });
                }
                if matches!(instruction.kind, InstructionKind::MemoryGrow(_)) {
                    pop_operands(&mut depth, controls.last().unwrap(), 1, instruction.offset)?;
                }
                depth += 1;
            }
            InstructionKind::LocalGet(index) => {
                validate_local_index(index, local_index_count, instruction.offset)?;
                depth += 1;
            }
            InstructionKind::LocalSet(index) | InstructionKind::LocalTee(index) => {
                validate_local_index(index, local_index_count, instruction.offset)?;
                pop_operands(&mut depth, controls.last().unwrap(), 1, instruction.offset)?;
                if matches!(instruction.kind, InstructionKind::LocalTee(_)) {
                    depth += 1;
                }
            }
            InstructionKind::I32Const(_) => {
                depth += 1;
            }
            InstructionKind::I32Eqz
            | InstructionKind::I32Clz
            | InstructionKind::I32Ctz
            | InstructionKind::I32Popcnt => {
                pop_operands(&mut depth, controls.last().unwrap(), 1, instruction.offset)?;
                depth += 1;
            }
            InstructionKind::I32Add
            | InstructionKind::I32Sub
            | InstructionKind::I32Mul
            | InstructionKind::I32DivS
            | InstructionKind::I32DivU
            | InstructionKind::I32RemS
            | InstructionKind::I32RemU
            | InstructionKind::I32And
            | InstructionKind::I32Or
            | InstructionKind::I32Xor
            | InstructionKind::I32Shl
            | InstructionKind::I32ShrS
            | InstructionKind::I32ShrU
            | InstructionKind::I32Rotl
            | InstructionKind::I32Rotr
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
                pop_operands(&mut depth, controls.last().unwrap(), 2, instruction.offset)?;
                depth += 1;
            }
        }
        max_stack_depth = max_stack_depth.max(depth);
    }
    let expected = usize::from(returns_i32);
    let actual = depth;
    if actual > expected || (actual < expected && !controls[0].unreachable) {
        return Err(FunctionError::ResultStackMismatch {
            expected,
            actual,
            offset: expression.end_offset,
        });
    }
    Ok(ValidatedFunction {
        expression,
        branches,
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
    /// Host execution budget, not a WebAssembly trap.
    FuelExhausted,
    CallDepthExceeded,
    CallRequiresFunctionSet,
    InvalidFunctionIndex(u32),
    MemoryOutOfBounds {
        offset: usize,
    },
    InvalidValidatedMemory,
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
            Self::FuelExhausted => write!(formatter, "execution step limit exceeded"),
            Self::CallDepthExceeded => write!(formatter, "host call depth limit exceeded"),
            Self::CallRequiresFunctionSet => {
                write!(formatter, "call requires a checked function set")
            }
            Self::InvalidFunctionIndex(index) => {
                write!(formatter, "unknown function index {index}")
            }
            Self::MemoryOutOfBounds { offset } => {
                write!(formatter, "out of bounds memory access at byte {offset}")
            }
            Self::InvalidValidatedMemory => write!(formatter, "validated memory is not available"),
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

/// Executes with a default host budget. `None` denotes a void result.
pub(crate) fn execute_function(
    function: &ValidatedFunction,
    arguments: &[i32],
) -> Result<Option<i32>, ExecutionError> {
    execute_function_with_fuel(function, arguments, 1_000_000)
}

/// Executes at most `fuel` decoded instructions; exhaustion is a host limit.
pub(crate) fn execute_function_with_fuel(
    function: &ValidatedFunction,
    arguments: &[i32],
    mut fuel: u64,
) -> Result<Option<i32>, ExecutionError> {
    let mut frame = ExecutionFrame::new(0, function, arguments)?;
    match resume_frame(&mut frame, function, &mut fuel, &mut None)? {
        FrameEvent::Done(result) => Ok(result),
        FrameEvent::Call(_) => Err(ExecutionError::CallRequiresFunctionSet),
    }
}

struct ExecutionFrame {
    function_index: usize,
    arguments: Vec<i32>,
    stack: Vec<i32>,
    modified_locals: HashMap<u32, i32>,
    pc: usize,
}

impl ExecutionFrame {
    fn new(
        function_index: usize,
        function: &ValidatedFunction,
        arguments: &[i32],
    ) -> Result<Self, ExecutionError> {
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
        let mut copied_arguments = Vec::new();
        copied_arguments
            .try_reserve_exact(arguments.len())
            .map_err(|_| ExecutionError::AllocationFailed)?;
        copied_arguments.extend_from_slice(arguments);
        // Store only modified locals, including modified parameters. Unmodified
        // locals are zero and need no allocation even when a group is very large.
        Ok(Self {
            function_index,
            arguments: copied_arguments,
            stack,
            modified_locals: HashMap::new(),
            pc: 0,
        })
    }
}

enum FrameEvent {
    Call(u32),
    Done(Option<i32>),
}

pub(super) fn execute_function_set(
    functions: &[ValidatedFunction],
    memory: &mut Option<LinearMemory>,
    index: u32,
    arguments: &[i32],
    mut fuel: u64,
) -> Result<Option<i32>, ExecutionError> {
    const MAX_CALL_DEPTH: usize = 1024;
    let index = usize::try_from(index)
        .ok()
        .filter(|i| *i < functions.len())
        .ok_or(ExecutionError::InvalidFunctionIndex(index))?;
    let mut frames = Vec::new();
    frames
        .try_reserve(1)
        .map_err(|_| ExecutionError::AllocationFailed)?;
    frames.push(ExecutionFrame::new(index, &functions[index], arguments)?);
    loop {
        let frame = frames.last_mut().expect("root frame exists");
        let function = &functions[frame.function_index];
        match resume_frame(frame, function, &mut fuel, memory)? {
            FrameEvent::Done(result) => {
                frames.pop();
                match frames.last_mut() {
                    Some(caller) => {
                        if let Some(value) = result {
                            caller.stack.push(value);
                        }
                    }
                    None => return Ok(result),
                }
            }
            FrameEvent::Call(index) => {
                if frames.len() >= MAX_CALL_DEPTH {
                    return Err(ExecutionError::CallDepthExceeded);
                }
                let callee_index = usize::try_from(index)
                    .ok()
                    .filter(|i| *i < functions.len())
                    .ok_or(ExecutionError::InvalidFunctionIndex(index))?;
                let callee = &functions[callee_index];
                frames
                    .try_reserve(1)
                    .map_err(|_| ExecutionError::AllocationFailed)?;
                let caller = frames.last_mut().expect("caller exists");
                let start = caller
                    .stack
                    .len()
                    .checked_sub(callee.parameter_count)
                    .ok_or(ExecutionError::InvalidValidatedStack)?;
                let next = ExecutionFrame::new(callee_index, callee, &caller.stack[start..])?;
                caller.stack.truncate(start);
                frames.push(next);
            }
        }
    }
}

fn resume_frame(
    frame: &mut ExecutionFrame,
    function: &ValidatedFunction,
    fuel: &mut u64,
    memory: &mut Option<LinearMemory>,
) -> Result<FrameEvent, ExecutionError> {
    let ExecutionFrame {
        arguments,
        stack,
        modified_locals,
        pc,
        ..
    } = frame;
    let mut position = *pc;
    while let Some(instruction) = function.expression.instructions.get(position) {
        if *fuel == 0 {
            return Err(ExecutionError::FuelExhausted);
        }
        *fuel -= 1;
        match instruction.kind {
            InstructionKind::I32Load16S { offset, .. }
            | InstructionKind::I32Load16U { offset, .. } => {
                let address = stack.pop().ok_or(ExecutionError::InvalidValidatedStack)?;
                let value = memory
                    .as_ref()
                    .ok_or(ExecutionError::InvalidValidatedMemory)?
                    .load_u16(address, offset)
                    .ok_or(ExecutionError::MemoryOutOfBounds {
                        offset: instruction.offset,
                    })?;
                stack.push(
                    if matches!(instruction.kind, InstructionKind::I32Load16S { .. }) {
                        i32::from(value as i16)
                    } else {
                        i32::from(value)
                    },
                );
            }
            InstructionKind::I32Store16 { offset, .. } => {
                let value = stack.pop().ok_or(ExecutionError::InvalidValidatedStack)?;
                let address = stack.pop().ok_or(ExecutionError::InvalidValidatedStack)?;
                memory
                    .as_mut()
                    .ok_or(ExecutionError::InvalidValidatedMemory)?
                    .store_u16(address, offset, value)
                    .ok_or(ExecutionError::MemoryOutOfBounds {
                        offset: instruction.offset,
                    })?;
            }
            InstructionKind::I32Load8S { offset, .. }
            | InstructionKind::I32Load8U { offset, .. } => {
                let address = stack.pop().ok_or(ExecutionError::InvalidValidatedStack)?;
                let value = memory
                    .as_ref()
                    .ok_or(ExecutionError::InvalidValidatedMemory)?
                    .load_u8(address, offset)
                    .ok_or(ExecutionError::MemoryOutOfBounds {
                        offset: instruction.offset,
                    })?;
                stack.push(
                    if matches!(instruction.kind, InstructionKind::I32Load8S { .. }) {
                        i32::from(value as i8)
                    } else {
                        i32::from(value)
                    },
                );
            }
            InstructionKind::I32Store8 { offset, .. } => {
                let value = stack.pop().ok_or(ExecutionError::InvalidValidatedStack)?;
                let address = stack.pop().ok_or(ExecutionError::InvalidValidatedStack)?;
                memory
                    .as_mut()
                    .ok_or(ExecutionError::InvalidValidatedMemory)?
                    .store_u8(address, offset, value)
                    .ok_or(ExecutionError::MemoryOutOfBounds {
                        offset: instruction.offset,
                    })?;
            }
            InstructionKind::MemorySize(_) => {
                let pages = memory
                    .as_ref()
                    .ok_or(ExecutionError::InvalidValidatedMemory)?
                    .size_pages();
                stack.push(pages as i32);
            }
            InstructionKind::MemoryGrow(_) => {
                let delta = stack.pop().ok_or(ExecutionError::InvalidValidatedStack)? as u32;
                let previous = memory
                    .as_mut()
                    .ok_or(ExecutionError::InvalidValidatedMemory)?
                    .grow(delta);
                stack.push(previous.map_or(-1, |pages| pages as i32));
            }
            InstructionKind::I32Load { offset, .. } => {
                let address = stack.pop().ok_or(ExecutionError::InvalidValidatedStack)?;
                let value = memory
                    .as_ref()
                    .ok_or(ExecutionError::InvalidValidatedMemory)?
                    .load_i32(address, offset)
                    .ok_or(ExecutionError::MemoryOutOfBounds {
                        offset: instruction.offset,
                    })?;
                stack.push(value);
            }
            InstructionKind::I32Store { offset, .. } => {
                let value = stack.pop().ok_or(ExecutionError::InvalidValidatedStack)?;
                let address = stack.pop().ok_or(ExecutionError::InvalidValidatedStack)?;
                memory
                    .as_mut()
                    .ok_or(ExecutionError::InvalidValidatedMemory)?
                    .store_i32(address, offset, value)
                    .ok_or(ExecutionError::MemoryOutOfBounds {
                        offset: instruction.offset,
                    })?;
            }
            InstructionKind::Call(index) => {
                *pc = position + 1;
                return Ok(FrameEvent::Call(index));
            }
            InstructionKind::Block { .. } | InstructionKind::Loop { .. } => {}
            InstructionKind::Br(_) | InstructionKind::BrIf(_) => {
                let taken = if matches!(instruction.kind, InstructionKind::BrIf(_)) {
                    stack.pop().ok_or(ExecutionError::InvalidValidatedStack)? != 0
                } else {
                    true
                };
                if taken {
                    let target = function
                        .branches
                        .get(position)
                        .copied()
                        .flatten()
                        .ok_or(ExecutionError::InvalidValidatedStack)?;
                    if target.instruction > function.expression.instructions.len()
                        || stack.len() < target.height + target.results
                    {
                        return Err(ExecutionError::InvalidValidatedStack);
                    }
                    let result = if target.results == 1 {
                        Some(stack.pop().ok_or(ExecutionError::InvalidValidatedStack)?)
                    } else {
                        None
                    };
                    stack.truncate(target.height);
                    if let Some(result) = result {
                        stack.push(result);
                    }
                    position = target.instruction;
                    continue;
                }
            }
            InstructionKind::If { false_target, .. } => {
                let condition = stack.pop().ok_or(ExecutionError::InvalidValidatedStack)?;
                if condition == 0 {
                    if false_target > function.expression.instructions.len() {
                        return Err(ExecutionError::InvalidValidatedStack);
                    }
                    position = false_target;
                    continue;
                }
            }
            InstructionKind::Else { end_target } => {
                if end_target > function.expression.instructions.len() {
                    return Err(ExecutionError::InvalidValidatedStack);
                }
                position = end_target;
                continue;
            }
            InstructionKind::End => {}
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
            InstructionKind::I32Eqz
            | InstructionKind::I32Clz
            | InstructionKind::I32Ctz
            | InstructionKind::I32Popcnt => {
                let value = stack
                    .last_mut()
                    .ok_or(ExecutionError::InvalidValidatedStack)?;
                *value = match instruction.kind {
                    InstructionKind::I32Eqz => i32::from(*value == 0),
                    InstructionKind::I32Clz => value.leading_zeros() as i32,
                    InstructionKind::I32Ctz => value.trailing_zeros() as i32,
                    InstructionKind::I32Popcnt => value.count_ones() as i32,
                    _ => return Err(ExecutionError::InvalidValidatedStack),
                };
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
            | InstructionKind::I32RemU
            | InstructionKind::I32And
            | InstructionKind::I32Or
            | InstructionKind::I32Xor
            | InstructionKind::I32Shl
            | InstructionKind::I32ShrS
            | InstructionKind::I32ShrU
            | InstructionKind::I32Rotl
            | InstructionKind::I32Rotr => {
                let right = stack.pop().ok_or(ExecutionError::InvalidValidatedStack)?;
                let left = stack.pop().ok_or(ExecutionError::InvalidValidatedStack)?;
                let offset = instruction.offset;
                let value = match instruction.kind {
                    InstructionKind::I32Add => left.wrapping_add(right),
                    InstructionKind::I32Sub => left.wrapping_sub(right),
                    InstructionKind::I32Mul => left.wrapping_mul(right),
                    InstructionKind::I32And => left & right,
                    InstructionKind::I32Or => left | right,
                    InstructionKind::I32Xor => left ^ right,
                    // Wasm uses the low five bits of the shift operand.
                    InstructionKind::I32Shl => left.wrapping_shl(right as u32),
                    InstructionKind::I32ShrS => left.wrapping_shr(right as u32),
                    InstructionKind::I32ShrU => (left as u32).wrapping_shr(right as u32) as i32,
                    InstructionKind::I32Rotl => left.rotate_left(right as u32),
                    InstructionKind::I32Rotr => left.rotate_right(right as u32),
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
        position += 1;
    }
    match (function.returns_i32, stack.as_slice()) {
        (false, []) => Ok(FrameEvent::Done(None)),
        (true, [value]) => Ok(FrameEvent::Done(Some(*value))),
        _ => Err(ExecutionError::InvalidValidatedStack),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ExecutionError, FunctionError, execute_function, execute_function_with_fuel,
        validate_function,
    };
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
        let unsupported = body(&[0x79, 0x0b]);
        assert!(matches!(
            validate_function(&declarations, 0, &unsupported),
            Err(FunctionError::Decode(error)) if error.kind == BinaryErrorKind::UnsupportedOpcode(0x79)
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
    fn executes_nested_if_branches_and_skips_untaken_traps() {
        let wat = "(module (func (param i32 i32) (result i32)
            local.get 0
            if (result i32)
                local.get 1
                if (result i32) i32.const 11 else i32.const 22 end
                else
                    i32.const 44 i32.const 2 i32.div_s
                end))";
        let bytes = module(wat);
        let framed = scan_module(&bytes).unwrap();
        let declarations = decode_declarations(&framed).unwrap();
        let bodies = decode_bodies(&framed).unwrap();
        let function = validate_function(&declarations, 0, &bodies[0]).unwrap();
        assert_eq!(execute_function(&function, &[0, 1]), Ok(Some(22)));
        assert_eq!(execute_function(&function, &[1, 0]), Ok(Some(22)));
        assert_eq!(execute_function(&function, &[-1, 1]), Ok(Some(11)));

        let bytes = module(
            "(module (func (param i32) (result i32)
            local.get 0 if (result i32) i32.const 42
            else i32.const 1 i32.const 0 i32.div_s end))",
        );
        let framed = scan_module(&bytes).unwrap();
        let declarations = decode_declarations(&framed).unwrap();
        let bodies = decode_bodies(&framed).unwrap();
        let function = validate_function(&declarations, 0, &bodies[0]).unwrap();
        assert_eq!(execute_function(&function, &[1]), Ok(Some(42)));
        assert!(matches!(
            execute_function(&function, &[0]),
            Err(ExecutionError::IntegerDivideByZero { .. })
        ));

        let bytes = module(
            "(module (func (param i32) (result i32) (local i32)
            local.get 0 if i32.const 42 local.set 1 end local.get 1))",
        );
        let framed = scan_module(&bytes).unwrap();
        let declarations = decode_declarations(&framed).unwrap();
        let bodies = decode_bodies(&framed).unwrap();
        let function = validate_function(&declarations, 0, &bodies[0]).unwrap();
        assert_eq!(execute_function(&function, &[0]), Ok(Some(0)));
        assert_eq!(execute_function(&function, &[1]), Ok(Some(42)));

        let bytes = module(
            "(module (func (param i32) (result i32)
            i32.const 10 local.get 0
            if (result i32) i32.const 2 else i32.const 3 end
            i32.add))",
        );
        let framed = scan_module(&bytes).unwrap();
        let declarations = decode_declarations(&framed).unwrap();
        let bodies = decode_bodies(&framed).unwrap();
        let function = validate_function(&declarations, 0, &bodies[0]).unwrap();
        assert_eq!(execute_function(&function, &[0]), Ok(Some(13)));
        assert_eq!(execute_function(&function, &[1]), Ok(Some(12)));
    }

    #[test]
    fn validates_if_branches_and_protects_the_outer_stack() {
        let declarations = Declarations {
            types: vec![FunctionType {
                parameters: vec![],
                results: vec![ValueType::I32],
            }],
            function_types: vec![],
        };
        let cases = [
            ("if end", "underflow"),
            ("i32.const 99 i32.const 1 if i32.eqz end", "underflow"),
            (
                "i32.const 1 if (result i32) i32.const 1 end",
                "missing_else",
            ),
            ("i32.const 1 if (result i32) i32.const 1 else end", "branch"),
            ("i32.const 1 if (result i32) else i32.const 1 end", "branch"),
            ("i32.const 1 if i32.const 1 end", "branch"),
        ];
        for (source, expected) in cases {
            let instructions = expression(source);
            let body = FunctionBody {
                locals: vec![],
                instructions_offset: 0,
                instructions: &instructions,
            };
            let error = match validate_function(&declarations, 0, &body) {
                Ok(_) => panic!("unexpectedly accepted {source}"),
                Err(error) => error,
            };
            assert!(
                match expected {
                    "underflow" =>
                        matches!(error, FunctionError::StackUnderflow { available: 0, .. }),
                    "missing_else" => matches!(error, FunctionError::MissingElseForResult { .. }),
                    "branch" => matches!(error, FunctionError::BranchStackMismatch { .. }),
                    _ => false,
                },
                "{source}: {error:?}"
            );
        }

        // Deeply nested valid input must not use the native call stack.
        let source = format!(
            "{}{}i32.const 42",
            "i32.const 1 if ".repeat(4096),
            "end ".repeat(4096)
        );
        let instructions = expression(&source);
        let body = FunctionBody {
            locals: vec![],
            instructions_offset: 0,
            instructions: &instructions,
        };
        let function = validate_function(&declarations, 0, &body).unwrap();
        assert_eq!(execute_function(&function, &[]), Ok(Some(42)));
    }

    #[test]
    fn executes_blocks_loops_and_value_carrying_branches() {
        let examples: &[(&str, &[i32], i32)] = &[
            (
                "(module (func (param i32) (result i32) (local i32)
                loop local.get 1 local.get 0 i32.add local.set 1
                    local.get 0 i32.const 1 i32.sub local.tee 0 br_if 0
                end local.get 1))",
                &[5],
                15,
            ),
            (
                "(module (func (result i32) i32.const 10 block (result i32)
                i32.const 99 i32.const 32 br 0 i32.add end i32.add))",
                &[],
                42,
            ),
            (
                "(module (func (param i32) (result i32) block (result i32)
                i32.const 37 local.get 0 br_if 0 i32.const 5 i32.add end))",
                &[0],
                42,
            ),
            (
                "(module (func (param i32) (result i32) block (result i32)
                i32.const 37 local.get 0 br_if 0 i32.const 5 i32.add end))",
                &[1],
                37,
            ),
            (
                "(module (func (result i32) block (result i32) block (result i32)
                i32.const 42 br 1 end i32.const 3 i32.add end))",
                &[],
                42,
            ),
            ("(module (func (result i32) i32.const 42 br 0))", &[], 42),
            (
                "(module (func (result i32) block (result i32)
                i32.const 1 if i32.const 42 br 1 end i32.const 0 end))",
                &[],
                42,
            ),
            (
                "(module (func (result i32) loop (result i32) i32.const 42 end))",
                &[],
                42,
            ),
        ];
        for (wat, args, expected) in examples {
            let bytes = module(wat);
            let framed = scan_module(&bytes).unwrap();
            let declarations = decode_declarations(&framed).unwrap();
            let bodies = decode_bodies(&framed).unwrap();
            let function = validate_function(&declarations, 0, &bodies[0]).unwrap();
            assert_eq!(
                execute_function(&function, args),
                Ok(Some(*expected)),
                "{wat}"
            );
        }
    }

    #[test]
    fn validates_branches_and_limits_infinite_loops() {
        let declarations = Declarations {
            types: vec![FunctionType {
                parameters: vec![],
                results: vec![],
            }],
            function_types: vec![],
        };
        let instructions = expression("br 1");
        let body = FunctionBody {
            locals: vec![],
            instructions_offset: 20,
            instructions: &instructions,
        };
        assert!(matches!(
            validate_function(&declarations, 0, &body),
            Err(FunctionError::InvalidLabelIndex {
                index: 1,
                offset: 20,
            })
        ));
        let instructions = expression("block (result i32) i32.const 1 br_if 0 i32.const 42 end");
        let body = FunctionBody {
            locals: vec![],
            instructions_offset: 20,
            instructions: &instructions,
        };
        assert!(matches!(
            validate_function(&declarations, 0, &body),
            Err(FunctionError::StackUnderflow {
                needed: 1,
                available: 0,
                ..
            })
        ));
        let instructions = expression("block br 0 i32.const 1 end");
        let body = FunctionBody {
            locals: vec![],
            instructions_offset: 20,
            instructions: &instructions,
        };
        assert!(matches!(
            validate_function(&declarations, 0, &body),
            Err(FunctionError::BranchStackMismatch {
                expected: 0,
                actual: 1,
                ..
            })
        ));
        let instructions = expression("loop br 0 end");
        let body = FunctionBody {
            locals: vec![],
            instructions_offset: 20,
            instructions: &instructions,
        };
        let function = validate_function(&declarations, 0, &body).unwrap();
        assert_eq!(
            execute_function_with_fuel(&function, &[], 20),
            Err(ExecutionError::FuelExhausted)
        );
        assert_eq!(
            execute_function_with_fuel(&function, &[], 0),
            Err(ExecutionError::FuelExhausted)
        );
    }

    #[test]
    fn executes_i32_bitwise_shifts_and_rotations() {
        let cases = [
            ("i32.and", 0x71, 0x12345678, 0x0f0f0f0f, 0x02040608),
            ("i32.or", 0x72, i32::MIN, 1, i32::MIN + 1),
            ("i32.xor", 0x73, 5, 3, 6),
            ("i32.shl", 0x74, 1, 31, i32::MIN),
            ("i32.shl", 0x74, 1, 32, 1),
            ("i32.shl", 0x74, 1, -1, i32::MIN),
            ("i32.shr_s", 0x75, i32::MIN, 1, -1073741824),
            ("i32.shr_s", 0x75, -1, 32, -1),
            ("i32.shr_u", 0x76, -1, 1, i32::MAX),
            ("i32.shr_u", 0x76, i32::MIN, -1, 1),
            ("i32.rotl", 0x77, 1, -1, i32::MIN),
            ("i32.rotr", 0x78, 1, 33, i32::MIN),
            ("i32.rotr", 0x78, i32::MIN, -1, 1),
        ];
        for (op, opcode, left, right, expected) in cases {
            assert_eq!(expression(op), [opcode, 0x0b]);
            let bytes = module(&format!(
                "(module (func (param i32 i32) (result i32) local.get 0 local.get 1 {op}))"
            ));
            let framed = scan_module(&bytes).unwrap();
            let declarations = decode_declarations(&framed).unwrap();
            let bodies = decode_bodies(&framed).unwrap();
            let function = validate_function(&declarations, 0, &bodies[0]).unwrap();
            assert_eq!(
                execute_function(&function, &[left, right]),
                Ok(Some(expected)),
                "{op} {left} {right}"
            );
            let instructions = expression(&format!("local.get 0 {op}"));
            let body = FunctionBody {
                locals: vec![],
                instructions_offset: 20,
                instructions: &instructions,
            };
            assert!(
                matches!(
                    validate_function(&declarations, 0, &body),
                    Err(FunctionError::StackUnderflow {
                        offset: 22,
                        needed: 2,
                        available: 1,
                    })
                ),
                "{op}"
            );
        }
    }

    #[test]
    fn executes_i32_bit_counts_including_zero() {
        let cases = [
            ("i32.clz", 0x67, 0, 32),
            ("i32.clz", 0x67, 1, 31),
            ("i32.clz", 0x67, -1, 0),
            ("i32.ctz", 0x68, 0, 32),
            ("i32.ctz", 0x68, i32::MIN, 31),
            ("i32.ctz", 0x68, 12, 2),
            ("i32.popcnt", 0x69, 0, 0),
            ("i32.popcnt", 0x69, -1, 32),
            ("i32.popcnt", 0x69, 0x12345678, 13),
        ];
        for (op, opcode, input, expected) in cases {
            assert_eq!(expression(op), [opcode, 0x0b]);
            let bytes = module(&format!(
                "(module (func (param i32) (result i32) local.get 0 {op}))"
            ));
            let framed = scan_module(&bytes).unwrap();
            let declarations = decode_declarations(&framed).unwrap();
            let bodies = decode_bodies(&framed).unwrap();
            let function = validate_function(&declarations, 0, &bodies[0]).unwrap();
            assert_eq!(
                execute_function(&function, &[input]),
                Ok(Some(expected)),
                "{op} {input}"
            );
            let instructions = expression(op);
            let body = FunctionBody {
                locals: vec![],
                instructions_offset: 20,
                instructions: &instructions,
            };
            assert!(
                matches!(
                    validate_function(&declarations, 0, &body),
                    Err(FunctionError::StackUnderflow {
                        offset: 20,
                        needed: 1,
                        available: 0
                    })
                ),
                "{op}"
            );
        }
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
