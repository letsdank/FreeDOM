//! Reconstruction of simple progressive JPEG images.
//!
//! ITU-T T.81, sections A.2, A.3 and G.1:
//! <https://www.w3.org/Graphics/JPEG/itu-t81.pdf>
//!
//! Only initial full-precision scans are supported. Quantization tables
//! cannot be redefined after scan data begins in this implementation.

use super::{
    ComponentPlane, DecodedPlanes, DequantizeError, InitialAcError, InitialDcError, JpegError,
    JpegEvent, JpegEventReader, PlaneLimits, ProgressiveError, ProgressiveLimits, ProgressiveState,
    QuantizationTable, dequantize, idct_8bit, parse_dqt_segment, read_frame_header,
};

#[derive(Debug)]
pub enum ProgressiveImageError {
    Header(JpegError),
    State(ProgressiveError),
    Dc(InitialDcError),
    Ac(InitialAcError),
    Dequantize(DequantizeError),

    UnsupportedScan,
    TooManyScans,
    UnsupportedMarker(u8),
    LateQuantization,
    InvalidRestartInterval,
    MissingQuantization(u8),
    MissingCoefficients(u8),

    LimitExceeded,
    SizeOverflow,
    AllocationFailed,
}

impl std::fmt::Display for ProgressiveImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "progressive JPEG decode error: {self:?}")
    }
}

impl std::error::Error for ProgressiveImageError {}

