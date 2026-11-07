//! Component planes for a single-scan baseline JPEG.
//!
//! ITU-T T.81, sections A.1 and A.2:
//! <https://www.w3.org/Graphics/JPEG/itu-t81.pdf>
//!
//! This rejects scans that omit frame components.
//! Samples are not upsampled, color-converted, or EXIF-oriented.

use super::{
    BaselineMcuDecoder, EntropyError, EntropyReader, HeaderSegments, JpegError, McuError,
    read_first_scan_header, read_frame_header, read_huffman_tables, read_quantization_tables,
};

#[derive(Clone, Copy, Debug)]
pub struct PlaneLimits {
    pub max_width: usize,
    pub max_height: usize,
    pub max_pixels: usize,
    pub max_samples: usize,
}

impl Default for PlaneLimits {
    fn default() -> Self {
        Self {
            max_width: 4096,
            max_height: 4096,
            max_pixels: 16 * 1024 * 1024,
            max_samples: 48 * 1024 * 1024,
        }
    }
}

#[derive(Debug)]
pub enum PlaneDecodeError {
    Header(JpegError),
    Mcu(McuError),
    Entropy(EntropyError),
    UnsupportedMultiScan,
    InvalidRestartInterval { offset: usize },
    UnsupportedScanEnd(u8),
    LimitExceeded,
    SizeOverflow,
    AllocationFailed,
}

impl std::fmt::Display for PlaneDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JPEG plane decode error: {self:?}")
    }
}

impl std::error::Error for PlaneDecodeError {}

#[derive(Debug)]
pub struct ComponentPlane {
    pub id: u8,
    pub horizontal_sampling: u8,
    pub vertical_sampling: u8,
    pub width: usize,
    pub height: usize,
    pub samples: Vec<u8>,
}

#[derive(Debug)]
pub struct DecodedPlanes {
    pub width: usize,
    pub height: usize,

    /// Component order is the frame order; samples are row-major.
    pub components: Vec<ComponentPlane>,
}

fn restart_interval(input: &[u8]) -> Result<u16, PlaneDecodeError> {
    let mut reader = HeaderSegments::new(input).map_err(PlaneDecodeError::Header)?;

    let mut interval = 0;

    while let Some(segment) = reader.next_segment().map_err(PlaneDecodeError::Header)? {
        if segment.marker == 0xdd {
            let [high, low] = segment.data else {
                return Err(PlaneDecodeError::InvalidRestartInterval {
                    offset: segment.marker_offset,
                });
            };

            interval = u16::from_be_bytes([*high, *low]);
        }
    }

    Ok(interval)
}

