use super::block::IMPLICIT_BLOCK_LENGTH;
use super::prefix_code::alphabet_bit_width;
use super::*;

#[test]
fn decodes_an_uncompressed_meta_block() {
    let stream = [0x20, 0x00, 0x10, b'a', b'b', b'c', 0x03];

    assert_eq!(decode_brotli(&stream, 3).unwrap(), b"abc");
}

#[test]
fn skips_metadata_in_an_empty_meta_block() {
    let stream = [0x2c, 0x00, 0x99, 0x03];

    assert_eq!(decode_brotli(&stream, 0).unwrap(), Vec::<u8>::new());
}

#[test]
fn rejects_non_zero_padding_after_a_final_empty_meta_block() {
    let error = decode_brotli(&[0x0e], 0).unwrap_err();

    assert_eq!(error.message, "Brotli byte-alignment padding must be zero");
}

#[test]
fn decodes_a_final_compressed_literal_only_meta_block() {
    // Independently verified with libbrotlidec.
    let stream = [0x42, 0x00, 0x00, 0x00, 0x44, 0x58, 0x60, 0x10, 0x00];

    assert_eq!(decode_brotli(&stream, 3).unwrap(), b"aaa");
}

#[test]
fn decodes_a_non_final_compressed_literal_only_meta_block() {
    // Independently verified with libbrotlidec.
    let stream = [0x20, 0x00, 0x00, 0x00, 0x44, 0x58, 0x60, 0x10, 0xc0];

    assert_eq!(decode_brotli(&stream, 3).unwrap(), b"aaa");
}

#[test]
fn rejects_compressed_output_above_the_configured_limit() {
    let stream = [0x42, 0x00, 0x00, 0x00, 0x44, 0x58, 0x60, 0x10, 0x00];

    let error = decode_brotli(&stream, 2).unwrap_err();

    assert_eq!(
        error.message,
        "Brotli output exceeds the configured size limit"
    );
}

#[test]
fn rejects_bytes_after_a_final_compressed_meta_block() {
    let stream = [0x42, 0x00, 0x00, 0x00, 0x44, 0x58, 0x60, 0x10, 0x00, 0x00];

    let error = decode_brotli(&stream, 3).unwrap_err();

    assert_eq!(
        error.message,
        "Brotli stream has bytes after its final meta-block"
    );
}

#[test]
fn decodes_a_three_symbol_simple_prefix_code() {
    let mut data = Vec::new();
    let mut bit_position = 0;

    append_bits(&mut data, &mut bit_position, 1, 2);
    append_bits(&mut data, &mut bit_position, 2, 2);
    append_bits(&mut data, &mut bit_position, 5, 8);
    append_bits(&mut data, &mut bit_position, 2, 8);
    append_bits(&mut data, &mut bit_position, 9, 8);

    append_bits(&mut data, &mut bit_position, 0, 1);
    append_bits(&mut data, &mut bit_position, 1, 2);
    append_bits(&mut data, &mut bit_position, 3, 2);

    let mut reader = BitReader::new(&data);
    let tree = read_simple_prefix_code(&mut reader, 256).unwrap();

    assert_eq!(tree.decode(&mut reader).unwrap(), 5);
    assert_eq!(tree.decode(&mut reader).unwrap(), 2);
    assert_eq!(tree.decode(&mut reader).unwrap(), 9);
}

#[test]
fn decodes_a_single_symbol_simple_prefix_code_without_consuming_bits() {
    let mut data = Vec::new();
    let mut bit_position = 0;

    append_bits(&mut data, &mut bit_position, 1, 2);
    append_bits(&mut data, &mut bit_position, 0, 2);
    append_bits(&mut data, &mut bit_position, 42, 8);

    let mut reader = BitReader::new(&data);
    let tree = read_simple_prefix_code(&mut reader, 256).unwrap();
    let before_decode = reader.bit_position();

    assert_eq!(tree.decode(&mut reader).unwrap(), 42);
    assert_eq!(reader.bit_position(), before_decode);
}

