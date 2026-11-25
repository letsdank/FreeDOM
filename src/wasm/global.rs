//! Defined i32 globals initialized by an i32 constant expression.
//!
//! Other value types and constant expressions are explicitly unsupported;
//! imports are handled separately by the function-set constructor.
//!
//! <https://webassembly.github.io/spec/core/binary/modules.html#global-section>
//! <https://webassembly.github.io/spec/core/valid/instructions.html#variable-instructions>

use super::{BinaryError, BinaryErrorKind, binary::FramedModule, reader::Reader};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Global {
    pub(crate) mutable: bool,
    pub(crate) value: i32,
}

#[derive(Debug)]
pub(crate) enum GlobalError {
    Binary(BinaryError),
    UnsupportedType { byte: u8, offset: usize },
    InvalidMutability { byte: u8, offset: usize },
    UnsupportedInitializer { opcode: u8, offset: usize },
    InvalidInitializerEnd { offset: usize },
}

pub(crate) fn decode_globals(module: &FramedModule<'_>) -> Result<Vec<Global>, GlobalError> {
    let Some(section) = module.sections.iter().find(|section| section.id == 6) else {
        return Ok(Vec::new());
    };
    let mut reader = Reader::new(section.payload, section.payload_offset);
    let count = reader.read_u32_leb().map_err(GlobalError::Binary)?;
    let mut globals = Vec::new();
    for _ in 0..count {
        let offset = reader.offset();
        let byte = reader.read_byte().map_err(GlobalError::Binary)?;
        if byte != 0x7f {
            return Err(GlobalError::UnsupportedType { byte, offset });
        }
        let offset = reader.offset();
        let byte = reader.read_byte().map_err(GlobalError::Binary)?;
        let mutable = match byte {
            0 => false,
            1 => true,
            _ => return Err(GlobalError::InvalidMutability { byte, offset }),
        };
        let offset = reader.offset();
        let opcode = reader.read_byte().map_err(GlobalError::Binary)?;
        if opcode != 0x41 {
            return Err(GlobalError::UnsupportedInitializer { opcode, offset });
        }
        let value = reader.read_i32_leb().map_err(GlobalError::Binary)?;
        let offset = reader.offset();
        if reader.read_byte().map_err(GlobalError::Binary)? != 0x0b {
            return Err(GlobalError::InvalidInitializerEnd { offset });
        }
        globals.try_reserve(1).map_err(|_| {
            GlobalError::Binary(BinaryError::new(offset, BinaryErrorKind::AllocationFailed))
        })?;
        globals.push(Global { mutable, value });
    }
    if !reader.is_empty() {
        return Err(GlobalError::Binary(BinaryError::new(
            reader.offset(),
            BinaryErrorKind::TrailingSectionData(6),
        )));
    }
    Ok(globals)
}

#[cfg(test)]
mod tests {
    use super::{GlobalError, decode_globals};
    use crate::wasm::{BinaryErrorKind, binary::scan_module, test_support::module};

    #[test]
    fn rejects_invalid_and_unsupported_global_encodings() {
        let original = module("(module (global i32 (i32.const -1)))");
        let framed = scan_module(&original).unwrap();
        let offset = framed
            .sections
            .iter()
            .find(|section| section.id == 6)
            .unwrap()
            .payload_offset;
        for (relative, byte) in [(1, 0x7e), (2, 2), (3, 0x23), (5, 0)] {
            let mut bytes = original.clone();
            bytes[offset + relative] = byte;
            let result = decode_globals(&scan_module(&bytes).unwrap());
            assert!(match relative {
                1 => matches!(result, Err(GlobalError::UnsupportedType { .. })),
                2 => matches!(result, Err(GlobalError::InvalidMutability { .. })),
                3 => matches!(result, Err(GlobalError::UnsupportedInitializer { .. })),
                _ => matches!(result, Err(GlobalError::InvalidInitializerEnd { .. })),
            });
        }
        let mut bytes = b"\0asm\x01\0\0\0".to_vec();
        bytes.extend_from_slice(&[6, 2, 0, 0]);
        assert!(matches!(
            decode_globals(&scan_module(&bytes).unwrap()),
            Err(GlobalError::Binary(error)) if error.kind == BinaryErrorKind::TrailingSectionData(6)
        ));
    }
}
