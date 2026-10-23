use crate::graphics::raster::{Color, Surface};
use std::{error::Error, fmt, io::Cursor};

const MAX_IMAGE_DIMENSION: usize = 4_096;
const MAX_IMAGE_PIXELS: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageError {
    message: String,
}

impl ImageError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ImageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid image: {}", self.message)
    }
}

impl Error for ImageError {}

pub fn decode_png(encoded: &[u8]) -> Result<Surface, ImageError> {
    let mut decoder = png::Decoder::new(Cursor::new(encoded));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);

    let mut reader = decoder
        .read_info()
        .map_err(|error| ImageError::new(error.to_string()))?;

    let header = reader.info();
    let width = header.width as usize;
    let height = header.height as usize;

    validate_dimensions(width, height)?;

    let mut buffer = vec![0; reader.output_buffer_size()];

    let frame = reader
        .next_frame(&mut buffer)
        .map_err(|error| ImageError::new(error.to_string()))?;

    let width = frame.width as usize;
    let height = frame.height as usize;

    validate_dimensions(width, height)?;

    let bytes = &buffer[..frame.buffer_size()];
    let channels = match frame.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Indexed => {
            return Err(ImageError::new("indexed PNG was not expanded"));
        }
    };

    let expected_length = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(channels))
        .ok_or_else(|| ImageError::new("PNG pixel data is too large"))?;

    if bytes.len() != expected_length {
        return Err(ImageError::new("PNG pixel data has an invalid length"));
    }

    let mut surface = Surface::new(width, height, Color::TRANSPARENT);

    for (index, pixel) in bytes.chunks_exact(channels).enumerate() {
        let color = match frame.color_type {
            png::ColorType::Rgb => Color::rgba(pixel[0], pixel[1], pixel[2], 255),
            png::ColorType::Rgba => Color::rgba(pixel[0], pixel[1], pixel[2], pixel[3]),
            png::ColorType::Grayscale => Color::rgba(pixel[0], pixel[0], pixel[0], 255),
            png::ColorType::GrayscaleAlpha => Color::rgba(pixel[0], pixel[0], pixel[0], pixel[1]),
            png::ColorType::Indexed => unreachable!(),
        };

        let x = index % width;
        let y = index / width;

        surface.set_pixel(x, y, color);
    }

    Ok(surface)
}

pub fn encode_png(surface: &Surface) -> Result<Vec<u8>, ImageError> {
    let width = u32::try_from(surface.width())
        .map_err(|_| ImageError::new("image width exceeds PNG limits"))?;
    let height = u32::try_from(surface.height())
        .map_err(|_| ImageError::new("image height exceeds PNG limits"))?;

    let capacity = surface
        .width()
        .checked_mul(surface.height())
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| ImageError::new("image pixel data is too large"))?;

    let mut pixels = Vec::with_capacity(capacity);

    for color in surface.pixels() {
        pixels.extend_from_slice(&[color.red, color.green, color.blue, color.alpha]);
    }

    let mut encoded = Vec::new();

    {
        let mut encoder = png::Encoder::new(&mut encoded, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);

        let mut writer = encoder
            .write_header()
            .map_err(|error| ImageError::new(error.to_string()))?;

        writer
            .write_image_data(&pixels)
            .map_err(|error| ImageError::new(error.to_string()))?;
    }

    Ok(encoded)
}

fn validate_dimensions(width: usize, height: usize) -> Result<(), ImageError> {
    if width == 0 || height == 0 {
        return Err(ImageError::new("image dimensions must not be zero"));
    }

    if width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
        return Err(ImageError::new(
            "image dimensions exceed implementation limits",
        ));
    }

    let pixels = width
        .checked_mul(height)
        .ok_or_else(|| ImageError::new("image dimensions overflow"))?;

    if pixels > MAX_IMAGE_PIXELS {
        return Err(ImageError::new("image contains too many pixels"));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_and_decodes_rgba_pixels() {
        let mut source = Surface::new(2, 2, Color::TRANSPARENT);
        source.set_pixel(0, 0, Color::rgba(255, 0, 0, 255));
        source.set_pixel(1, 0, Color::rgba(0, 255, 0, 128));
        source.set_pixel(0, 1, Color::rgba(0, 0, 255, 64));
        source.set_pixel(1, 1, Color::rgba(255, 255, 255, 0));

        let encoded = encode_png(&source).unwrap();

        assert_eq!(&encoded[..8], b"\x89PNG\r\n\x1a\n");

        let decoded = decode_png(&encoded).unwrap();

        assert_eq!(decoded.width(), 2);
        assert_eq!(decoded.height(), 2);
        assert_eq!(decoded.pixel(0, 0), Some(Color::rgba(255, 0, 0, 255)));
        assert_eq!(decoded.pixel(1, 0), Some(Color::rgba(0, 255, 0, 128)));
        assert_eq!(decoded.pixel(0, 1), Some(Color::rgba(0, 0, 255, 64)));
        assert_eq!(decoded.pixel(1, 1), Some(Color::rgba(255, 255, 255, 0)));
    }

    #[test]
    fn rejects_non_png_data() {
        assert!(decode_png(b"FreeDOM is not a PNG").is_err());
    }
}