#[test]
fn rejects_duplicate_symbols_in_a_simple_prefix_code() {
    let mut data = Vec::new();
    let mut bit_position = 0;

    append_bits(&mut data, &mut bit_position, 1, 2);
    append_bits(&mut data, &mut bit_position, 1, 2);
    append_bits(&mut data, &mut bit_position, 7, 8);
    append_bits(&mut data, &mut bit_position, 7, 8);

    let mut reader = BitReader::new(&data);
    let error = read_simple_prefix_code(&mut reader, 256).unwrap_err();

    assert_eq!(
        error.message,
        "Brotli simple prefix code contains a duplicate symbol"
    );
}

#[test]
fn decodes_a_complex_prefix_code() {
    let mut data = Vec::new();
    let mut bit_position = 0;

    append_bits(&mut data, &mut bit_position, 0, 2);
    append_bits(&mut data, &mut bit_position, 7, 4);

    for _ in 0..17 {
        append_bits(&mut data, &mut bit_position, 0, 2);
    }

    append_bits(&mut data, &mut bit_position, 0, 1);
    append_bits(&mut data, &mut bit_position, 1, 1);

    let mut reader = BitReader::new(&data);
    let tree = read_prefix_code(&mut reader, 2).unwrap();

    assert_eq!(tree.decode(&mut reader).unwrap(), 0);
    assert_eq!(tree.decode(&mut reader).unwrap(), 1);
}

#[test]
fn expands_repeated_complex_code_lengths() {
    let mut data = Vec::new();
    let mut bit_position = 0;

    append_bits(&mut data, &mut bit_position, 0, 2);
    append_bits(&mut data, &mut bit_position, 0, 2);
    append_bits(&mut data, &mut bit_position, 0, 2);
    append_bits(&mut data, &mut bit_position, 7, 4);

    for _ in 0..5 {
        append_bits(&mut data, &mut bit_position, 0, 2);
    }

    append_bits(&mut data, &mut bit_position, 7, 4);

    append_bits(&mut data, &mut bit_position, 0, 1);
    append_bits(&mut data, &mut bit_position, 1, 1);
    append_bits(&mut data, &mut bit_position, 0, 2);
    append_bits(&mut data, &mut bit_position, 1, 1);
    append_bits(&mut data, &mut bit_position, 0, 2);
    append_bits(&mut data, &mut bit_position, 5, 3);

    let mut reader = BitReader::new(&data);
    let tree = read_prefix_code(&mut reader, 8).unwrap();

    assert_eq!(tree.decode(&mut reader).unwrap(), 5);
}

#[test]
fn decodes_brotli_block_type_counts() {
    for expected in [
        1_usize, 2, 3, 4, 5, 8, 9, 16, 17, 32, 33, 64, 65, 128, 129, 256,
    ] {
        let mut data = Vec::new();
        let mut bit_position = 0;

        append_variable_length_u8(&mut data, &mut bit_position, expected - 1);

        let mut reader = BitReader::new(&data);

        assert_eq!(read_block_type_count(&mut reader).unwrap(), expected);
    }
}

#[test]
fn reads_a_single_type_brotli_block_partition() {
    let mut data = Vec::new();
    let mut bit_position = 0;

    append_variable_length_u8(&mut data, &mut bit_position, 0);

    let mut reader = BitReader::new(&data);
    let partition = read_block_partition(&mut reader).unwrap();

    assert_eq!(partition.block_type_count, 1);
    assert_eq!(partition.current_type, 0);
    assert_eq!(partition.previous_type, 1);
    assert_eq!(partition.remaining, IMPLICIT_BLOCK_LENGTH);
    assert!(partition.block_type_tree.is_none());
    assert!(partition.block_length_tree.is_none());
}

#[test]
fn reads_a_multi_type_brotli_block_partition() {
    let mut data = Vec::new();
    let mut bit_position = 0;

    append_variable_length_u8(&mut data, &mut bit_position, 1);

    append_bits(&mut data, &mut bit_position, 1, 2);
    append_bits(&mut data, &mut bit_position, 0, 2);
    append_bits(&mut data, &mut bit_position, 0, 2);

    append_bits(&mut data, &mut bit_position, 1, 2);
    append_bits(&mut data, &mut bit_position, 0, 2);
    append_bits(&mut data, &mut bit_position, 0, 5);

    append_bits(&mut data, &mut bit_position, 3, 2);

    let mut reader = BitReader::new(&data);
    let partition = read_block_partition(&mut reader).unwrap();

    assert_eq!(partition.block_type_count, 2);
    assert_eq!(partition.current_type, 0);
    assert_eq!(partition.previous_type, 1);
    assert_eq!(partition.remaining, 4);
    assert!(partition.block_type_tree.is_some());
    assert!(partition.block_length_tree.is_some());
}

