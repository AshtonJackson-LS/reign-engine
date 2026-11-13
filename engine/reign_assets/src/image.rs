//! PNG decoding into the RGBA8 pixels the renderer uploads.

use std::fmt;

/// Decoded pixels, 4 bytes per pixel in row order, top row first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Why a PNG could not be turned into RGBA8 pixels.
#[derive(Debug)]
pub enum PngError {
    /// The bytes are not a PNG the decoder can read.
    Decode(png::DecodingError),
    /// The image decoded, but to something other than 8-bit RGBA, such as
    /// greyscale or RGB without alpha.
    NotRgba(png::ColorType),
    /// The image is too large to hold in memory.
    TooLarge,
}

impl fmt::Display for PngError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PngError::Decode(e) => write!(f, "invalid PNG: {e}"),
            PngError::NotRgba(c) => write!(f, "PNG decodes to {c:?}, not RGBA"),
            PngError::TooLarge => f.write_str("PNG is too large to decode"),
        }
    }
}

impl std::error::Error for PngError {}

impl From<png::DecodingError> for PngError {
    fn from(e: png::DecodingError) -> Self {
        PngError::Decode(e)
    }
}

/// Decodes a PNG into RGBA8. Palette images and low bit depths are
/// expanded; anything that still isn't RGBA after that is an error.
pub fn decode_png(bytes: &[u8]) -> Result<RgbaImage, PngError> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info()?;
    let mut rgba = vec![0; reader.output_buffer_size().ok_or(PngError::TooLarge)?];
    let info = reader.next_frame(&mut rgba)?;
    if info.color_type != png::ColorType::Rgba {
        return Err(PngError::NotRgba(info.color_type));
    }
    rgba.truncate(info.buffer_size());
    Ok(RgbaImage { rgba, width: info.width, height: info.height })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(color: png::ColorType, pixels: &[u8], w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, w, h);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().unwrap().write_image_data(pixels).unwrap();
        out
    }

    #[test]
    fn rgba_round_trips() {
        let pixels = [1, 2, 3, 4, 5, 6, 7, 8];
        let image = decode_png(&encode(png::ColorType::Rgba, &pixels, 2, 1)).unwrap();
        assert_eq!(image, RgbaImage { rgba: pixels.to_vec(), width: 2, height: 1 });
    }

    #[test]
    fn rejects_non_rgba_and_garbage() {
        let rgb = encode(png::ColorType::Rgb, &[1, 2, 3], 1, 1);
        assert!(matches!(decode_png(&rgb), Err(PngError::NotRgba(png::ColorType::Rgb))));
        assert!(matches!(decode_png(b"not a png"), Err(PngError::Decode(_))));
    }
}
