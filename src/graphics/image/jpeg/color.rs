//! Conversion of baseline JPEG component planes to RGB.
//!
//! JFIF 1.02, "Conversion to and from RGB":
//! <https://www.w3.org/Graphics/JPEG/jfif.pdf>
//!
//! Chroma is upsampled using nearest-neighbor sampling. EXIF orientation and
//! embedded color profiles are not applied.

use super::{
    DecodedPlanes, HeaderSegments, JpegError, PlaneDecodeError, PlaneLimits, decode_baseline_planes,
};

#[derive(Debug)]
pub struct Rgb8Image {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

#[derive(Debug)]
pub enum ColorDecodeError {
    Header(JpegError),
    Planes(PlaneDecodeError),
    UnsupportedColorModel,
    InvalidColorMetadata,
    SizeOverflow,
    AllocationFailed,
}

impl std::fmt::Display for ColorDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JPEG color decode error: {self:?}")
    }
}

impl std::error::Error for ColorDecodeError {}

/// Decodes grayscale or explicitly identified JFIF/Adobe YCbCr JPEG.
///
/// The underlying plane decoder defines the supported JPEG coding processes.
pub fn decode_baseline_rgb8(
    input: &[u8],
    limits: PlaneLimits,
) -> Result<Rgb8Image, ColorDecodeError> {
    let planes = decode_baseline_planes(input, limits).map_err(ColorDecodeError::Planes)?;

    let model = color_model(input, &planes)?;

    convert_planes(&planes, model)
}

#[derive(Clone, Copy)]
enum ColorModel {
    Grayscale,
    Ycbcr,
}

fn color_model(input: &[u8], planes: &DecodedPlanes) -> Result<ColorModel, ColorDecodeError> {
    if planes.components.len() == 1 {
        return Ok(ColorModel::Grayscale);
    }

    if planes.components.len() != 3 {
        return Err(ColorDecodeError::UnsupportedColorModel);
    }

    let mut segments = HeaderSegments::new(input).map_err(ColorDecodeError::Header)?;

    let mut jfif = false;
    let mut adobe_transform = None;

    while let Some(segment) = segments.next_segment().map_err(ColorDecodeError::Header)? {
        if segment.marker == 0xe0 && segment.data.starts_with(b"JFIF\0") {
            if segment.data.len() < 14 {
                return Err(ColorDecodeError::InvalidColorMetadata);
            }

            jfif = true;
        }

        if segment.marker == 0xee && segment.data.starts_with(b"Adobe") {
            let transform = *segment
                .data
                .get(11)
                .ok_or(ColorDecodeError::InvalidColorMetadata)?;

            if adobe_transform.is_some_and(|previous| previous != transform) {
                return Err(ColorDecodeError::InvalidColorMetadata);
            }

            adobe_transform = Some(transform);
        }
    }

    if adobe_transform.is_some_and(|transform| transform != 1) {
        return Err(ColorDecodeError::UnsupportedColorModel);
    }

    if !jfif && adobe_transform != Some(1) {
        return Err(ColorDecodeError::UnsupportedColorModel);
    }

    if jfif && planes.components.iter().map(|plane| plane.id).ne([1, 2, 3]) {
        return Err(ColorDecodeError::UnsupportedColorModel);
    }

    Ok(ColorModel::Ycbcr)
}

fn convert_planes(
    planes: &DecodedPlanes,
    model: ColorModel,
) -> Result<Rgb8Image, ColorDecodeError> {
    let length = planes
        .width
        .checked_mul(planes.height)
        .and_then(|count| count.checked_mul(3))
        .ok_or(ColorDecodeError::SizeOverflow)?;

    let mut pixels = Vec::new();

    pixels
        .try_reserve_exact(length)
        .map_err(|_| ColorDecodeError::AllocationFailed)?;

    let max_h = planes
        .components
        .iter()
        .map(|plane| usize::from(plane.horizontal_sampling))
        .max()
        .unwrap_or(1);

    let max_v = planes
        .components
        .iter()
        .map(|plane| usize::from(plane.vertical_sampling))
        .max()
        .unwrap_or(1);

    for y in 0..planes.height {
        for x in 0..planes.width {
            let sample = |index: usize| {
                let plane = &planes.components[index];

                let sx = x * usize::from(plane.horizontal_sampling) / max_h;
                let sy = y * usize::from(plane.vertical_sampling) / max_v;

                plane.samples[sy * plane.width + sx]
            };

            let rgb = match model {
                ColorModel::Grayscale => [sample(0); 3],

                ColorModel::Ycbcr => ycbcr_to_rgb(sample(0), sample(1), sample(2)),
            };

            pixels.extend_from_slice(&rgb);
        }
    }

    Ok(Rgb8Image {
        width: planes.width,
        height: planes.height,
        pixels,
    })
}