/// Decodes one complete baseline scan containing every frame component.
///
/// Only EOI is accepted after the scan. Input size is not limited here.
pub fn decode_baseline_planes(
    input: &[u8],
    limits: PlaneLimits,
) -> Result<DecodedPlanes, PlaneDecodeError> {
    let frame = read_frame_header(input).map_err(PlaneDecodeError::Header)?;
    let scan = read_first_scan_header(input).map_err(PlaneDecodeError::Header)?;

    let width = usize::from(frame.width);
    let height = usize::from(frame.height);

    let pixels = width
        .checked_mul(height)
        .ok_or(PlaneDecodeError::SizeOverflow)?;

    if width > limits.max_width || height > limits.max_height || pixels > limits.max_pixels {
        return Err(PlaneDecodeError::LimitExceeded);
    }

    if frame.components.len() != scan.components.len() {
        return Err(PlaneDecodeError::UnsupportedMultiScan);
    }

    let interval = restart_interval(input)?;
    let huffman = read_huffman_tables(input).map_err(PlaneDecodeError::Header)?;
    let quantization = read_quantization_tables(input).map_err(PlaneDecodeError::Header)?;

    let mut decoder = BaselineMcuDecoder::new(&frame, &scan, &huffman, &quantization)
        .map_err(PlaneDecodeError::Mcu)?;

    let max_h = usize::from(
        frame
            .components
            .iter()
            .map(|component| component.horizontal_sampling)
            .max()
            .unwrap_or(1),
    );

    let max_v = usize::from(
        frame
            .components
            .iter()
            .map(|component| component.vertical_sampling)
            .max()
            .unwrap_or(1),
    );

    let interleaved = scan.components.len() > 1;

    let mcu_width = if interleaved { max_h * 8 } else { 8 };
    let mcu_height = if interleaved { max_v * 8 } else { 8 };

    let columns = width.div_ceil(mcu_width);
    let rows = height.div_ceil(mcu_height);

    let mut components = Vec::with_capacity(frame.components.len());
    let mut total_samples = 0usize;

    for component in &frame.components {
        let h = usize::from(component.horizontal_sampling);
        let v = usize::from(component.vertical_sampling);

        let plane_width = width
            .checked_mul(h)
            .ok_or(PlaneDecodeError::SizeOverflow)?
            .div_ceil(max_h);

        let plane_height = height
            .checked_mul(v)
            .ok_or(PlaneDecodeError::SizeOverflow)?
            .div_ceil(max_v);

        let len = plane_width
            .checked_mul(plane_height)
            .ok_or(PlaneDecodeError::SizeOverflow)?;

        total_samples = total_samples
            .checked_add(len)
            .ok_or(PlaneDecodeError::SizeOverflow)?;

        if total_samples > limits.max_samples {
            return Err(PlaneDecodeError::LimitExceeded);
        }

        let mut samples = Vec::new();

        samples
            .try_reserve_exact(len)
            .map_err(|_| PlaneDecodeError::AllocationFailed)?;

        samples.resize(len, 0);

        components.push(ComponentPlane {
            id: component.id,
            horizontal_sampling: component.horizontal_sampling,
            vertical_sampling: component.vertical_sampling,
            width: plane_width,
            height: plane_height,
            samples,
        });
    }

    let mut reader = EntropyReader::new(&input[scan.entropy_offset..]);

    let mut completed_mcus = 0usize;
    let mut next_restart = 0u8;

    for mcu_y in 0..rows {
        for mcu_x in 0..columns {
            // Restart markers are between intervals, never after the final MCU.
            if interval != 0 && completed_mcus != 0 && completed_mcus % usize::from(interval) == 0 {
                reader
                    .consume_restart_marker(0xd0 + next_restart)
                    .map_err(PlaneDecodeError::Entropy)?;

                decoder.reset_predictors();

                next_restart = (next_restart + 1) % 8;
            }

            let mcu = decoder
                .decode_next(&mut reader)
                .map_err(PlaneDecodeError::Mcu)?;

            completed_mcus += 1;

            for (plane, component) in components.iter_mut().zip(mcu.components) {
                let columns = usize::from(component.block_columns);
                let block_rows = usize::from(component.block_rows);

                for (index, block) in component.blocks.iter().enumerate() {
                    let base_x = (mcu_x * columns + index % columns) * 8;
                    let base_y = (mcu_y * block_rows + index / columns) * 8;

                    for y in 0..8 {
                        if base_y + y >= plane.height {
                            break;
                        }

                        for x in 0..8 {
                            if base_x + x >= plane.width {
                                break;
                            }

                            let destination = (base_y + y) * plane.width + base_x + x;
                            plane.samples[destination] = block[y * 8 + x];
                        }
                    }
                }
            }
        }
    }

    let marker = reader
        .peek_scan_end_marker()
        .map_err(PlaneDecodeError::Entropy)?;

    if marker != 0xd9 {
        return Err(PlaneDecodeError::UnsupportedScanEnd(marker));
    }

    Ok(DecodedPlanes {
        width,
        height,
        components,
    })
}

#[cfg(test)]
mod tests {
    use super::super::EntropyErrorKind;
    use super::*;

    fn segment(output: &mut Vec<u8>, marker: u8, data: &[u8]) {
        output.extend_from_slice(&[0xff, marker]);
        output.extend_from_slice(&((data.len() + 2) as u16).to_be_bytes());
        output.extend_from_slice(data);
    }

