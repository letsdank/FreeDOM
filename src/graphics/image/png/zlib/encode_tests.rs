use std::io::{self, Read, Write};

use super::super::decode_zlib;
use super::encode_zlib_stored;

#[test]
fn encodes_known_streams() {
    let mut output = Vec::new();

    encode_zlib_stored(b"", &mut output).unwrap();

    assert_eq!(
        output,
        [
            0x78, 0x01, 0x01, 0x00, 0x00, 0xff, 0xff, 0x00, 0x00, 0x00, 0x01
        ]
    );

    output.clear();

    encode_zlib_stored(b"abc", &mut output).unwrap();

    assert_eq!(
        output,
        [
            0x78, 0x01, 0x01, 0x03, 0x00, 0xfc, 0xff, b'a', b'b', b'c', 0x02, 0x4d, 0x01, 0x27
        ]
    );
}

#[test]
fn roundtrips_stored_blocks_at_boundaries() {
    for len in [0, 1, 65_534, 65_535, 65_536, 131_070, 131_071] {
        let input: Vec<u8> = (0..len).map(|i| (i * 37 + i / 17) as u8).collect();

        let mut encoded = Vec::new();

        encode_zlib_stored(&input, &mut encoded).unwrap();

        let decoded = decode_zlib(encoded.as_slice(), Vec::new(), len).unwrap();

        assert_eq!(decoded.output, input, "FreeDOM decoder: length {len}");

        let mut independent = Vec::new();
        let mut reader = flate2::read::ZlibDecoder::new(encoded.as_slice());

        reader.read_to_end(&mut independent).unwrap();

        assert_eq!(
            reader.total_in() as usize,
            encoded.len(),
            "independent decoder did not consume the complete stream"
        );

        assert_eq!(independent, input, "independent decoder: length {len}");
    }
}

struct ShortWriter {
    bytes: Vec<u8>,
    max: usize,
}

impl Write for ShortWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let count = data.len().min(self.max);

        self.bytes.extend_from_slice(&data[..count]);

        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn handles_short_writes_and_rejects_write_zero() {
    let mut short = ShortWriter {
        bytes: Vec::new(),
        max: 1,
    };

    encode_zlib_stored(b"short writes", &mut short).unwrap();

    let decoded = decode_zlib(short.bytes.as_slice(), Vec::new(), 12).unwrap();

    assert_eq!(decoded.output, b"short writes");

    let mut stalled = ShortWriter {
        bytes: Vec::new(),
        max: 0,
    };

    let error = encode_zlib_stored(b"x", &mut stalled).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::WriteZero);
}