#[test]
fn switches_brotli_block_types_after_the_current_block_expires() {
    let mut data = Vec::new();
    let mut bit_position = 0;

    append_bits(&mut data, &mut bit_position, 2, 2);

    let mut reader = BitReader::new(&data);
    let mut partition = BlockPartition {
        block_type_count: 3,
        block_type_tree: Some(HuffmanTree::single(1)),
        block_length_tree: Some(HuffmanTree::single(0)),
        previous_type: 1,
        current_type: 0,
        remaining: 0,
    };

    assert_eq!(partition.take_block_type(&mut reader).unwrap(), 1);
    assert_eq!(partition.previous_type, 0);
    assert_eq!(partition.current_type, 1);
    assert_eq!(partition.remaining, 2);

    assert_eq!(partition.take_block_type(&mut reader).unwrap(), 1);
    assert_eq!(partition.take_block_type(&mut reader).unwrap(), 1);
    assert_eq!(partition.remaining, 0);
}

#[test]
fn reuses_the_previous_brotli_block_type() {
    let mut data = Vec::new();
    let mut bit_position = 0;

    append_bits(&mut data, &mut bit_position, 0, 2);

    let mut reader = BitReader::new(&data);
    let mut partition = BlockPartition {
        block_type_count: 3,
        block_type_tree: Some(HuffmanTree::single(0)),
        block_length_tree: Some(HuffmanTree::single(0)),
        previous_type: 1,
        current_type: 0,
        remaining: 0,
    };

    assert_eq!(partition.take_block_type(&mut reader).unwrap(), 1);
    assert_eq!(partition.previous_type, 0);
    assert_eq!(partition.current_type, 1);
    assert_eq!(partition.remaining, 0);
}

#[test]
fn reads_brotli_block_lengths_from_the_length_tree() {
    let mut data = Vec::new();
    let mut bit_position = 0;

    append_variable_length_u8(&mut data, &mut bit_position, 1);
    append_single_symbol_prefix_code(&mut data, &mut bit_position, 4, 0);
    append_single_symbol_prefix_code(&mut data, &mut bit_position, 26, 1);
    append_bits(&mut data, &mut bit_position, 0, 2);

    let mut reader = BitReader::new(&data);
    let partition = read_block_partition(&mut reader).unwrap();

    assert_eq!(partition.block_type_count, 2);
    assert_eq!(partition.remaining, 5);
}

#[test]
fn computes_brotli_literal_context_ids() {
    assert_eq!(literal_context_id(LiteralContextMode::Lsb6, 0xd5, 0), 0x15);
    assert_eq!(literal_context_id(LiteralContextMode::Msb6, 0xd5, 0), 0x35);
    assert_eq!(literal_context_id(LiteralContextMode::Utf8, b'A', b'a'), 51);
    assert_eq!(
        literal_context_id(LiteralContextMode::Signed, 0xf0, 0x10),
        50
    );
    assert_eq!(literal_context_id(LiteralContextMode::Utf8, b'U', 0), 48);
    assert_eq!(literal_context_id(LiteralContextMode::Utf8, b'V', 0), 52);
}

#[test]
fn computes_brotli_distance_context_ids() {
    assert_eq!(distance_context_id(2), 0);
    assert_eq!(distance_context_id(3), 1);
    assert_eq!(distance_context_id(4), 2);
    assert_eq!(distance_context_id(5), 3);
    assert_eq!(distance_context_id(128), 3);
}

