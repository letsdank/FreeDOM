//! Decoding one baseline JPEG minimum coded unit.
//!
//! ITU-T T.81, sections A.2 and F.2.2:
//! <https://www.w3.org/Graphics/JPEG/itu-t81.pdf>
//!
//! This does not locate MCUs in an image, handle restart markers, perform
//! chroma upsampling, or convert component samples to RGB.

use super::{
    BaselineScanError, BlockDecodeError, DequantizeError, EntropyReader, HuffmanBuildError,
    HuffmanDecoder, HuffmanTables, JpegFrameHeader, QuantizationTable, ScanHeader,
    decode_baseline_block, dequantize, idct_8bit, validate_baseline_scan,
};

#[derive(Debug)]
pub enum McuError {
    Scan(BaselineScanError),
    InvalidFramePrecision(u8),
    InvalidSampling(u8),
    TooManyBlocks,
    MissingQuantization(u8),
    UnsupportedQuantizationPrecision(u8),
    InvalidQuantization(DequantizeError),
    Huffman(HuffmanBuildError),
    Block(BlockDecodeError),
}

impl std::fmt::Display for McuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JPEG MCU error: {self:?}")
    }
}

impl std::error::Error for McuError {}

/// Component blocks are ordered left-to-right, then top-to-bottom.
pub struct McuComponent {
    pub id: u8,
    pub block_columns: u8,
    pub block_rows: u8,
    pub blocks: Vec<[u8; 64]>,
}

pub struct Mcu {
    pub components: Vec<McuComponent>,
}

struct ComponentDecoder {
    id: u8,
    block_columns: u8,
    block_rows: u8,
    dc: HuffmanDecoder,
    ac: HuffmanDecoder,
    quantization: QuantizationTable,
    predictor: i32,
}

pub struct BaselineMcuDecoder {
    components: Vec<ComponentDecoder>,
}

impl BaselineMcuDecoder {
    pub fn new(
        frame: &JpegFrameHeader,
        scan: &ScanHeader,
        huffman: &HuffmanTables,
        quantization: &[Option<QuantizationTable>; 4],
    ) -> Result<Self, McuError> {
        validate_baseline_scan(frame, scan, huffman).map_err(McuError::Scan)?;

        if frame.precision != 8 {
            return Err(McuError::InvalidFramePrecision(frame.precision));
        }

        let interleaved = scan.components.len() > 1;

        let mut components = Vec::with_capacity(scan.components.len());
        let mut total_blocks = 0usize;

        for selected in &scan.components {
            let component = frame
                .components
                .iter()
                .find(|item| item.id == selected.id)
                .ok_or(McuError::Scan(BaselineScanError::UnknownComponent(
                    selected.id,
                )))?;

            if !(1..=4).contains(&component.horizontal_sampling)
                || !(1..=4).contains(&component.vertical_sampling)
            {
                return Err(McuError::InvalidSampling(component.id));
            }

            let columns = if interleaved {
                component.horizontal_sampling
            } else {
                1
            };

            let rows = if interleaved {
                component.vertical_sampling
            } else {
                1
            };

            total_blocks += usize::from(columns) * usize::from(rows);

            if total_blocks > 10 {
                return Err(McuError::TooManyBlocks);
            }

            let qid = component.quantization_table;

            let table = quantization
                .get(usize::from(qid))
                .and_then(Option::as_ref)
                .ok_or(McuError::MissingQuantization(qid))?;

            if table.precision != 8 {
                return Err(McuError::UnsupportedQuantizationPrecision(table.precision));
            }

            dequantize(&[0; 64], table).map_err(McuError::InvalidQuantization)?;

            // validate_baseline_scan has already checked both selectors
            // and table presence.
            let dc_table = huffman.dc[selected.dc_table as usize]
                .as_ref()
                .expect("validated DC table");

            let ac_table = huffman.ac[selected.ac_table as usize]
                .as_ref()
                .expect("validated AC table");

            components.push(ComponentDecoder {
                id: component.id,
                block_columns: columns,
                block_rows: rows,

                dc: HuffmanDecoder::new(dc_table).map_err(McuError::Huffman)?,

                ac: HuffmanDecoder::new(ac_table).map_err(McuError::Huffman)?,

                quantization: table.clone(),
                predictor: 0,
            });
        }

        Ok(Self { components })
    }

    /// Resets DC predictors after a caller has consumed a valid restart marker.
    pub fn reset_predictors(&mut self) {
        for component in &mut self.components {
            component.predictor = 0;
        }
    }

