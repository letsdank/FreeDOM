//! Initial progressive DC scans.
//!
//! ITU-T T.81, sections A.2, F.2.2 and G.1.2.1:
//! <https://www.w3.org/Graphics/JPEG/itu-t81.pdf>
//!
//! Refinement scans and AC scans are not decoded here.

use super::{
    BlockDecodeError, EntropyError, EntropyReader, HuffmanBuildError, HuffmanDecoder,
    ProgressiveState, ScanHeader, block::decode_dc,
};

#[derive(Debug)]
pub enum InitialDcError {
    InvalidScan,
    UnknownComponent(u8),
    ComponentOrder,
    AlreadyInitialized(u8),
    TooManyBlocks,
    MissingDcTable(u8),
    InvalidEntropyRange,
    UnexpectedScanEnd,
    ValueOverflow,
    AllocationFailed,
    Huffman(HuffmanBuildError),
    Block(BlockDecodeError),
    Entropy(EntropyError),
}

impl std::fmt::Display for InitialDcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "progressive JPEG initial DC scan error: {self:?}")
    }
}

impl std::error::Error for InitialDcError {}

struct Selected {
    id: u8,
    h: usize,
    v: usize,
    table: HuffmanDecoder,
    predictor: i32,
}

impl ProgressiveState {
    /// Decodes an initial DC scan and commits it only after validating the
    /// scan boundary. Entropy errors use offsets relative to entropy_offset.
    pub fn decode_initial_dc_scan(
        &mut self,
        input: &[u8],
        scan: &ScanHeader,
        entropy_end: usize,
        restart_interval: u16,
    ) -> Result<(), InitialDcError> {
        if scan.spectral_start != 0
            || scan.spectral_end != 0
            || scan.approximation_high != 0
            || scan.approximation_low > 13
            || scan.components.is_empty()
            || scan.components.len() > 4
        {
            return Err(InitialDcError::InvalidScan);
        }

        let entropy = input
            .get(scan.entropy_offset..)
            .ok_or(InitialDcError::InvalidEntropyRange)?;

        let end = entropy_end
            .checked_sub(scan.entropy_offset)
            .ok_or(InitialDcError::InvalidEntropyRange)?;

        if input.get(entropy_end) != Some(&0xff) {
            return Err(InitialDcError::InvalidEntropyRange);
        }

        let interleaved = scan.components.len() > 1;

        let mut selected = Vec::with_capacity(scan.components.len());
        let mut previous = None;
        let mut blocks_per_mcu = 0usize;

        for component in &scan.components {
            let index = self
                .frame
                .components
                .iter()
                .position(|item| item.id == component.id)
                .ok_or(InitialDcError::UnknownComponent(component.id))?;

            if previous.is_some_and(|previous| index <= previous) {
                return Err(InitialDcError::ComponentOrder);
            }

            previous = Some(index);

            let storage = self
                .component(component.id)
                .ok_or(InitialDcError::UnknownComponent(component.id))?;

            if storage.dc_initialized {
                return Err(InitialDcError::AlreadyInitialized(component.id));
            }

            let spec = self.frame.components[index];

            let h = if interleaved {
                usize::from(spec.horizontal_sampling)
            } else {
                1
            };

            let v = if interleaved {
                usize::from(spec.vertical_sampling)
            } else {
                1
            };

            blocks_per_mcu += h * v;

            let table = self
                .huffman_tables()
                .dc
                .get(usize::from(component.dc_table))
                .and_then(Option::as_ref)
                .ok_or(InitialDcError::MissingDcTable(component.dc_table))?;

            selected.push(Selected {
                id: component.id,
                h,
                v,
                table: HuffmanDecoder::new(table).map_err(InitialDcError::Huffman)?,
                predictor: 0,
            });
        }

        if blocks_per_mcu > 10 {
            return Err(InitialDcError::TooManyBlocks);
        }

        let (columns, rows) = if interleaved {
            let max_h = usize::from(
                self.frame
                    .components
                    .iter()
                    .map(|component| component.horizontal_sampling)
                    .max()
                    .unwrap(),
            );

            let max_v = usize::from(
                self.frame
                    .components
                    .iter()
                    .map(|component| component.vertical_sampling)
                    .max()
                    .unwrap(),
            );

            (
                usize::from(self.frame.width).div_ceil(max_h * 8),
                usize::from(self.frame.height).div_ceil(max_v * 8),
            )
        } else {
            self.component(selected[0].id).unwrap().visible_dimensions()
        };

        let count = columns
            .checked_mul(rows)
            .and_then(|count| count.checked_mul(blocks_per_mcu))
            .ok_or(InitialDcError::ValueOverflow)?;

        let mut staged = Vec::new();

        staged
            .try_reserve_exact(count)
            .map_err(|_| InitialDcError::AllocationFailed)?;

        let mut reader = EntropyReader::new(entropy);
        let mut completed = 0usize;
        let mut next_restart = 0u8;

        for mcu_y in 0..rows {
            for mcu_x in 0..columns {
                if restart_interval != 0
                    && completed != 0
                    && completed % usize::from(restart_interval) == 0
                {
                    reader
                        .consume_restart_marker(0xd0 + next_restart)
                        .map_err(InitialDcError::Entropy)?;

                    for component in &mut selected {
                        component.predictor = 0;
                    }

                    next_restart = (next_restart + 1) % 8;
                }

                for component in &mut selected {
                    for block_y in 0..component.v {
                        for block_x in 0..component.h {
                            let dc = decode_dc(&mut reader, &component.table, component.predictor)
                                .map_err(InitialDcError::Block)?;

                            component.predictor = dc;

                            let scaled = i64::from(dc) * (1i64 << scan.approximation_low);

                            let value =
                                i32::try_from(scaled).map_err(|_| InitialDcError::ValueOverflow)?;

                            staged.push((
                                component.id,
                                mcu_x * component.h + block_x,
                                mcu_y * component.v + block_y,
                                value,
                            ));
                        }
                    }
                }

                completed += 1;
            }
        }

        if reader.byte_position() != end {
            return Err(InitialDcError::UnexpectedScanEnd);
        }

        let marker = reader
            .peek_scan_end_marker()
            .map_err(InitialDcError::Entropy)?;

        if (0xd0..=0xd7).contains(&marker) {
            return Err(InitialDcError::UnexpectedScanEnd);
        }

        for (id, x, y, value) in staged {
            self.component_mut(id).unwrap().block_mut(x, y).unwrap()[0] = value;
        }

        for component in selected {
            self.component_mut(component.id).unwrap().dc_initialized = true;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        JpegComponent, JpegEvent, JpegEventReader, JpegFrameHeader, JpegFrameKind,
        ProgressiveLimits, ScanComponent, read_frame_header,
    };

    use super::*;

    fn state(width: u16) -> ProgressiveState {
        let frame = JpegFrameHeader {
            kind: JpegFrameKind::Progressive,
            width,
            height: 8,
            precision: 8,

            components: vec![JpegComponent {
                id: 1,
                horizontal_sampling: 1,
                vertical_sampling: 1,
                quantization_table: 0,
            }],
        };

        let mut state = ProgressiveState::new(&frame, ProgressiveLimits::default()).unwrap();

        let mut dht = vec![0, 1];

        dht.extend([0; 15]);
        dht.push(1);

        state.update_huffman(&dht).unwrap();

        state
    }

    fn scan() -> ScanHeader {
        ScanHeader {
            components: vec![ScanComponent {
                id: 1,
                dc_table: 0,
                ac_table: 0,
            }],

            spectral_start: 0,
            spectral_end: 0,
            approximation_high: 0,
            approximation_low: 2,
            entropy_offset: 0,
        }
    }

    #[test]
    fn decodes_dc_predictors_and_point_transform() {
        let mut state = state(24);
        let header = scan();

        state
            .decode_initial_dc_scan(&[0x57, 0xff, 0xd9], &header, 1, 0)
            .unwrap();

        for (x, expected) in [4, 8, 12].into_iter().enumerate() {
            assert_eq!(
                state.component(1).unwrap().block(x, 0).unwrap()[0],
                expected
            );
        }

        assert!(matches!(
            state.decode_initial_dc_scan(&[0x57, 0xff, 0xd9], &header, 1, 0),
            Err(InitialDcError::AlreadyInitialized(1))
        ));

        let mut negative = state_for_negative_test();

        negative
            .decode_initial_dc_scan(&[0x3f, 0xff, 0xd9], &header, 1, 0)
            .unwrap();

        assert_eq!(negative.component(1).unwrap().block(0, 0).unwrap()[0], -4);
    }

    fn state_for_negative_test() -> ProgressiveState {
        state(8)
    }

    #[test]
    fn restarts_predictors_and_does_not_commit_on_failure() {
        let data = [0x7f, 0xff, 0xd0, 0x7f, 0xff, 0xd1, 0x7f, 0xff, 0xd9];

        let mut state = state(24);
        state.decode_initial_dc_scan(&data, &scan(), 7, 1).unwrap();

        for x in 0..3 {
            assert_eq!(state.component(1).unwrap().block(x, 0).unwrap()[0], 4);
        }

        let mut bad = data;
        bad[6] = 0x40;

        let mut retry = state_for_retry_test();

        assert!(retry.decode_initial_dc_scan(&bad, &scan(), 7, 1).is_err());

        for x in 0..3 {
            assert_eq!(retry.component(1).unwrap().block(x, 0).unwrap()[0], 0);
        }

        retry.decode_initial_dc_scan(&data, &scan(), 7, 1).unwrap();
    }

    fn state_for_retry_test() -> ProgressiveState {
        state(24)
    }

    #[test]
    fn decodes_wpt_first_dc_scan_against_libjpeg_coefficients() {
        // libjpeg jpeg_read_coefficients; WPT asset, not a WPT browser test.
        let input = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/images/computer.jpg"
        ));

