use std::io::{self, Read, Write};

use super::encode_fixed_literals;

use super::super::{
    DeflateBitReader, DeflateBlockType, DeflateOutput, decode_fixed_huffman_block,
    read_block_header,
};

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
