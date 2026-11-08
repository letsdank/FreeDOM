//! Initial progressive AC scans.
//!
//! ITU-T T.81, sections G.1.1, G.1.2.2 and G.2.2:
//! <https://www.w3.org/Graphics/JPEG/itu-t81.pdf>
//!
//! Refinement scans are not decoded here.

use super::{
    BlockDecodeError, EntropyError, EntropyReader, HuffmanBuildError, HuffmanDecodeError,
    HuffmanDecoder, ProgressiveState, ScanHeader, ZIGZAG_TO_NATURAL, block::read_signed,
};

#[derive(Debug)]
pub enum InitialAcError {
    InvalidScan,
    UnknownComponent(u8),
    MissingDc(u8),
    AlreadyInitialized(u8),
    MissingAcTable(u8),
    InvalidEntropyRange,
    UnexpectedScanEnd,
    InvalidAcCategory(u8),
    RunExceedsBand,
    EobRunExceedsInterval,
    ValueOverflow,
    AllocationFailed,
    HuffmanBuild(HuffmanBuildError),
    HuffmanDecode(HuffmanDecodeError),
    Block(BlockDecodeError),
    Entropy(EntropyError),
}

impl std::fmt::Display for InitialAcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "progressive JPEG initial AC scan error: {self:?}")
    }
}

impl std::error::Error for InitialAcError {}

