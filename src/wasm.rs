//! WebAssembly core binary support.
//!
//! Scans binary envelopes and decodes a subset of type and function sections.
//! It does not yet validate complete modules, instantiate, or execute them.
//!
//! <https://webassembly.github.io/spec/core/binary/modules.html>

#[path = "wasm/binary.rs"]
pub(crate) mod binary;

#[path = "wasm/decode.rs"]
pub(crate) mod decode;

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
    UnsupportedTypeForm(u8),
    UnsupportedValueType(u8),
    TrailingSectionData(u8),
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
            BinaryErrorKind::UnsupportedTypeForm(byte) => {
                write!(formatter, "unsupported type form 0x{byte:02x}")
            }
            BinaryErrorKind::UnsupportedValueType(byte) => {
                write!(formatter, "unsupported value type 0x{byte:02x}")
            }
            BinaryErrorKind::TrailingSectionData(id) => {
                write!(formatter, "trailing bytes in section {id}")
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