        let frame = read_frame_header(input).unwrap();
        let mut state = ProgressiveState::new(&frame, ProgressiveLimits::default()).unwrap();
        let mut reader = JpegEventReader::new(input).unwrap();

        loop {
            match reader.next_event().unwrap().unwrap() {
                JpegEvent::Segment {
                    marker: 0xc4, data, ..
                } => {
                    state.update_huffman(data).unwrap();
                }

                JpegEvent::Scan {
                    header,
                    entropy_end,
                } => {
                    assert_eq!(
                        (
                            header.spectral_start,
                            header.spectral_end,
                            header.approximation_high,
                            header.approximation_low
                        ),
                        (0, 0, 0, 0)
                    );

                    state
                        .decode_initial_dc_scan(input, &header, entropy_end, 0)
                        .unwrap();

                    break;
                }

                JpegEvent::End { .. } => panic!("missing DC scan"),

                _ => {}
            }
        }

        for (id, expected) in [
            (1, [-1, -1, 0, 0, 0, 1, 1, 1, 2, 3]),
            (2, [0; 10]),
            (3, [0; 10]),
        ] {
            for (x, value) in expected.into_iter().enumerate() {
                let block = state.component(id).unwrap().block(x, 0).unwrap();

                assert_eq!(block[0], value, "component {id}, block {x}");
                assert!(block[1..].iter().all(|coefficient| *coefficient == 0));
            }
        }
    }
}