impl ProgressiveState {
    /// Decodes one initial AC scan.
    ///
    /// On error, coefficients and band state are unchanged.
    /// Entropy errors use offsets relative to entropy_offset.
    pub fn decode_initial_ac_scan(
        &mut self,
        input: &[u8],
        scan: &ScanHeader,
        entropy_end: usize,
        restart_interval: u16,
    ) -> Result<(), InitialAcError> {
        if scan.components.len() != 1
            || scan.spectral_start == 0
            || scan.spectral_start > scan.spectral_end
            || scan.spectral_end > 63
            || scan.approximation_high != 0
            || scan.approximation_low > 13
        {
            return Err(InitialAcError::InvalidScan);
        }

        let component = scan.components[0];

        let storage = self
            .component(component.id)
            .ok_or(InitialAcError::UnknownComponent(component.id))?;

        if !storage.dc_initialized {
            return Err(InitialAcError::MissingDc(component.id));
        }

        let start = usize::from(scan.spectral_start);
        let end = usize::from(scan.spectral_end);

        if storage.ac_bits[start..=end].iter().any(Option::is_some) {
            return Err(InitialAcError::AlreadyInitialized(component.id));
        }

        let table = self
            .huffman_tables()
            .ac
            .get(usize::from(component.ac_table))
            .and_then(Option::as_ref)
            .ok_or(InitialAcError::MissingAcTable(component.ac_table))?;

        let decoder = HuffmanDecoder::new(table).map_err(InitialAcError::HuffmanBuild)?;

        let entropy = input
            .get(scan.entropy_offset..)
            .ok_or(InitialAcError::InvalidEntropyRange)?;

        let expected_end = entropy_end
            .checked_sub(scan.entropy_offset)
            .ok_or(InitialAcError::InvalidEntropyRange)?;

        if input.get(entropy_end) != Some(&0xff) {
            return Err(InitialAcError::InvalidEntropyRange);
        }

        let (columns, rows) = storage.visible_dimensions();

        let count = columns
            .checked_mul(rows)
            .ok_or(InitialAcError::ValueOverflow)?;

        // keep the original blocks intact until the scan is fully validated.
        let mut staged = Vec::new();

        staged
            .try_reserve_exact(count)
            .map_err(|_| InitialAcError::AllocationFailed)?;

        let mut reader = EntropyReader::new(entropy);
        let mut eob_run = 0u32;
        let mut next_restart = 0u8;

        for index in 0..count {
            if restart_interval != 0 && index != 0 && index % usize::from(restart_interval) == 0 {
                if eob_run != 0 {
                    return Err(InitialAcError::EobRunExceedsInterval);
                }

                reader
                    .consume_restart_marker(0xd0 + next_restart)
                    .map_err(InitialAcError::Entropy)?;

                next_restart = (next_restart + 1) % 8;
            }

            let mut block = *storage.block(index % columns, index / columns).unwrap();

            if eob_run != 0 {
                eob_run -= 1;
            } else {
                let mut k = start;

                while k <= end {
                    let symbol = decoder
                        .decode(&mut reader)
                        .map_err(InitialAcError::HuffmanDecode)?;

                    let run = usize::from(symbol >> 4);
                    let size = symbol & 0x0f;

                    if size == 0 {
                        if run == 15 {
                            // ZRL represents sixteen zero coeffients.
                            k += 16;

                            if k > end + 1 {
                                return Err(InitialAcError::RunExceedsBand);
                            }

                            continue;
                        }

                        let extra = reader
                            .read_bits(run as u8)
                            .map_err(InitialAcError::Entropy)?;

                        // The current block is already part of the run.
                        eob_run = (1u32 << run) + u32::from(extra) - 1;

                        break;
                    }

                    if size > 10 {
                        return Err(InitialAcError::InvalidAcCategory(size));
                    }

                    k += run;

                    if k > end {
                        return Err(InitialAcError::RunExceedsBand);
                    }

                    let value = read_signed(&mut reader, size).map_err(InitialAcError::Block)?;

                    let scaled = i64::from(value) * (1i64 << scan.approximation_low);

                    block[ZIGZAG_TO_NATURAL[k]] =
                        i32::try_from(scaled).map_err(|_| InitialAcError::ValueOverflow)?;

                    k += 1;
                }
            }

            staged.push(block);
        }

        if eob_run != 0 {
            return Err(InitialAcError::EobRunExceedsInterval);
        }

        if reader.byte_position() != expected_end {
            return Err(InitialAcError::UnexpectedScanEnd);
        }

        let marker = reader
            .peek_scan_end_marker()
            .map_err(InitialAcError::Entropy)?;

        if (0xd0..=0xd7).contains(&marker) {
            return Err(InitialAcError::UnexpectedScanEnd);
        }

        let storage = self.component_mut(component.id).unwrap();

        for (index, block) in staged.into_iter().enumerate() {
            *storage.block_mut(index % columns, index / columns).unwrap() = block;
        }

        for bit in &mut storage.ac_bits[start..=end] {
            *bit = Some(scan.approximation_low);
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

    fn state(width: u16, symbols: &[u8]) -> ProgressiveState {
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

        let mut result = ProgressiveState::new(&frame, ProgressiveLimits::default()).unwrap();

        result.component_mut(1).unwrap().dc_initialized = true;

        let mut dht = vec![0x10, 1];

        dht.push(if symbols.len() == 2 { 1 } else { 0 });
        dht.extend([0; 14]);
        dht.extend_from_slice(symbols);

        result.update_huffman(&dht).unwrap();

        result
    }

    fn scan(start: u8, end: u8, low: u8) -> ScanHeader {
        ScanHeader {
            components: vec![ScanComponent {
                id: 1,
                dc_table: 0,
                ac_table: 0,
            }],

            spectral_start: start,
            spectral_end: end,
            approximation_high: 0,
            approximation_low: low,
            entropy_offset: 0,
        }
    }

    #[test]
    fn decodes_eob_run_across_blocks() {
        let mut state = state(24, &[0x10, 0]);

        state.component_mut(1).unwrap().block_mut(0, 0).unwrap()[0] = 23;

        state
            .decode_initial_ac_scan(&[0x2f, 0xff, 0xd9], &scan(1, 63, 0), 1, 0)
            .unwrap();

        assert_eq!(state.component(1).unwrap().block(0, 0).unwrap()[0], 23);

        assert!(matches!(
            state.decode_initial_ac_scan(&[0x2f, 0xff, 0xd9], &scan(1, 63, 0), 1, 0),
            Err(InitialAcError::AlreadyInitialized(1))
        ));
    }

    #[test]
    fn decodes_negative_ac_and_point_transform() {
        let mut state = state(8, &[1]);

        state
            .decode_initial_ac_scan(&[0x3f, 0xff, 0xd9], &scan(1, 1, 2), 1, 0)
            .unwrap();

        assert_eq!(state.component(1).unwrap().block(0, 0).unwrap()[1], -4);
    }

    #[test]
    fn validates_zero_runs_and_restart_boundaries_atomically() {
        let mut zrl = state(8, &[0xf0]);

        zrl.decode_initial_ac_scan(&[0x7f, 0xff, 0xd9], &scan(1, 16, 0), 1, 0)
            .unwrap();

        let mut too_short = state(8, &[0xf0]);

        assert!(matches!(
            too_short.decode_initial_ac_scan(&[0x7f, 0xff, 0xd9], &scan(1, 15, 0), 1, 0),
            Err(InitialAcError::RunExceedsBand)
        ));

        let data = [0x7f, 0xff, 0xd0, 0x7f, 0xff, 0xd1, 0x7f, 0xff, 0xd9];
        let mut restarts = state(24, &[0]);

        restarts
            .decode_initial_ac_scan(&data, &scan(1, 63, 0), 7, 1)
            .unwrap();

        let mut crossing = state(24, &[0x10]);

        let invalid = [0x3f, 0xff, 0xd0, 0x7f, 0xff, 0xd9];

        assert!(matches!(
            crossing.decode_initial_ac_scan(&invalid, &scan(1, 63, 0), 4, 1),
            Err(InitialAcError::EobRunExceedsInterval)
        ));

        assert!(
            crossing.component(1).unwrap().ac_bits[1..]
                .iter()
                .all(Option::is_none)
        );

        assert_eq!(
            crossing.component(1).unwrap().block(0, 0).unwrap(),
            &[0; 64]
        );
    }

    #[test]
    fn decodes_wpt_initial_ac_scans_against_libjpeg_coefficients() {
        // libjpeg jpeg_read_coefficients; WPT asset, not a WPT browser test.
        let input = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/images/computer.jpg"
        ));

        let frame = read_frame_header(input).unwrap();

        let mut state = ProgressiveState::new(&frame, ProgressiveLimits::default()).unwrap();
        let mut reader = JpegEventReader::new(input).unwrap();
        let mut scans = 0;

        while let Some(event) = reader.next_event().unwrap() {
            match event {
                JpegEvent::Segment {
                    marker: 0xc4, data, ..
                } => {
                    state.update_huffman(data).unwrap();
                }

                JpegEvent::Scan {
                    header,
                    entropy_end,
                } => {
                    if header.spectral_start == 0 {
                        state
                            .decode_initial_dc_scan(input, &header, entropy_end, 0)
                            .unwrap();
                    } else {
                        state
                            .decode_initial_ac_scan(input, &header, entropy_end, 0)
                            .unwrap();
                    }

                    scans += 1;
                }

                _ => {}
            }
        }

        assert_eq!(scans, 4);

        for (id, x, y, expected) in [
            (1, 9, 0, &[(0, 3), (8, -1), (16, 1), (24, -1)][..]),
            (1, 38, 0, &[(0, 6), (1, -1), (8, 2), (9, -1), (16, -1)][..]),
            (3, 10, 4, &[(0, 2), (16, -1)][..]),
        ] {
            let block = state.component(id).unwrap().block(x, y).unwrap();
            let mut reference = [0; 64];

            for &(index, value) in expected {
                reference[index] = value;
            }

            assert_eq!(*block, reference, "component {id}, block ({x},{y})");
        }

        assert!(
            state
                .component(2)
                .unwrap()
                .block(15, 7)
                .unwrap()
                .iter()
                .all(|&value| value == 0)
        );
    }
}
