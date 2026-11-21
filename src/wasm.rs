//! WebAssembly core binary support.
//!
//! The initial implementation scans the binary envelope only. It does not
//! validate section contents, instantiate modules, or execute instructions.
//!
//! <https://webassembly.github.io/spec/core/binary/modules.html>

#[path = "wasm/binary.rs"]
pub(crate) mod binary;

#[path = "wasm/reader.rs"]
mod reader;

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
    InvalidSectionId(u8),
    DuplicateSection(u8),
    OutOfOrderSection(u8),
    InvalidNameUtf8,
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
            BinaryErrorKind::InvalidSectionId(id) => write!(formatter, "invalid section id {id}"),
            BinaryErrorKind::DuplicateSection(id) => write!(formatter, "duplicate section {id}"),
            BinaryErrorKind::OutOfOrderSection(id) => {
                write!(formatter, "section {id} is out of order")
            }
            BinaryErrorKind::InvalidNameUtf8 => write!(formatter, "invalid UTF-8 section name"),
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
