//! CPU-side images and the one PNG decoder every asset goes through.
//!
//! Deliberately GPU-free: model files carry their own PNGs, so the loaders need
//! to decode without a Vulkan device anywhere in scope.

/// A decoded RGBA8 image on the CPU, row-major from the top-left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba8 {
    pub pixels: Vec<u8>,
    pub size: [u32; 2],
}

impl Rgba8 {
    pub fn width(&self) -> u32 {
        self.size[0]
    }

    pub fn height(&self) -> u32 {
        self.size[1]
    }
}

/// Decode a PNG of any size to RGBA8, normalising bit depth and colour type.
///
/// Every PNG the game reads goes through here — skins, armor sheets, block
/// tiles, and the textures embedded in model files — so there is one answer to
/// "which PNG flavours do we accept".
pub fn decode_png(bytes: &[u8]) -> Result<Rgba8, String> {
    let mut decoder = png::Decoder::new(bytes);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;

    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        other => return Err(format!("unsupported color type {other:?}")),
    };
    let count = (info.width as usize) * (info.height as usize);
    let mut pixels = Vec::with_capacity(count * 4);
    for i in 0..count {
        let src = &buf[i * channels..];
        let px = match channels {
            1 => [src[0], src[0], src[0], 255],
            2 => [src[0], src[0], src[0], src[1]],
            3 => [src[0], src[1], src[2], 255],
            _ => [src[0], src[1], src[2], src[3]],
        };
        pixels.extend_from_slice(&px);
    }
    Ok(Rgba8 {
        pixels,
        size: [info.width, info.height],
    })
}

/// Encode an RGBA8 image to PNG bytes.
///
/// The inverse of [`decode_png`], and its round-trip partner: everything this
/// crate hands out is RGBA8, so there is exactly one flavour to write back.
///
/// Fails rather than truncating if `pixels` does not hold `width * height * 4`
/// bytes — a short buffer here means the caller mis-sized a readback, and a PNG
/// with a torn last row is a far worse thing to debug than an error.
pub fn encode_png(image: &Rgba8) -> Result<Vec<u8>, String> {
    let [width, height] = image.size;
    let expected = (width as usize) * (height as usize) * 4;
    if image.pixels.len() != expected {
        return Err(format!(
            "expected {expected} bytes for {width}x{height} RGBA8, got {}",
            image.pixels.len()
        ));
    }

    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer
            .write_image_data(&image.pixels)
            .map_err(|e| e.to_string())?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two halves of this module are each other's inverse. A screenshot is
    /// written by `encode_png` and read back by `decode_png` in the tests that
    /// check it, so a drift between them would show up as a passing test over a
    /// broken file.
    #[test]
    fn encoding_then_decoding_returns_the_original_image() {
        let image = Rgba8 {
            pixels: vec![
                255, 0, 0, 255, // red
                0, 255, 0, 128, // half-transparent green
                0, 0, 255, 255, // blue
                9, 9, 9, 0, // fully transparent
            ],
            size: [2, 2],
        };

        let bytes = encode_png(&image).expect("encode");
        let back = decode_png(&bytes).expect("decode");

        assert_eq!(back, image);
    }

    #[test]
    fn a_pixel_buffer_that_does_not_match_the_size_is_rejected() {
        let image = Rgba8 {
            pixels: vec![255; 4 * 3],
            size: [2, 2],
        };
        assert!(encode_png(&image).is_err());
    }
}
