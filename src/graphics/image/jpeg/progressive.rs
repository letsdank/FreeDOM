//! Persistent coefficient storage for progressive JPEG scans.
//!
//! ITU-T T.81, sections A.2 and G.1.1:
//! <https://www.w3.org/Graphics/JPEG/itu-t81.pdf>
//!
//! Storage uses natural DCT order, as in the baseline block decoder.
//! This module does not yet decode scans.

use super::{HuffmanTables, JpegError, JpegFrameHeader, JpegFrameKind};

#[derive(Clone, Copy, Debug)]
pub struct ProgressiveLimits {
    pub max_width: usize,
    pub max_height: usize,
    pub max_pixels: usize,
    pub max_coefficient_bytes: usize,
}

impl Default for ProgressiveLimits {
    fn default() -> Self {
        Self {
            max_width: 4096,
            max_height: 4096,
            max_pixels: 16 * 1024 * 1024,
            max_coefficient_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProgressiveError {
    UnsupportedFrame,
    UnsupportedPrecision(u8),
    InvalidFrame,
    LimitExceeded,
    SizeOverflow,
    AllocationFailed,
}

impl std::fmt::Display for ProgressiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "progressive JPEG error: {self:?}")
    }
}

impl std::error::Error for ProgressiveError {}

pub struct CoefficientComponent {
    id: u8,

    columns: usize,
    rows: usize,

    visible_columns: usize,
    visible_rows: usize,

    blocks: Vec<[i32; 64]>,

    pub(super) dc_initialized: bool,

    /// Initial approximation level by spectral (zigzag) index.
    pub(super) ac_bits: [Option<u8>; 64],
}

impl CoefficientComponent {
    pub fn id(&self) -> u8 {
        self.id
    }

    pub fn padded_dimensions(&self) -> (usize, usize) {
        (self.columns, self.rows)
    }

    pub fn visible_dimensions(&self) -> (usize, usize) {
        (self.visible_columns, self.visible_rows)
    }

    pub fn block(&self, x: usize, y: usize) -> Option<&[i32; 64]> {
        if x >= self.columns || y >= self.rows {
            return None;
        }

        self.blocks.get(y * self.columns + x)
    }

    pub fn block_mut(&mut self, x: usize, y: usize) -> Option<&mut [i32; 64]> {
        if x >= self.columns || y >= self.rows {
            return None;
        }

        self.blocks.get_mut(y * self.columns + x)
    }
}

pub struct ProgressiveState {
    components: Vec<CoefficientComponent>,
    huffman: HuffmanTables,
    pub(super) frame: JpegFrameHeader,
}