fn ycbcr_to_rgb(y: u8, cb: u8, cr: u8) -> [u8; 3] {
    let y = f64::from(y);
    let cb = f64::from(cb) - 128.0;
    let cr = f64::from(cr) - 128.0;

    let channel = |value: f64| value.round().clamp(0.0, 255.0) as u8;

    [
        channel(y + 1.402 * cr),
        channel(y - 0.34414 * cb - 0.71414 * cr),
        channel(y + 1.772 * cb),
    ]
}

#[cfg(test)]
mod tests {
    use super::super::ComponentPlane;
    use super::*;

    fn plane(id: u8, h: u8, v: u8, width: usize, height: usize, samples: &[u8]) -> ComponentPlane {
        ComponentPlane {
            id,
            horizontal_sampling: h,
            vertical_sampling: v,
            width,
            height,
            samples: samples.to_vec(),
        }
    }

    #[test]
    fn converts_jfif_colors_and_clamps_channels() {
        assert_eq!(ycbcr_to_rgb(128, 128, 128), [128, 128, 128]);
        assert_eq!(ycbcr_to_rgb(76, 85, 255), [254, 0, 0]);
        assert_eq!(ycbcr_to_rgb(0, 0, 0), [0, 135, 0]);
    }

    #[test]
    fn upsamples_odd_sized_420_planes() {
        let planes = DecodedPlanes {
            width: 3,
            height: 3,
            components: vec![
                plane(1, 2, 2, 3, 3, &[128; 9]),
                plane(2, 1, 1, 2, 2, &[128, 255, 0, 128]),
                plane(3, 1, 1, 2, 2, &[128; 4]),
            ],
        };

        let image = convert_planes(&planes, ColorModel::Ycbcr).unwrap();
        let pixel = |x: usize, y: usize| &image.pixels[(y * 3 + x) * 3..][..3];

        assert_eq!(pixel(0, 0), &[128, 128, 128]);
        assert_eq!(pixel(1, 1), &[128, 128, 128]);
        assert_eq!(pixel(2, 0), &ycbcr_to_rgb(128, 255, 128));
        assert_eq!(pixel(0, 2), &ycbcr_to_rgb(128, 0, 128));
    }

    #[test]
    fn converts_grayscale_without_color_metadata() {
        let planes = DecodedPlanes {
            width: 2,
            height: 1,
            components: vec![plane(1, 1, 1, 2, 1, &[0, 200])],
        };

        let image = convert_planes(&planes, ColorModel::Grayscale).unwrap();

        assert_eq!(image.pixels, [0, 0, 0, 200, 200, 200]);
    }

    #[test]
    fn rejects_ambiguous_and_non_ycbcr_color_metadata() {
        let planes = DecodedPlanes {
            width: 1,
            height: 1,
            components: vec![
                plane(1, 1, 1, 1, 1, &[128]),
                plane(2, 1, 1, 1, 1, &[128]),
                plane(3, 1, 1, 1, 1, &[128]),
            ],
        };

        assert!(matches!(
            color_model(&[0xff, 0xd8, 0xff, 0xd9], &planes),
            Err(ColorDecodeError::UnsupportedColorModel)
        ));

        let adobe_rgb = [
            0xff, 0xd8, 0xff, 0xee, 0, 14, b'A', b'd', b'o', b'b', b'e', 0, 100, 0, 0, 0, 0, 0,
            0xff, 0xd9,
        ];

        assert!(matches!(
            color_model(&adobe_rgb, &planes),
            Err(ColorDecodeError::UnsupportedColorModel)
        ));
    }

    #[test]
    fn decodes_wpt_jpeg_asset_without_applying_exif_orientation() {
        // This checks an asset from WPT, not the WPT browser test.
        let data = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/testdata/wpt/images/arrow-oriented-upright.jpg"
        ));

        let image = decode_baseline_rgb8(data, PlaneLimits::default()).unwrap();

        assert_eq!((image.width, image.height), (240, 144));

        for (x, y, expected) in [(0, 0, 0u8), (50, 50, 0), (120, 72, 255), (239, 143, 0)] {
            let offset = (y * image.width + x) * 3;

            for value in &image.pixels[offset..offset + 3] {
                assert!(value.abs_diff(expected) <= 1);
            }
        }
    }
}