    fn synthetic(all_components: bool, interval: Option<u16>) -> Vec<u8> {
        let mut bytes = vec![0xff, 0xd8];

        let mut dqt = vec![0];
        dqt.extend([8; 64]);

        segment(&mut bytes, 0xdb, &dqt);

        // 17x9 image with 4:2:0 sampling.
        segment(
            &mut bytes,
            0xc0,
            &[8, 0, 9, 0, 17, 3, 1, 0x22, 0, 2, 0x11, 0, 3, 0x11, 0],
        );

        let mut dht = vec![0, 1];
        dht.extend([0; 15]);
        dht.push(1);

        dht.extend([0x10, 1]);
        dht.extend([0; 15]);
        dht.push(0);

        segment(&mut bytes, 0xc4, &dht);

        if let Some(interval) = interval {
            segment(&mut bytes, 0xdd, &interval.to_be_bytes());
        }

        if all_components {
            segment(&mut bytes, 0xda, &[3, 1, 0, 2, 0, 3, 0, 0, 63, 0]);
        } else {
            segment(&mut bytes, 0xda, &[1, 1, 0, 0, 63, 0]);
        }

        let bits = "010".repeat(12);

        for chunk in bits.as_bytes().chunks(8) {
            let padding = 8 - chunk.len();

            let value = chunk
                .iter()
                .fold(0u8, |value, bit| (value << 1) | (bit - b'0'))
                << padding;

            let value = value | ((1u16 << padding) - 1) as u8;

            bytes.push(value);

            if value == 0xff {
                bytes.push(0);
            }
        }

        bytes.extend([0xff, 0xd9]);

        bytes
    }

    fn restart_image(mcus: u8) -> Vec<u8> {
        let mut bytes = synthetic(true, Some(1));

        let frame = bytes
            .windows(2)
            .position(|pair| pair == [0xff, 0xc0])
            .unwrap();

        let width = u16::from(mcus) * 16 - 15;

        bytes[frame + 7..frame + 9].copy_from_slice(&width.to_be_bytes());

        let scan = read_first_scan_header(&bytes).unwrap();

        bytes.truncate(scan.entropy_offset);

        for index in 0..mcus {
            // Six blocks: DC difference +1 and AC end-of-block.
            bytes.extend_from_slice(&[0x49, 0x24, 0xbf]);

            if index + 1 != mcus {
                bytes.extend_from_slice(&[0xff, 0xd0 + index % 8]);
            }
        }

        bytes.extend_from_slice(&[0xff, 0xd9]);

        bytes
    }

    #[test]
    fn decodes_restart_intervals_and_resets_all_predictors() {
        let data = restart_image(10);

        let output = decode_baseline_planes(&data, PlaneLimits::default()).unwrap();

        assert_eq!((output.width, output.height), (145, 9));

        for index in 0..10 {
            let x = index * 16;

            assert_eq!(output.components[0].samples[x], 129);
            assert_eq!(output.components[1].samples[x / 2], 129);
            assert_eq!(output.components[2].samples[x / 2], 129);
        }

        // A final interval can end exactly at EOI without a restart marker.
        assert!(decode_baseline_planes(&synthetic(true, Some(2)), PlaneLimits::default()).is_ok());
        assert!(decode_baseline_planes(&synthetic(true, Some(0)), PlaneLimits::default()).is_ok());
    }

    #[test]
    fn decodes_restart_interval_of_two_mcus() {
        let mut data = synthetic(true, Some(2));

        let frame = data
            .windows(2)
            .position(|pair| pair == [0xff, 0xc0])
            .unwrap();

        data[frame + 7..frame + 9].copy_from_slice(&49u16.to_be_bytes());

        let scan = read_first_scan_header(&data).unwrap().entropy_offset;
        let interval = data[scan..data.len() - 2].to_vec();

        data.truncate(scan);

        data.extend_from_slice(&interval);
        data.extend_from_slice(&[0xff, 0xd0]);
        data.extend_from_slice(&interval);
        data.extend_from_slice(&[0xff, 0xd9]);

        let output = decode_baseline_planes(&data, PlaneLimits::default()).unwrap();
        let y = &output.components[0].samples;

        assert_eq!([y[0], y[16], y[32], y[48]], [129, 133, 129, 133]);

        let cb = &output.components[1].samples;

        assert_eq!([cb[0], cb[8], cb[16], cb[24]], [129, 130, 129, 130]);
    }