impl ProgressiveState {
    pub fn new(
        frame: &JpegFrameHeader,
        limits: ProgressiveLimits,
    ) -> Result<Self, ProgressiveError> {
        if frame.kind != JpegFrameKind::Progressive {
            return Err(ProgressiveError::UnsupportedFrame);
        }

        if frame.precision != 8 {
            return Err(ProgressiveError::UnsupportedPrecision(frame.precision));
        }

        let width = usize::from(frame.width);
        let height = usize::from(frame.height);

        if width == 0 || height == 0 || !(1..=4).contains(&frame.components.len()) {
            return Err(ProgressiveError::InvalidFrame);
        }

        let pixels = width
            .checked_mul(height)
            .ok_or(ProgressiveError::SizeOverflow)?;

        if width > limits.max_width || height > limits.max_height || pixels > limits.max_pixels {
            return Err(ProgressiveError::LimitExceeded);
        }

        for (index, component) in frame.components.iter().enumerate() {
            let duplicate_id = frame.components[..index]
                .iter()
                .any(|other| other.id == component.id);

            if !(1..=4).contains(&component.horizontal_sampling)
                || !(1..=4).contains(&component.vertical_sampling)
                || duplicate_id
            {
                return Err(ProgressiveError::InvalidFrame);
            }
        }

        let max_h = usize::from(
            frame
                .components
                .iter()
                .map(|component| component.horizontal_sampling)
                .max()
                .unwrap(),
        );

        let max_v = usize::from(
            frame
                .components
                .iter()
                .map(|component| component.vertical_sampling)
                .max()
                .unwrap(),
        );

        let mcu_columns = width.div_ceil(max_h * 8);
        let mcu_rows = height.div_ceil(max_v * 8);

        let mut layouts = Vec::with_capacity(frame.components.len());
        let mut total_bytes = 0usize;

        for component in &frame.components {
            let h = usize::from(component.horizontal_sampling);
            let v = usize::from(component.vertical_sampling);

            let columns = mcu_columns
                .checked_mul(h)
                .ok_or(ProgressiveError::SizeOverflow)?;

            let rows = mcu_rows
                .checked_mul(v)
                .ok_or(ProgressiveError::SizeOverflow)?;

            let visible_columns = width
                .checked_mul(h)
                .ok_or(ProgressiveError::SizeOverflow)?
                .div_ceil(max_h * 8);

            let visible_rows = height
                .checked_mul(v)
                .ok_or(ProgressiveError::SizeOverflow)?
                .div_ceil(max_v * 8);

            let block_count = columns
                .checked_mul(rows)
                .ok_or(ProgressiveError::SizeOverflow)?;

            let bytes = block_count
                .checked_mul(std::mem::size_of::<[i32; 64]>())
                .ok_or(ProgressiveError::SizeOverflow)?;

            total_bytes = total_bytes
                .checked_add(bytes)
                .ok_or(ProgressiveError::SizeOverflow)?;

            if total_bytes > limits.max_coefficient_bytes {
                return Err(ProgressiveError::LimitExceeded);
            }

            layouts.push((
                component.id,
                columns,
                rows,
                visible_columns,
                visible_rows,
                block_count,
            ));
        }

        let mut components = Vec::with_capacity(layouts.len());

        for (id, columns, rows, visible_columns, visible_rows, count) in layouts {
            let mut blocks = Vec::new();

            blocks
                .try_reserve_exact(count)
                .map_err(|_| ProgressiveError::AllocationFailed)?;

            blocks.resize(count, [0; 64]);

            components.push(CoefficientComponent {
                id,
                columns,
                rows,
                visible_columns,
                visible_rows,
                blocks,
                dc_initialized: false,
                ac_bits: [None; 64],
            });
        }

        Ok(Self {
            components,
            huffman: HuffmanTables::default(),
            frame: frame.clone(),
        })
    }

    pub fn component(&self, id: u8) -> Option<&CoefficientComponent> {
        self.components.iter().find(|component| component.id == id)
    }

    pub fn component_mut(&mut self, id: u8) -> Option<&mut CoefficientComponent> {
        self.components
            .iter_mut()
            .find(|component| component.id == id)
    }

    pub fn huffman_tables(&self) -> &HuffmanTables {
        &self.huffman
    }

