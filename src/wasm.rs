//! WebAssembly core binary support.
//!
//! Scans binary envelopes, decodes sections, and executes a small function subset.
//! This does not validate or instantiate complete modules.
//!
//! <https://webassembly.github.io/spec/core/binary/modules.html>

#[path = "wasm/binary.rs"]
pub(crate) mod binary;

#[path = "wasm/code.rs"]
pub(crate) mod code;

#[path = "wasm/data.rs"]
pub(crate) mod data;

#[path = "wasm/decode.rs"]
pub(crate) mod decode;

#[path = "wasm/execution.rs"]
pub(crate) mod execution;

#[path = "wasm/export.rs"]
pub(crate) mod export;

#[path = "wasm/instructions.rs"]
pub(crate) mod instructions;

#[path = "wasm/instance.rs"]
pub(crate) mod instance;

#[path = "wasm/memory.rs"]
pub(crate) mod memory;

#[path = "wasm/reader.rs"]
mod reader;

#[cfg(test)]
mod test_support;

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BinaryError {
    pub(crate) offset: usize,
    pub(crate) kind: BinaryErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BinaryErrorKind {
    UnexpectedEnd,
    InvalidMagic,
    UnsupportedVersion([u8; 4]),
    InvalidUnsignedLeb128,
    InvalidUnsignedLeb64,
    InvalidSignedLeb128,
    InvalidSectionId(u8),
    DuplicateSection(u8),
    OutOfOrderSection(u8),
    InvalidNameUtf8,
    InvalidExportKind(u8),
    UnsupportedTypeForm(u8),
    UnsupportedValueType(u8),
    TrailingSectionData(u8),
    TooManyLocals,
    UnsupportedOpcode(u8),
    UnsupportedBlockType(u8),
    UnsupportedMemoryImmediate,
    UnexpectedElse,
    MissingExpressionEnd,
    TrailingExpressionData,
    SizeOverflow,
    AllocationFailed,
}

impl BinaryError {
    fn new(offset: usize, kind: BinaryErrorKind) -> Self {
        Self { offset, kind }
    }
}

impl fmt::Display for BinaryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "WebAssembly binary error at byte {}: ",
            self.offset
        )?;
        match &self.kind {
            BinaryErrorKind::UnexpectedEnd => write!(formatter, "unexpected end of input"),
            BinaryErrorKind::InvalidMagic => write!(formatter, "invalid magic"),
            BinaryErrorKind::UnsupportedVersion(version) => {
                write!(formatter, "unsupported version {version:02x?}")
            }
            BinaryErrorKind::InvalidUnsignedLeb128 => write!(formatter, "invalid u32 LEB128"),
            BinaryErrorKind::InvalidUnsignedLeb64 => write!(formatter, "invalid u64 LEB128"),
            BinaryErrorKind::InvalidSignedLeb128 => write!(formatter, "invalid i32 LEB128"),
            BinaryErrorKind::InvalidSectionId(id) => write!(formatter, "invalid section id {id}"),
            BinaryErrorKind::DuplicateSection(id) => write!(formatter, "duplicate section {id}"),
            BinaryErrorKind::OutOfOrderSection(id) => {
                write!(formatter, "section {id} is out of order")
            }
            BinaryErrorKind::InvalidNameUtf8 => write!(formatter, "invalid UTF-8 name"),
            BinaryErrorKind::InvalidExportKind(kind) => {
                write!(formatter, "invalid export kind 0x{kind:02x}")
            }
            BinaryErrorKind::UnsupportedTypeForm(byte) => {
                write!(formatter, "unsupported type form 0x{byte:02x}")
            }
            BinaryErrorKind::UnsupportedValueType(byte) => {
                write!(formatter, "unsupported value type 0x{byte:02x}")
            }
            BinaryErrorKind::TrailingSectionData(id) => {
                write!(formatter, "trailing bytes in section {id}")
            }
            BinaryErrorKind::TooManyLocals => write!(formatter, "too many function locals"),
            BinaryErrorKind::UnsupportedOpcode(opcode) => {
                write!(formatter, "unsupported opcode 0x{opcode:02x}")
            }
            BinaryErrorKind::UnsupportedBlockType(byte) => {
                write!(
                    formatter,
                    "unsupported block type starting with 0x{byte:02x}"
                )
            }
            BinaryErrorKind::UnsupportedMemoryImmediate => {
                write!(
                    formatter,
                    "multi-memory instruction encoding is not supported"
                )
            }
            BinaryErrorKind::UnexpectedElse => write!(formatter, "unexpected else"),
            BinaryErrorKind::MissingExpressionEnd => write!(formatter, "missing expression end"),
            BinaryErrorKind::TrailingExpressionData => {
                write!(formatter, "trailing bytes after expression end")
            }
            BinaryErrorKind::SizeOverflow => {
                write!(formatter, "section size exceeds addressable memory")
            }
            BinaryErrorKind::AllocationFailed => {
                write!(formatter, "cannot allocate section metadata")
            }
        }
    }
}

impl std::error::Error for BinaryError {}