    /// Decodes one MCU.
    ///
    /// On error, predictors are unchanged, but input bits may already be
    /// consumed and the same reader cannot safely be retired.
    pub fn decode_next(&mut self, reader: &mut EntropyReader<'_>) -> Result<Mcu, McuError> {
        let mut components = Vec::with_capacity(self.components.len());
        let mut predictors = [0i32; 4];

        for (index, component) in self.components.iter().enumerate() {
            let mut predictor = component.predictor;

            let count = usize::from(component.block_columns) * usize::from(component.block_rows);

            let mut blocks = Vec::with_capacity(count);

            for _ in 0..count {
                let block = decode_baseline_block(reader, &component.dc, &component.ac, predictor)
                    .map_err(McuError::Block)?;

                predictor = block[0];

                let coefficients = dequantize(&block, &component.quantization)
                    .map_err(McuError::InvalidQuantization)?;

                blocks.push(idct_8bit(&coefficients));
            }

            predictors[index] = predictor;

            components.push(McuComponent {
                id: component.id,
                block_columns: component.block_columns,
                block_rows: component.block_rows,
                blocks,
            });
        }

        for (component, predictor) in self.components.iter_mut().zip(predictors) {
            component.predictor = predictor;
        }

        Ok(Mcu { components })
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        EntropyErrorKind, HuffmanDecodeError, HuffmanTable, JpegComponent, JpegFrameKind,
        ScanComponent, read_first_scan_header, read_frame_header, read_huffman_tables,
        read_quantization_tables,
    };
    use super::*;

    fn synthetic() -> (
        JpegFrameHeader,
        ScanHeader,
        HuffmanTables,
        [Option<QuantizationTable>; 4],
    ) {
        let frame = JpegFrameHeader {
            kind: JpegFrameKind::Baseline,
            width: 16,
            height: 16,
            precision: 8,
            components: vec![
                JpegComponent {
                    id: 1,
                    horizontal_sampling: 2,
                    vertical_sampling: 2,
                    quantization_table: 0,
                },
                JpegComponent {
                    id: 2,
                    horizontal_sampling: 1,
                    vertical_sampling: 1,
                    quantization_table: 0,
                },
                JpegComponent {
                    id: 3,
                    horizontal_sampling: 1,
                    vertical_sampling: 1,
                    quantization_table: 0,
                },
            ],
        };

        let scan = ScanHeader {
            components: (1..=3)
                .map(|id| ScanComponent {
                    id,
                    dc_table: 0,
                    ac_table: 0,
                })
                .collect(),

            spectral_start: 0,
            spectral_end: 63,
            approximation_high: 0,
            approximation_low: 0,
            entropy_offset: 0,
        };

        let mut counts = [0; 16];
        counts[0] = 1;

        let mut huffman = HuffmanTables::default();

        huffman.dc[0] = Some(HuffmanTable {
            counts,
            symbols: vec![1],
        });

        huffman.ac[0] = Some(HuffmanTable {
            counts,
            symbols: vec![0],
        });

        let quantization = [
            Some(QuantizationTable {
                precision: 8,
                values: [8; 64],
            }),
            None,
            None,
            None,
        ];

        (frame, scan, huffman, quantization)
    }

    fn stream(block_count: usize) -> Vec<u8> {
        let bits = "010".repeat(block_count);

        bits.as_bytes()
            .chunks(8)
            .map(|chunk| {
                chunk
                    .iter()
                    .fold(0u8, |byte, bit| (byte << 1) | (bit - b'0'))
                    << (8 - chunk.len())
            })
            .collect()
    }

    #[test]
    fn decodes_interleaved_420_and_keeps_independent_predictors() {
        let (frame, scan, huffman, quantization) = synthetic();

        let mut decoder = BaselineMcuDecoder::new(&frame, &scan, &huffman, &quantization).unwrap();

        let bytes = stream(12);
        let mut reader = EntropyReader::new(&bytes);

        let first = decoder.decode_next(&mut reader).unwrap();

        assert_eq!(
            (
                first.components[0].block_columns,
                first.components[0].block_rows
            ),
            (2, 2)
        );

        assert_eq!(
            first.components[0]
                .blocks
                .iter()
                .map(|block| block[0])
                .collect::<Vec<_>>(),
            [129, 130, 131, 132]
        );

        assert_eq!(first.components[1].blocks[0], [129; 64]);
        assert_eq!(first.components[2].blocks[0], [129; 64]);

        let second = decoder.decode_next(&mut reader).unwrap();

        assert_eq!(
            second.components[0]
                .blocks
                .iter()
                .map(|block| block[0])
                .collect::<Vec<_>>(),
            [133, 134, 135, 136]
        );

        assert_eq!(second.components[1].blocks[0], [130; 64]);
        assert_eq!(second.components[2].blocks[0], [130; 64]);
    }