    #[test]
    fn rejects_bad_restart_markers_and_padding() {
        let data = restart_image(2);
        let scan = read_first_scan_header(&data).unwrap().entropy_offset;

        let mut wrong = data.clone();
        wrong[scan + 4] = 0xd1;

        assert!(matches!(
            decode_baseline_planes(&wrong, PlaneLimits::default()),
            Err(PlaneDecodeError::Entropy(EntropyError {
                kind: EntropyErrorKind::UnexpectedRestartMarker {
                    expected: 0xd0,
                    found: 0xd1,
                },
                ..
            }))
        ));

        let mut invalid_padding = data.clone();
        invalid_padding[scan + 2] &= !1;

        assert!(matches!(
            decode_baseline_planes(&invalid_padding, PlaneLimits::default()),
            Err(PlaneDecodeError::Entropy(EntropyError {
                kind: EntropyErrorKind::InvalidPadding,
                ..
            }))
        ));

        let mut missing = data;
        missing.drain(scan + 3..scan + 5);

        assert!(matches!(
            decode_baseline_planes(&missing, PlaneLimits::default()),
            Err(PlaneDecodeError::Entropy(EntropyError {
                kind: EntropyErrorKind::ExpectedMarker,
                ..
            }))
        ));
    }

    #[test]
    fn reconstructs_420_planes_and_clips_partial_mcus() {
        let output =
            decode_baseline_planes(&synthetic(true, None), PlaneLimits::default()).unwrap();

        assert_eq!((output.width, output.height), (17, 9));

        let y = &output.components[0];
        let cb = &output.components[1];
        let cr = &output.components[2];

        assert_eq!((y.width, y.height), (17, 9));
        assert_eq!((cb.width, cb.height), (9, 5));
        assert_eq!((cr.width, cr.height), (9, 5));

        let sample = |plane: &ComponentPlane, x, y| plane.samples[y * plane.width + x];

        assert_eq!(
            [
                sample(y, 0, 0),
                sample(y, 8, 0),
                sample(y, 0, 8),
                sample(y, 8, 8),
                sample(y, 16, 0),
                sample(y, 16, 8)
            ],
            [129, 130, 131, 132, 133, 135],
        );

        assert_eq!(
            [
                sample(cb, 0, 0),
                sample(cb, 8, 0),
                sample(cr, 0, 0),
                sample(cr, 8, 0)
            ],
            [129, 130, 129, 130]
        );
    }

