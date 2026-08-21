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