    #[test]
    fn non_interleaved_scan_has_one_block_and_reset_clears_predictor() {
        let (frame, mut scan, huffman, quantization) = synthetic();

        scan.components.truncate(1);

        let mut decoder = BaselineMcuDecoder::new(&frame, &scan, &huffman, &quantization).unwrap();

        let bytes = stream(2);
        let mut reader = EntropyReader::new(&bytes);

        let first = decoder.decode_next(&mut reader).unwrap();

        assert_eq!(
            (
                first.components[0].block_columns,
                first.components[0].block_rows
            ),
            (1, 1)
        );

        assert_eq!(first.components[0].blocks[0], [129; 64]);

        decoder.reset_predictors();

        assert_eq!(
            decoder.decode_next(&mut reader).unwrap().components[0].blocks[0],
            [129; 64]
        );
    }

    #[test]
    fn failed_mcu_does_not_commit_predictors() {
        let (frame, scan, huffman, quantization) = synthetic();

        let mut decoder = BaselineMcuDecoder::new(&frame, &scan, &huffman, &quantization).unwrap();

        let result = decoder.decode_next(&mut EntropyReader::new(&[0x49]));

        match result {
            Err(McuError::Block(BlockDecodeError::DcHuffman(HuffmanDecodeError::Entropy(
                error,
            ))))
            | Err(McuError::Block(BlockDecodeError::AcHuffman(HuffmanDecodeError::Entropy(
                error,
            ))))
            | Err(McuError::Block(BlockDecodeError::Entropy(error))) => {
                assert_eq!(error.kind, EntropyErrorKind::Truncated);
            }

            _ => panic!("expected a truncated MCU"),
        }
        let bytes = stream(6);

        let first = decoder
            .decode_next(&mut EntropyReader::new(&bytes))
            .unwrap();

        assert_eq!(first.components[0].blocks[0], [129; 64]);
    }

    #[test]
    fn rejects_missing_invalid_tables_and_excessive_interleaving() {
        let (mut frame, scan, huffman, mut quantization) = synthetic();

        quantization[0] = None;

        assert!(matches!(
            BaselineMcuDecoder::new(&frame, &scan, &huffman, &quantization),
            Err(McuError::MissingQuantization(0))
        ));

        quantization[0] = Some(QuantizationTable {
            precision: 16,
            values: [1; 64],
        });

        assert!(matches!(
            BaselineMcuDecoder::new(&frame, &scan, &huffman, &quantization),
            Err(McuError::UnsupportedQuantizationPrecision(16))
        ));

        quantization[0] = Some(QuantizationTable {
            precision: 8,
            values: [1; 64],
        });

        frame.components[0].horizontal_sampling = 4;
        frame.components[0].vertical_sampling = 4;

        assert!(matches!(
            BaselineMcuDecoder::new(&frame, &scan, &huffman, &quantization),
            Err(McuError::TooManyBlocks)
        ));
    }

    #[test]
    fn decodes_wpt_first_mcu_against_libjpeg_component_samples() {
        // Expected component samples were checked with libjpeg.
        // This tests a WPT asset, not the original browser test.
        let bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/images/arrow-oriented-upright.jpg"
        ));

        let frame = read_frame_header(bytes).unwrap();
        let scan = read_first_scan_header(bytes).unwrap();
        let huffman = read_huffman_tables(bytes).unwrap();
        let quantization = read_quantization_tables(bytes).unwrap();

        let mut decoder = BaselineMcuDecoder::new(&frame, &scan, &huffman, &quantization).unwrap();
        let mut reader = EntropyReader::new(&bytes[scan.entropy_offset..]);
        let mcu = decoder.decode_next(&mut reader).unwrap();

        assert_eq!(
            mcu.components
                .iter()
                .map(|component| component.id)
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );

        assert_eq!(mcu.components[0].blocks, vec![[0; 64]]);
        assert_eq!(mcu.components[1].blocks, vec![[128; 64]]);
        assert_eq!(mcu.components[2].blocks, vec![[128; 64]]);
    }
}