    /// DHT errors use offsets relative to the payload.
    pub fn update_huffman(&mut self, data: &[u8]) -> Result<(), JpegError> {
        self.huffman.update_from_dht(data)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{JpegComponent, JpegEvent, JpegEventReader, read_frame_header};
    use super::*;

    fn frame() -> JpegFrameHeader {
        JpegFrameHeader {
            kind: JpegFrameKind::Progressive,
            width: 17,
            height: 9,
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
                    quantization_table: 1,
                },
                JpegComponent {
                    id: 3,
                    horizontal_sampling: 1,
                    vertical_sampling: 1,
                    quantization_table: 1,
                },
            ],
        }
    }

    fn dht(info: u8, symbol: u8) -> Vec<u8> {
        let mut result = vec![info, 1];

        result.extend([0; 15]);
        result.push(symbol);

        result
    }

    #[test]
    fn stores_visible_and_padded_coefficients_per_component() {
        let mut state = ProgressiveState::new(&frame(), ProgressiveLimits::default()).unwrap();

        let y = state.component(1).unwrap();

        assert_eq!(y.padded_dimensions(), (4, 2));
        assert_eq!(y.visible_dimensions(), (3, 2));
        assert_eq!(y.block(3, 1).unwrap(), &[0; 64]);

        assert!(y.block(4, 0).is_none());
        assert!(y.block(0, 2).is_none());

        assert_eq!(state.component(2).unwrap().visible_dimensions(), (2, 1));

        state.component_mut(1).unwrap().block_mut(3, 1).unwrap()[5] = -9;
        state.component_mut(2).unwrap().block_mut(0, 0).unwrap()[0] = 123;

        assert_eq!(state.component(1).unwrap().block(3, 1).unwrap()[5], -9);
        assert_eq!(state.component(2).unwrap().block(0, 0).unwrap()[0], 123);

        assert!(state.component_mut(9).is_none());
    }

    #[test]
    fn applies_dht_incrementally_and_atomically() {
        let mut state = ProgressiveState::new(&frame(), ProgressiveLimits::default()).unwrap();

        state.update_huffman(&dht(0, 3)).unwrap();
        state.update_huffman(&dht(0x10, 4)).unwrap();
        state.update_huffman(&dht(0, 8)).unwrap();

        assert_eq!(state.huffman_tables().dc[0].as_ref().unwrap().symbols, [8]);
        assert_eq!(state.huffman_tables().ac[0].as_ref().unwrap().symbols, [4]);

        let before = state.huffman_tables().clone();

        let mut partial = dht(0, 7);
        partial.push(0x10);

        assert!(state.update_huffman(&partial).is_err());
        assert_eq!(*state.huffman_tables(), before);
    }

    #[test]
    fn rejects_invalid_frames_and_memory_limits() {
        let mut invalid = frame();
        invalid.components[1].id = 1;

        assert!(matches!(
            ProgressiveState::new(&invalid, ProgressiveLimits::default()),
            Err(ProgressiveError::InvalidFrame)
        ));

        invalid = frame();
        invalid.components[0].horizontal_sampling = 0;

        assert!(matches!(
            ProgressiveState::new(&invalid, ProgressiveLimits::default()),
            Err(ProgressiveError::InvalidFrame)
        ));

        let mut limits = ProgressiveLimits::default();

        // (8 + 2 + 2) blocks * 256 bytes = 3072 bytes.
        limits.max_coefficient_bytes = 2559;

        assert!(matches!(
            ProgressiveState::new(&frame(), limits),
            Err(ProgressiveError::LimitExceeded)
        ));

        limits.max_coefficient_bytes = 3072;

        assert!(ProgressiveState::new(&frame(), limits).is_ok());
    }

    #[test]
    fn updates_tables_in_order_from_progressive_wpt_asset() {
        // The file comes from WPT; this is not a WPT browser test.
        let data = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/images/computer.jpg"
        ));

        let frame = read_frame_header(data).unwrap();
        let mut state = ProgressiveState::new(&frame, ProgressiveLimits::default()).unwrap();
        let mut reader = JpegEventReader::new(data).unwrap();

        let mut scans = 0;
        let mut tables = 0;

        while let Some(event) = reader.next_event().unwrap() {
            match event {
                JpegEvent::Segment {
                    marker: 0xc4, data, ..
                } => {
                    state.update_huffman(data).unwrap();
                    tables += 1;
                }

                JpegEvent::Scan { header, .. } => {
                    for component in &header.components {
                        assert!(state.component(component.id).is_some());

                        let huffman = state.huffman_tables();

                        if header.spectral_start == 0 {
                            assert!(huffman.dc[usize::from(component.dc_table)].is_some());
                        } else {
                            assert!(huffman.ac[usize::from(component.ac_table)].is_some());
                        }
                    }

                    scans += 1;
                }

                _ => {}
            }
        }

        assert_eq!((scans, tables), (4, 4));
    }
}
