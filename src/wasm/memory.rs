//! A single unshared 32-bit linear memory.
//!
//! Decoding and allocation are separate so a malformed function cannot force
//! memory allocation. The host cap is a resource policy, not a Wasm limit.
//!
//! <https://webassembly.github.io/spec/core/binary/modules.html#memory-section>
//! <https://webassembly.github.io/spec/core/exec/instructions.html#memory-instructions>

use super::{BinaryError, BinaryErrorKind, binary::FramedModule, reader::Reader};

pub(super) const PAGE_SIZE: usize = 65_536;
const WASM32_MAX_PAGES: u32 = 65_536;
const HOST_MAX_PAGES: u32 = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MemoryType {
    pub(crate) minimum: u32,
    pub(crate) maximum: Option<u32>,
}

#[derive(Debug)]
pub(crate) enum MemoryError {
    Binary(BinaryError),
    MultipleMemoriesUnsupported,
    UnsupportedLimitsFlags(u8),
    InvalidLimits { minimum: u64, maximum: Option<u64> },
    HostLimitExceeded { minimum: u32, limit: u32 },
    AllocationFailed,
}

/// Reads one unshared memory32 definition without allocating its bytes.
pub(crate) fn decode_memory(module: &FramedModule<'_>) -> Result<Option<MemoryType>, MemoryError> {
    let Some(section) = module.sections.iter().find(|section| section.id == 5) else {
        return Ok(None);
    };
    let mut reader = Reader::new(section.payload, section.payload_offset);
    let count = reader.read_u32_leb().map_err(MemoryError::Binary)?;
    if count > 1 {
        return Err(MemoryError::MultipleMemoriesUnsupported);
    }
    let memory = if count == 0 {
        None
    } else {
        let flags = reader.read_byte().map_err(MemoryError::Binary)?;
        if flags > 1 {
            return Err(MemoryError::UnsupportedLimitsFlags(flags));
        }
        let minimum = reader.read_u64_leb().map_err(MemoryError::Binary)?;
        let maximum = if flags == 1 {
            Some(reader.read_u64_leb().map_err(MemoryError::Binary)?)
        } else {
            None
        };
        if minimum > u64::from(WASM32_MAX_PAGES)
            || maximum.is_some_and(|max| max > u64::from(WASM32_MAX_PAGES) || minimum > max)
        {
            return Err(MemoryError::InvalidLimits { minimum, maximum });
        }
        Some(MemoryType {
            minimum: minimum as u32,
            maximum: maximum.map(|max| max as u32),
        })
    };
    if !reader.is_empty() {
        return Err(MemoryError::Binary(BinaryError::new(
            reader.offset(),
            BinaryErrorKind::TrailingSectionData(5),
        )));
    }
    Ok(memory)
}

pub(crate) struct LinearMemory {
    bytes: Vec<u8>,
    maximum: Option<u32>,
}

impl LinearMemory {
    pub(crate) fn new(ty: MemoryType) -> Result<Self, MemoryError> {
        if ty.minimum > HOST_MAX_PAGES {
            return Err(MemoryError::HostLimitExceeded {
                minimum: ty.minimum,
                limit: HOST_MAX_PAGES,
            });
        }
        let len = usize::try_from(ty.minimum)
            .ok()
            .and_then(|pages| pages.checked_mul(PAGE_SIZE))
            .ok_or(MemoryError::AllocationFailed)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(len)
            .map_err(|_| MemoryError::AllocationFailed)?;
        bytes.resize(len, 0);
        Ok(Self {
            bytes,
            maximum: ty.maximum,
        })
    }

    /// Returns the current number of 64 KiB pages.
    pub(crate) fn size_pages(&self) -> u32 {
        (self.bytes.len() / PAGE_SIZE) as u32
    }

    /// Returns the previous size, or `None` without changing memory on failure.
    pub(crate) fn grow(&mut self, delta: u32) -> Option<u32> {
        let previous = self.size_pages();
        let next = previous.checked_add(delta)?;
        if next > WASM32_MAX_PAGES
            || next > HOST_MAX_PAGES
            || self.maximum.is_some_and(|maximum| next > maximum)
        {
            return None;
        }
        let len = usize::try_from(next).ok()?.checked_mul(PAGE_SIZE)?;
        self.bytes.try_reserve_exact(len - self.bytes.len()).ok()?;
        self.bytes.resize(len, 0);
        Some(previous)
    }