/// Decodes only full-precision initial scans into 8-bit component planes.
pub fn decode_progressive_planes(
    input: &[u8],
    limits: PlaneLimits,
) -> Result<DecodedPlanes, ProgressiveImageError> {
    let frame = read_frame_header(input).map_err(ProgressiveImageError::Header)?;

    let progressive_limits = ProgressiveLimits {
        max_width: limits.max_width,
        max_height: limits.max_height,
        max_pixels: limits.max_pixels,
        ..ProgressiveLimits::default()
    };

    let mut state =
        ProgressiveState::new(&frame, progressive_limits).map_err(ProgressiveImageError::State)?;

    let mut reader = JpegEventReader::new(input).map_err(ProgressiveImageError::Header)?;
    let mut quantization: [Option<QuantizationTable>; 4] = [None, None, None, None];

    let mut restart_interval = 0u16;
    let mut scan_count = 0usize;
    let mut saw_frame = false;
    let mut saw_end = false;

    while let Some(event) = reader.next_event().map_err(ProgressiveImageError::Header)? {
        match event {
            JpegEvent::Segment { marker: 0xc2, .. } if !saw_frame && scan_count == 0 => {
                saw_frame = true;
            }

            JpegEvent::Segment {
                marker: 0xdb,
                data,
                data_offset,
                ..
            } => {
                if scan_count != 0 {
                    return Err(ProgressiveImageError::LateQuantization);
                }

                let parsed = parse_dqt_segment(data).map_err(|mut error| {
                    error.offset += data_offset;
                    ProgressiveImageError::Header(error)
                })?;

                for (destination, table) in quantization.iter_mut().zip(parsed) {
                    if table.is_some() {
                        *destination = table;
                    }
                }
            }

            JpegEvent::Segment {
                marker: 0xc4,
                data,
                data_offset,
                ..
            } => {
                state.update_huffman(data).map_err(|mut error| {
                    error.offset += data_offset;
                    ProgressiveImageError::Header(error)
                })?;
            }

            JpegEvent::Segment {
                marker: 0xdd, data, ..
            } => {
                let [high, low] = data else {
                    return Err(ProgressiveImageError::InvalidRestartInterval);
                };

                restart_interval = u16::from_be_bytes([*high, *low]);
            }

            JpegEvent::Segment {
                marker: 0xe0..=0xef | 0xfe,
                ..
            } => {}

            JpegEvent::Segment { marker, .. } => {
                return Err(ProgressiveImageError::UnsupportedMarker(marker));
            }

            JpegEvent::Scan {
                header,
                entropy_end,
            } => {
                if !saw_frame || header.approximation_high != 0 || header.approximation_low != 0 {
                    return Err(ProgressiveImageError::UnsupportedScan);
                }

                scan_count += 1;

                if scan_count > 256 {
                    return Err(ProgressiveImageError::TooManyScans);
                }

                if header.spectral_start == 0 {
                    state
                        .decode_initial_dc_scan(input, &header, entropy_end, restart_interval)
                        .map_err(ProgressiveImageError::Dc)?;
                } else {
                    state
                        .decode_initial_ac_scan(input, &header, entropy_end, restart_interval)
                        .map_err(ProgressiveImageError::Ac)?;
                }
            }

            JpegEvent::End { .. } => {
                saw_end = true;
                break;
            }
        }
    }

    if !saw_end || scan_count == 0 {
        return Err(ProgressiveImageError::UnsupportedScan);
    }

    let width = usize::from(frame.width);
    let height = usize::from(frame.height);

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

    let mut total_samples = 0usize;
    let mut planes = Vec::with_capacity(frame.components.len());

    for component in &frame.components {
        let storage = state.component(component.id).unwrap();

        if !storage.dc_initialized || storage.ac_bits[1..].iter().any(|value| *value != Some(0)) {
            return Err(ProgressiveImageError::MissingCoefficients(component.id));
        }

        let table = quantization
            .get(usize::from(component.quantization_table))
            .and_then(Option::as_ref)
            .ok_or(ProgressiveImageError::MissingQuantization(
                component.quantization_table,
            ))?;

        let plane_width = width
            .checked_mul(usize::from(component.horizontal_sampling))
            .ok_or(ProgressiveImageError::SizeOverflow)?
            .div_ceil(max_h);

        let plane_height = height
            .checked_mul(usize::from(component.vertical_sampling))
            .ok_or(ProgressiveImageError::SizeOverflow)?
            .div_ceil(max_v);

        let size = plane_width
            .checked_mul(plane_height)
            .ok_or(ProgressiveImageError::SizeOverflow)?;

        total_samples = total_samples
            .checked_add(size)
            .ok_or(ProgressiveImageError::SizeOverflow)?;

        if total_samples > limits.max_samples {
            return Err(ProgressiveImageError::LimitExceeded);
        }

        let mut samples = Vec::new();

        samples
            .try_reserve_exact(size)
            .map_err(|_| ProgressiveImageError::AllocationFailed)?;

        samples.resize(size, 0);

        let (columns, rows) = storage.visible_dimensions();

        for block_y in 0..rows {
            for block_x in 0..columns {
                let block = storage.block(block_x, block_y).unwrap();

                let coefficients =
                    dequantize(block, table).map_err(ProgressiveImageError::Dequantize)?;

                let decoded = idct_8bit(&coefficients);

                for y in 0..8 {
                    let pixel_y = block_y * 8 + y;

                    if pixel_y >= plane_height {
                        break;
                    }

                    for x in 0..8 {
                        let pixel_x = block_x * 8 + x;

                        if pixel_x >= plane_width {
                            break;
                        }

                        samples[pixel_y * plane_width + pixel_x] = decoded[y * 8 + x];
                    }
                }
            }
        }

        planes.push(ComponentPlane {
            id: component.id,
            horizontal_sampling: component.horizontal_sampling,
            vertical_sampling: component.vertical_sampling,
            width: plane_width,
            height: plane_height,
            samples,
        });
    }

    Ok(DecodedPlanes {
        width,
        height,
        components: planes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_wpt_progressive_image_and_detects_truncation() {
        let input = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/images/computer.jpg"
        ));

        let planes = decode_progressive_planes(input, PlaneLimits::default()).unwrap();

        assert_eq!(
            (planes.width, planes.height, planes.components.len()),
            (320, 240, 3)
        );

        assert!(
            decode_progressive_planes(&input[..input.len() - 2], PlaneLimits::default()).is_err()
        );

        let mut limits = PlaneLimits::default();
        limits.max_samples = 1;

        assert!(matches!(
            decode_progressive_planes(input, limits),
            Err(ProgressiveImageError::LimitExceeded)
        ));
    }

    #[test]
    fn compares_all_pixels_with_libjpeg() {
        // The JPEG comes from WPT; this is not a WPT browser test.
        let input = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/images/computer.jpg"
        ));

        let expected = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/jpeg/computer.rgb"
        ));

        let actual = super::super::decode_rgb8(input, PlaneLimits::default()).unwrap();

        assert_eq!(actual.pixels.len(), expected.len());

        let mut max_difference = 0u8;

        for (&actual, &reference) in actual.pixels.iter().zip(expected.iter()) {
            max_difference = max_difference.max(actual.abs_diff(reference));
        }

        eprintln!(
            "Progressive JPEG: {} RGB channels, max difference {}",
            expected.len(),
            max_difference
        );

        assert!(max_difference <= 4);
    }

    #[test]
    fn rejects_incomplete_and_unsupported_scans() {
        let input = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/images/computer.jpg"
        ));

        let first = super::super::read_first_scan_header(input).unwrap();

        let mut unsupported = input.to_vec();

        // Change Al of the first scan to an unsupported value.
        unsupported[first.entropy_offset - 1] = 1;

        assert!(matches!(
            decode_progressive_planes(&unsupported, PlaneLimits::default()),
            Err(ProgressiveImageError::UnsupportedScan)
        ));

        let mut events = JpegEventReader::new(input).unwrap();
        let mut seen = 0;
        let mut end = None;

        while let Some(event) = events.next_event().unwrap() {
            if let JpegEvent::Scan { entropy_end, .. } = event {
                seen += 1;

                if seen == 3 {
                    end = Some(entropy_end);
                    break;
                }
            }
        }

        // End the file before the fourth component scan.
        let mut truncated = input[..end.unwrap()].to_vec();
        truncated.extend_from_slice(&[0xff, 0xd9]);

        assert!(matches!(
            decode_progressive_planes(&truncated, PlaneLimits::default()),
            Err(ProgressiveImageError::MissingCoefficients(_))
        ));
    }
}
