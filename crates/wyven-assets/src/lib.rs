//! Asset intake: where bytes come from, and how images are decoded.
//!
//! Sits below both the model loaders and the game's content loader so neither
//! has to reach for the other — and below the renderer, so decoding a PNG needs
//! no Vulkan device.

pub mod image;
pub mod source;

pub use image::{Rgba8, decode_png};
pub use source::{AssetSource, EmbeddedSource, FsSource, MapSource, load_or_builtin};