    #[test]
    fn decodes_baseline_wpt_planes_and_matches_libjpeg_samples() {
        // Reference samples were checked with libjpeg.
        // This tests a WPT asset, not the original browser test.
        let bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/images/arrow-oriented-upright.jpg"
        ));

        let output = decode_baseline_planes(bytes, PlaneLimits::default()).unwrap();

        assert_eq!(
            (output.width, output.height, output.components.len()),
            (240, 144, 3)
        );

        let positions = [(0, 0), (50, 50), (120, 72), (239, 143), (140, 70)];
        let expected = [[0u8, 0, 255, 0, 0], [128; 5], [128; 5]];

        for (plane, reference) in output.components.iter().zip(expected) {
            assert_eq!((plane.width, plane.height), (240, 144));

            for ((x, y), value) in positions.into_iter().zip(reference) {
                let actual = plane.samples[y * plane.width + x];

                assert!(
                    actual.abs_diff(value) <= 1,
                    "component {} at ({x},{y}): {actual} vs {value}",
                    plane.id
                );
            }
        }
    }

    #[test]
    fn rejects_limits_multiscan_and_bad_end() {
        let data = synthetic(true, None);

        let mut limits = PlaneLimits::default();
        limits.max_width = 16;

        assert!(matches!(
            decode_baseline_planes(&data, limits),
            Err(PlaneDecodeError::LimitExceeded)
        ));

        let mut limits = PlaneLimits::default();
        limits.max_samples = 1;

        assert!(matches!(
            decode_baseline_planes(&data, limits),
            Err(PlaneDecodeError::LimitExceeded)
        ));

        // An interval greater than the scan contains no restart marker.
        assert!(decode_baseline_planes(&synthetic(true, Some(3)), PlaneLimits::default()).is_ok());

        assert!(matches!(
            decode_baseline_planes(&synthetic(false, None), PlaneLimits::default()),
            Err(PlaneDecodeError::UnsupportedMultiScan)
        ));

        let mut bad = data.clone();
        let last_entropy = bad.len() - 3;
        bad[last_entropy] &= !1;

        assert!(matches!(
            decode_baseline_planes(&bad, PlaneLimits::default()),
            Err(PlaneDecodeError::Entropy(EntropyError {
                kind: EntropyErrorKind::InvalidPadding,
                ..
            }))
        ));

        let mut bad = data.clone();
        bad.pop();
        bad.pop();

        assert!(matches!(
            decode_baseline_planes(&bad, PlaneLimits::default()),
            Err(PlaneDecodeError::Entropy(EntropyError {
                kind: EntropyErrorKind::Truncated,
                ..
            }))
        ));

        let mut bad = data;
        *bad.last_mut().unwrap() = 0xd0;

        assert!(matches!(
            decode_baseline_planes(&bad, PlaneLimits::default()),
            Err(PlaneDecodeError::UnsupportedScanEnd(0xd0))
        ));
    }

    #[test]
    fn decodes_restart_jpeg_against_libjpeg_rgb_reference() {
        // The JPEG was generated from the WPT computer.jpg asset using
        // ImageMagick/libjpeg with a restart interval of two MCUs.
        //
        // The reference contains RGB8 pixels decoded by ImageMagick/libjpeg.
        // This is a codec test, not an official WPT browser test.
        let jpeg = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/jpeg/restart-2.jpg"
        ));

        let reference = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/jpeg/restart-2.rgb"
        ));

        assert_eq!(restart_interval(jpeg).unwrap(), 2);

        let frame = read_frame_header(jpeg).unwrap();

        assert_eq!((frame.width, frame.height), (320, 240));

        // The fixture uses 4:4:4 sampling, avoiding differences between
        // chroma upsampling implementations in this comparison.
        assert!(frame.components.iter().all(|component| {
            component.horizontal_sampling == 1 && component.vertical_sampling == 1
        }));

        let scan = read_first_scan_header(jpeg).unwrap();

        let markers: Vec<u8> = jpeg[scan.entropy_offset..]
            .windows(2)
            .filter_map(|pair| {
                (pair[0] == 0xff && (0xd0..=0xd7).contains(&pair[1])).then_some(pair[1])
            })
            .collect();

        assert_eq!(markers.len(), 599);

        for (index, marker) in markers.iter().copied().enumerate() {
            assert_eq!(marker, 0xd0 + (index % 8) as u8, "restart marker {index}");
        }

        let image = super::super::decode_baseline_rgb8(jpeg, PlaneLimits::default()).unwrap();

        assert_eq!((image.width, image.height), (320, 240));
        assert_eq!(image.pixels.len(), reference.len());

        let mut max_difference = 0u8;

        for (index, (&actual, &expected)) in image.pixels.iter().zip(reference.iter()).enumerate() {
            let difference = actual.abs_diff(expected);

            max_difference = max_difference.max(difference);

            // IDCT implementations may round samples differently.
            assert!(
                difference <= 4,
                "channel {index}: expected {expected}, got {actual}, \
                 difference {difference}"
            );
        }

        eprintln!(
            "JPEG restart fixture: {} RGB channels, max difference {}",
            reference.len(),
            max_difference
        );
    }
}
