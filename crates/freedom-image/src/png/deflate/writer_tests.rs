use std::io::{self, Read, Write};

use super::super::super::DeflateBitReader;
use super::DeflateBitWriter;

#[test]
fn writes_fields_across_byte_boundaries() {
    let mut writer = DeflateBitWriter::new(Vec::new());

    for (value, count) in [(6, 3), (2, 4), (11, 5), (12, 4)] {
        writer.write_bits(value, count).unwrap();
    }

    let bytes = writer.finish().unwrap();

    assert_eq!(bytes, [0x96, 0xc5]);

    let mut reader = DeflateBitReader::new(bytes.as_slice());

    for (value, count) in [(6, 3), (2, 4), (11, 5), (12, 4)] {
        assert_eq!(reader.read_bits(count).unwrap(), value);
    }
}

#[test]
fn handles_zero_32_bit_values_and_masks_unused_bits() {
    let mut writer = DeflateBitWriter::new(Vec::new());

    writer.write_bits(u32::MAX, 0).unwrap();
    writer.write_bits(0x1234_5678, 32).unwrap();

    writer.write_bits(0xff, 3).unwrap();
    writer.write_bits(0, 5).unwrap();

    assert_eq!(writer.finish().unwrap(), [0x78, 0x56, 0x34, 0x12, 0x07]);
}

#[test]
fn aligns_with_zero_padding_and_can_continue() {
    let mut writer = DeflateBitWriter::new(Vec::new());

    writer.write_bits(0xabc, 12).unwrap();
    writer.align_to_byte().unwrap();
    writer.align_to_byte().unwrap();
    writer.write_bits(0x42, 8).unwrap();

    assert_eq!(writer.finish().unwrap(), [0xbc, 0x0a, 0x42]);
}

#[test]
fn rejects_oversized_fields_without_changing_pending_bits() {
    let mut writer = DeflateBitWriter::new(Vec::new());

    writer.write_bits(5, 3).unwrap();

    assert_eq!(
        writer.write_bits(0, 33).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );

    writer.write_bits(0x1f, 5).unwrap();

    assert_eq!(writer.finish().unwrap(), [0xfd]);
}

#[test]
fn writes_fixed_huffman_hello_stream() {
    let mut writer = DeflateBitWriter::new(Vec::new());

    // BFINAL=1, BTYPE=01: final fixed-Huffman block.
    writer.write_bits(1, 1).unwrap();
    writer.write_bits(1, 2).unwrap();

    for byte in b"hello" {
        // Fixed literal codes for values 0..=143 have eight bits.
        let canonical_code = 0x30 + u32::from(*byte);
        let transmitted = canonical_code.reverse_bits() >> 24;

        writer.write_bits(transmitted, 8).unwrap();
    }

    // End-of-block symbol 256 has the fixed seven-bit code 0000000.
    writer.write_bits(0, 7).unwrap();

    let encoded = writer.finish().unwrap();

    assert_eq!(encoded, [0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00]);

    let mut independent = Vec::new();

    flate2::read::DeflateDecoder::new(encoded.as_slice())
        .read_to_end(&mut independent)
        .unwrap();

    assert_eq!(independent, b"hello");
}

struct Stalled;

impl Write for Stalled {
    fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
        Ok(0)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn propagates_write_zero_during_full_and_partial_byte_output() {
    let mut full = DeflateBitWriter::new(Stalled);

    assert_eq!(
        full.write_bits(0, 8).unwrap_err().kind(),
        io::ErrorKind::WriteZero
    );

    let mut partial = DeflateBitWriter::new(Stalled);

    partial.write_bits(1, 1).unwrap();

    assert!(matches!(
        partial.finish(),
        Err(error) if error.kind() == io::ErrorKind::WriteZero
    ));
}