    /// Initializes bytes of an active segment, including zero-length segments.
    pub(crate) fn initialize(&mut self, address: u32, bytes: &[u8]) -> Option<()> {
        let start = usize::try_from(address).ok()?;
        let end = start.checked_add(bytes.len())?;

        self.bytes.get_mut(start..end)?.copy_from_slice(bytes);
        Some(())
    }

    fn range(&self, address: i32, offset: u64, width: u64) -> Option<std::ops::Range<usize>> {
        let start = u64::from(address as u32).checked_add(offset)?;
        let end = start.checked_add(width)?;
        if end > self.bytes.len() as u64 {
            return None;
        }
        Some(usize::try_from(start).ok()?..usize::try_from(end).ok()?)
    }

    pub(crate) fn load_i32(&self, address: i32, offset: u64) -> Option<i32> {
        let range = self.range(address, offset, 4)?;
        Some(i32::from_le_bytes(self.bytes[range].try_into().ok()?))
    }

    pub(crate) fn store_i32(&mut self, address: i32, offset: u64, value: i32) -> Option<()> {
        let range = self.range(address, offset, 4)?;
        self.bytes[range].copy_from_slice(&value.to_le_bytes());
        Some(())
    }

    pub(crate) fn load_u8(&self, address: i32, offset: u64) -> Option<u8> {
        let range = self.range(address, offset, 1)?;
        Some(self.bytes[range.start])
    }

    pub(crate) fn store_u8(&mut self, address: i32, offset: u64, value: i32) -> Option<()> {
        let range = self.range(address, offset, 1)?;
        self.bytes[range.start] = value as u8;
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::{LinearMemory, MemoryError, MemoryType, decode_memory};
    use crate::wasm::{
        BinaryErrorKind,
        binary::{FramedModule, Section, scan_module},
        test_support::module,
    };

    #[test]
    fn decodes_memory_limits_and_separates_host_policy() {
        let bytes = module("(module (memory 1 2))");
        assert_eq!(
            decode_memory(&scan_module(&bytes).unwrap()).unwrap(),
            Some(MemoryType {
                minimum: 1,
                maximum: Some(2)
            })
        );
        for wat in [
            "(module (memory 2 1))",
            "(module (memory 65537))",
            "(module (memory 0 65537))",
        ] {
            let bytes = module(wat);
            assert!(
                matches!(
                    decode_memory(&scan_module(&bytes).unwrap()),
                    Err(MemoryError::InvalidLimits { .. }),
                ),
                "{wat}"
            );
        }
        let bytes = module("(module (memory 1025))");
        let ty = decode_memory(&scan_module(&bytes).unwrap())
            .unwrap()
            .unwrap();
        assert!(matches!(
            LinearMemory::new(ty),
            Err(MemoryError::HostLimitExceeded {
                minimum: 1025,
                limit: 1024
            }),
        ));
        let padded = [1, 0, 0x80, 0x80, 0x80, 0x80, 0x80, 0];
        let module = FramedModule {
            sections: vec![Section {
                id: 5,
                offset: 98,
                payload_offset: 100,
                payload: &padded,
                custom: None,
            }],
        };
        assert_eq!(
            decode_memory(&module).unwrap(),
            Some(MemoryType {
                minimum: 0,
                maximum: None
            })
        );
    }

    #[test]
    fn rejects_unsupported_and_malformed_memory_sections() {
        for (payload, expected) in [
            (&[2][..], "multiple"),
            (&[1, 2, 1][..], "flags"),
            (&[1, 0x80, 0, 0][..], "bad_flags"),
            (&[1, 1, 1][..], "truncated"),
            (&[0, 0][..], "trailing"),
        ] {
            let module = FramedModule {
                sections: vec![Section {
                    id: 5,
                    offset: 98,
                    payload_offset: 100,
                    payload,
                    custom: None,
                }],
            };
            let error = decode_memory(&module).unwrap_err();
            assert!(
                match expected {
                    "multiple" => matches!(error, MemoryError::MultipleMemoriesUnsupported),
                    "flags" => matches!(error, MemoryError::UnsupportedLimitsFlags(2)),
                    "bad_flags" => matches!(error, MemoryError::UnsupportedLimitsFlags(0x80)),
                    "truncated" =>
                        matches!(error, MemoryError::Binary(ref e) if e.kind == BinaryErrorKind::UnexpectedEnd),
                    "trailing" =>
                        matches!(error, MemoryError::Binary(ref e) if e.kind == BinaryErrorKind::TrailingSectionData(5)),
                    _ => false,
                },
                "{payload:?}: {error:?}"
            );
        }
    }
}
