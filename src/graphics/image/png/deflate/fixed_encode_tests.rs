use super::super::{
    DeflateBitReader, DeflateBlockType, DeflateOutput, decode_fixed_huffman_block,
    read_block_header,
};
use super::{encode_fixed_literals, encode_fixed_lz77, write_match, write_symbol};
use crate::graphics::image::png_format::DeflateBitWriter;
use std::io::{self, Read, Write};

#[test]
fn matches_known_streams() {
    assert_eq!(
        encode_fixed_literals(b"", Vec::new()).unwrap(),
        [0x03, 0x00]
    );

    assert_eq!(
        encode_fixed_literals(b"hello", Vec::new()).unwrap(),
        [0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00]
    );
}

#[test]
fn decodes_every_literal_and_larger_inputs() {
    let inputs = [
        Vec::new(),
        vec![0, 143, 144, 255],
        (0..=255).collect::<Vec<u8>>(),
        (0..65_536).map(|i| (i * 73 + i / 31) as u8).collect(),
    ];

    for input in inputs {
        let encoded = encode_fixed_literals(&input, Vec::new()).unwrap();

        let mut bits = DeflateBitReader::new(encoded.as_slice());
        let header = read_block_header(&mut bits).unwrap();

        assert!(header.is_final);
        assert_eq!(header.block_type, DeflateBlockType::FixedHuffman);

        let mut output = DeflateOutput::new(Vec::new(), 32_768, input.len()).unwrap();

        decode_fixed_huffman_block(&mut bits, &mut output).unwrap();

        assert_eq!(output.into_inner(), input);

        // Validate the stream using an independent decoder as well.
        let mut independent = Vec::new();

        flate2::read::DeflateDecoder::new(encoded.as_slice())
            .read_to_end(&mut independent)
            .unwrap();

        assert_eq!(independent, input);
    }
}

struct Stalled;

impl Write for Stalled {
    fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
        Ok(0)
    }

    fn flush(&mut self) -> io::Result<()> {
        panic!("encoder must not flush")
    }
}

#[test]
fn propagates_writer_failure() {
    assert!(matches!(
        encode_fixed_literals(b"hello",Stalled),
        Err(error) if error.kind() == io::ErrorKind::WriteZero,
    ));
}

#[test]
fn leaves_underlying_writer_unflushed() {
    #[derive(Default)]
    struct FlushTrap(Vec<u8>);

    impl Write for FlushTrap {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            panic!("encoder must not flush")
        }
    }

    let result = encode_fixed_literals(b"hello", FlushTrap::default()).unwrap();

    assert_eq!(result.0, [0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00]);
}

fn check_both(encoded: &[u8], expected: &[u8]) {
    let mut reader = DeflateBitReader::new(encoded);
    let header = read_block_header(&mut reader).unwrap();

    assert!(header.is_final);
    assert_eq!(header.block_type, DeflateBlockType::FixedHuffman);

    let mut decoded = DeflateOutput::new(Vec::new(), 32_768, expected.len()).unwrap();

    decode_fixed_huffman_block(&mut reader, &mut decoded).unwrap();

    assert_eq!(decoded.into_inner(), expected);

    let mut independent = Vec::new();

    flate2::read::DeflateDecoder::new(encoded)
        .read_to_end(&mut independent)
        .unwrap();

    assert_eq!(independent, expected);
}

fn stream_with_match(history: &[u8], length: usize, distance: usize) -> (Vec<u8>, Vec<u8>) {
    let mut bits = DeflateBitWriter::new(Vec::new());

    bits.write_bits(1, 1).unwrap();
    bits.write_bits(1, 2).unwrap();

    for &byte in history {
        write_symbol(&mut bits, u16::from(byte)).unwrap();
    }

    write_match(&mut bits, length, distance).unwrap();
    write_symbol(&mut bits, 256).unwrap();

    let mut expected = history.to_vec();

    // The match may overlap with bytes it has already produced.
    for _ in 0..length {
        let byte = expected[expected.len() - distance];
        expected.push(byte);
    }

    (bits.finish().unwrap(), expected)
}

#[test]
fn encodes_every_match_length() {
    for length in 3..=258 {
        let (encoded, expected) = stream_with_match(&[42], length, 1);

        check_both(&encoded, &expected);
    }
}

#[test]
fn encodes_distance_code_boundaries() {
    use super::super::tables::{DISTANCE_BASE, DISTANCE_EXTRA};

    for (&base, &extra) in DISTANCE_BASE.iter().zip(DISTANCE_EXTRA.iter()) {
        let upper = base + ((1usize << extra) - 1);

        for distance in [base, upper] {
            let history: Vec<u8> = (0..distance).map(|i| (i * 29 + i / 113) as u8).collect();

            let (encoded, expected) = stream_with_match(&history, 3, distance);

            check_both(&encoded, &expected);
        }
    }
}

#[test]
fn encodes_lz77_streams_with_fixed_huffman() {
    let mut state = 0x1234_5678u32;

    let random: Vec<u8> = (0..65_536)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;

            (state >> 24) as u8
        })
        .collect();

    let inputs = [
        Vec::new(),
        b"abc".to_vec(),
        vec![0; 10_000],
        b"abracadabra".repeat(1_000),
        random,
    ];

    for input in inputs {
        let encoded = encode_fixed_lz77(&input, Vec::new()).unwrap();

        check_both(&encoded, &input);
    }

    let repeated = vec![0; 10_000];
    let encoded = encode_fixed_lz77(&repeated, Vec::new()).unwrap();

    assert!(encoded.len() < repeated.len() / 10);
}

#[test]
fn lz77_encoder_propagates_write_errors() {
    assert!(matches!(
        encode_fixed_lz77(b"hello", Stalled),
        Err(error) if error.kind() == io::ErrorKind::WriteZero,
    ));
}