#[test]
fn decodes_brotli_insert_copy_commands() {
    let mut reader = BitReader::new(&[]);

    assert_eq!(
        read_insert_copy_command(&mut reader, 0).unwrap(),
        InsertCopyCommand {
            insert_length: 0,
            copy_length: 2,
            implicit_distance: true,
        }
    );

    assert_eq!(
        read_insert_copy_command(&mut reader, 128).unwrap(),
        InsertCopyCommand {
            insert_length: 0,
            copy_length: 2,
            implicit_distance: false,
        }
    );

    let mut reader = BitReader::new(&[0b0000_0011]);

    // Symbol 64 uses insert code 0 and copy code 8.
    // Copy code 8 has one extra bit.
    assert_eq!(
        read_insert_copy_command(&mut reader, 64).unwrap(),
        InsertCopyCommand {
            insert_length: 0,
            copy_length: 11,
            implicit_distance: true,
        }
    );
    assert_eq!(reader.bit_position(), 1);
}

#[test]
fn decodes_brotli_large_insert_copy_lengths() {
    let mut reader = BitReader::new(&[0xff; 6]);

    assert_eq!(
        read_insert_copy_command(&mut reader, 703).unwrap(),
        InsertCopyCommand {
            insert_length: 16_799_809,
            copy_length: 16_779_333,
            implicit_distance: false,
        }
    );
    assert_eq!(reader.bit_position(), 48);
}

#[test]
fn rejects_invalid_or_truncated_brotli_commands() {
    let mut reader = BitReader::new(&[]);
    assert!(read_insert_copy_command(&mut reader, 704).is_err());

    let mut reader = BitReader::new(&[0; 5]);
    assert!(read_insert_copy_command(&mut reader, 703).is_err());
}

#[test]
fn copies_bytes_from_a_previous_meta_block() {
    // Independently verified with libbrotlidec.
    let stream = [
        0x30, 0x00, 0x10, b'a', b'b', b'c', b'd', 0x11, 0x00, 0x00, 0x00, 0x22, 0x2c, 0x00, 0x08,
        0x00,
    ];

    assert_eq!(decode_brotli(&stream, 6).unwrap(), b"abcdab");
}

#[test]
fn copies_overlapping_brotli_data() {
    // Independently verified with libbrotlidec.
    let stream = [0x42, 0x01, 0x00, 0x04, 0x44, 0x58, 0x20, 0x13, 0x10];

    assert_eq!(decode_brotli(&stream, 11).unwrap(), b"aaaaaaaaaaa");
}

#[test]
fn rejects_a_copy_that_exceeds_the_meta_block_length() {
    let stream = [
        0x30, 0x00, 0x10, b'a', b'b', b'c', b'd', 0x01, 0x00, 0x00, 0x00, 0x22, 0x2c, 0x00, 0x08,
        0x00,
    ];

    let error = decode_brotli(&stream, 6).unwrap_err();

    assert_eq!(
        error.message,
        "Brotli backward copy exceeds the meta-block length",
    );
}

pub(super) fn append_bits(
    data: &mut Vec<u8>,
    bit_position: &mut usize,
    value: u32,
    bit_count: usize,
) {
    for bit_index in 0..bit_count {
        if *bit_position / 8 == data.len() {
            data.push(0);
        }

        let bit = (value >> bit_index) & 1;

        data[*bit_position / 8] |= (bit as u8) << (*bit_position % 8);
        *bit_position += 1;
    }
}

pub(super) fn append_variable_length_u8(
    data: &mut Vec<u8>,
    bit_position: &mut usize,
    value: usize,
) {
    if value == 0 {
        append_bits(data, bit_position, 0, 1);
        return;
    }

    append_bits(data, bit_position, 1, 1);

    let bit_count = (usize::BITS - value.leading_zeros() - 1) as usize;
    append_bits(data, bit_position, bit_count as u32, 3);
    append_bits(
        data,
        bit_position,
        (value - (1_usize << bit_count)) as u32,
        bit_count,
    );
}

pub(super) fn append_single_symbol_prefix_code(
    data: &mut Vec<u8>,
    bit_position: &mut usize,
    alphabet_size: usize,
    symbol: usize,
) {
    append_bits(data, bit_position, 1, 2);
    append_bits(data, bit_position, 0, 2);
    append_bits(
        data,
        bit_position,
        symbol as u32,
        alphabet_bit_width(alphabet_size).unwrap(),
    );
}
