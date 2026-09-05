//! Saving the finished frame to a PNG.
//!
//! The composited frame — world pass, then egui on top of it — only ever exists
//! as the swapchain image, and only inside [`crate::runner`]'s frame pump. So a
//! screenshot is a `copy_image_to_buffer` recorded between the egui pass and the
//! present, and read back on the CPU once the present has waited on it.
//!
//! What the engine does *not* decide is which key takes one or where the file
//! goes: those arrive from the game through [`ScreenshotConfig`], the same way
//! every other game-specific fact reaches this crate.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use vulkano::buffer::{Buffer, BufferCreateInfo, BufferUsage, Subbuffer};
use vulkano::command_buffer::{
    AutoCommandBufferBuilder, CommandBufferUsage, CopyImageToBufferInfo,
};
use vulkano::format::Format;
use vulkano::image::{ImageUsage, view::ImageView};
use vulkano::memory::allocator::{AllocationCreateInfo, MemoryTypeFilter};
use vulkano::sync::GpuFuture;
use winit::keyboard::KeyCode;
use wyven_assets::{Rgba8, encode_png};
use wyven_render::RenderContext;

/// What a game must supply before the runner will capture anything.
///
/// Returning this from [`Game::screenshots`](crate::Game::screenshots) is also
/// what opts the swapchain into `TRANSFER_SRC`: a game that never asks for a
/// screenshot never asks the driver for the usage flag either.
#[derive(Debug, Clone)]
pub struct ScreenshotConfig {
    /// The key that takes one. Read on every screen, so it works in menus too.
    pub key: KeyCode,
    /// Directory the PNGs are written to. Created on first write.
    pub dir: PathBuf,
    /// Elapsed seconds after which one frame is captured with no keypress.
    ///
    /// For automated and headless use: a key needs someone at the keyboard, and
    /// the whole point of the feature is being able to *look* at a frame from a
    /// run nobody was sitting at.
    pub auto_at: Option<f32>,
}

/// A readback in flight: the GPU has been told to fill `buffer`, and the copy
/// completes when the future it was chained onto does.
pub struct Pending {
    buffer: Subbuffer<[u8]>,
    size: [u32; 2],
    format: Format,
}

/// Record a copy of `view` into a fresh host-visible buffer, chained onto `before`.
///
/// Returns `None` — never panics — if the image cannot be copied from. A missing
/// usage flag or an exotic swapchain format should cost a log line and a missing
/// screenshot, not the session that was about to be photographed.
pub fn download(
    ctx: &RenderContext,
    before: Box<dyn GpuFuture>,
    view: &Arc<ImageView>,
) -> (Box<dyn GpuFuture>, Option<Pending>) {
    let image = view.image();
    let format = image.format();
    let extent = image.extent();
    let size = [extent[0], extent[1]];

    if !image.usage().contains(ImageUsage::TRANSFER_SRC) {
        log::error!("cannot screenshot: the swapchain was not created with TRANSFER_SRC");
        return (before, None);
    }
    if channel_order(format).is_none() {
        log::error!("cannot screenshot: unsupported swapchain format {format:?}");
        return (before, None);
    }

    let len = (size[0] as u64) * (size[1] as u64) * 4;
    let buffer = match Buffer::new_slice::<u8>(
        ctx.memory_allocator.clone(),
        BufferCreateInfo {
            usage: BufferUsage::TRANSFER_DST,
            ..Default::default()
        },
        AllocationCreateInfo {
            memory_type_filter: MemoryTypeFilter::PREFER_HOST
                | MemoryTypeFilter::HOST_RANDOM_ACCESS,
            ..Default::default()
        },
        len,
    ) {
        Ok(buffer) => buffer,
        Err(err) => {
            log::error!("screenshot buffer: {err}");
            return (before, None);
        }
    };

    let mut builder = match AutoCommandBufferBuilder::primary(
        ctx.command_allocator.clone(),
        ctx.graphics_queue().queue_family_index(),
        CommandBufferUsage::OneTimeSubmit,
    ) {
        Ok(builder) => builder,
        Err(err) => {
            log::error!("screenshot command buffer: {err}");
            return (before, None);
        }
    };
    if let Err(err) = builder.copy_image_to_buffer(CopyImageToBufferInfo::image_buffer(
        image.clone(),
        buffer.clone(),
    )) {
        log::error!("record screenshot copy: {err}");
        return (before, None);
    }

    let command_buffer = match builder.build() {
        Ok(command_buffer) => command_buffer,
        Err(err) => {
            log::error!("build screenshot copy: {err}");
            return (before, None);
        }
    };
    match before.then_execute(ctx.graphics_queue().clone(), command_buffer) {
        Ok(future) => (
            future.boxed(),
            Some(Pending {
                buffer,
                size,
                format,
            }),
        ),
        Err(err) => {
            log::error!("submit screenshot copy: {err}");
            // `then_execute` consumed the future on the way to failing, so there
            // is nothing left to hand back but a fresh no-op.
            (vulkano::sync::now(ctx.device().clone()).boxed(), None)
        }
    }
}

/// Read the completed readback and write it out. Call only once the future
/// [`download`] returned has been waited on.
///
/// Returns the path written, for the caller to log or show.
pub fn save(pending: &Pending, dir: &Path, unix_secs: u64) -> Result<PathBuf, String> {
    let bytes = pending
        .buffer
        .read()
        .map_err(|e| format!("map screenshot buffer: {e}"))?;
    let image = to_rgba8(&bytes, pending.size, pending.format)
        .ok_or_else(|| format!("unsupported swapchain format {:?}", pending.format))?;
    let png = encode_png(&image)?;

    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = unique_path(dir, unix_secs);
    std::fs::write(&path, png).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Which byte of a source pixel supplies red. `None` means we cannot read it.
///
/// Only the two 8-8-8-8 layouts a surface actually hands out are accepted, in
/// both their UNORM and SRGB spellings. The bytes are copied through verbatim
/// either way: whatever encoding the display interprets them under is the one
/// that made the image on screen, so reproducing it is exactly right.
fn channel_order(format: Format) -> Option<[usize; 3]> {
    match format {
        Format::B8G8R8A8_UNORM | Format::B8G8R8A8_SRGB => Some([2, 1, 0]),
        Format::R8G8B8A8_UNORM | Format::R8G8B8A8_SRGB => Some([0, 1, 2]),
        _ => None,
    }
}

/// Turn raw swapchain bytes into the RGBA8 every writer here speaks.
///
/// Alpha is forced opaque rather than carried over. The world pass writes
/// whatever the shaders happened to leave in that channel, and a screenshot
/// whose sky is transparent is useless for judging how the sky looks.
pub fn to_rgba8(bytes: &[u8], size: [u32; 2], format: Format) -> Option<Rgba8> {
    let [r, g, b] = channel_order(format)?;
    let count = (size[0] as usize) * (size[1] as usize);
    if bytes.len() < count * 4 {
        return None;
    }

    let mut pixels = Vec::with_capacity(count * 4);
    for i in 0..count {
        let src = &bytes[i * 4..i * 4 + 4];
        pixels.extend_from_slice(&[src[r], src[g], src[b], 255]);
    }
    Some(Rgba8 { pixels, size })
}

/// `YYYY-MM-DD_HH-MM-SS.png` in UTC, with a `-2`, `-3`… suffix if that second
/// already has a file. Two screenshots in one second is a held key, not a
/// mistake, so neither should be silently dropped.
fn unique_path(dir: &Path, unix_secs: u64) -> PathBuf {
    let stem = timestamp(unix_secs);
    let first = dir.join(format!("{stem}.png"));
    if !first.exists() {
        return first;
    }
    for n in 2.. {
        let path = dir.join(format!("{stem}-{n}.png"));
        if !path.exists() {
            return path;
        }
    }
    unreachable!("the range is unbounded")
}

/// Format seconds-since-the-epoch as `YYYY-MM-DD_HH-MM-SS`, UTC.
///
/// Hand-rolled because the alternative is a date crate for one line of output.
/// The civil-from-days conversion is Howard Hinnant's, which is exact for every
/// date this will ever see and does not need a leap-second table.
pub fn timestamp(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let secs = unix_secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    format!("{year:04}-{month:02}-{day:02}_{h:02}-{m:02}-{s:02}")
}

/// Days since 1970-01-01 → (year, month, day).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgra_is_swapped_into_rgba() {
        // One pixel, stored blue-green-red-alpha.
        let bytes = [10u8, 20, 30, 40];
        let image = to_rgba8(&bytes, [1, 1], Format::B8G8R8A8_UNORM).expect("supported");
        assert_eq!(image.pixels, vec![30, 20, 10, 255]);
        assert_eq!(image.size, [1, 1]);
    }

    #[test]
    fn rgba_passes_through_unswapped() {
        let bytes = [10u8, 20, 30, 40];
        let image = to_rgba8(&bytes, [1, 1], Format::R8G8B8A8_UNORM).expect("supported");
        assert_eq!(image.pixels, vec![10, 20, 30, 255]);
    }

    /// The sky is drawn with whatever alpha the shaders left behind. Carrying it
    /// into the file would produce a screenshot you cannot see.
    #[test]
    fn alpha_is_always_opaque() {
        let bytes = [1u8, 2, 3, 0, 4, 5, 6, 0];
        let image = to_rgba8(&bytes, [2, 1], Format::R8G8B8A8_UNORM).expect("supported");
        assert_eq!(image.pixels[3], 255);
        assert_eq!(image.pixels[7], 255);
    }

    #[test]
    fn srgb_spellings_are_accepted_too() {
        assert!(to_rgba8(&[0; 4], [1, 1], Format::B8G8R8A8_SRGB).is_some());
        assert!(to_rgba8(&[0; 4], [1, 1], Format::R8G8B8A8_SRGB).is_some());
    }

    /// A format we cannot interpret must produce no file at all — writing the
    /// bytes anyway would give a picture whose colours silently lie.
    #[test]
    fn an_unsupported_format_is_refused() {
        assert!(to_rgba8(&[0; 8], [1, 1], Format::R16G16B16A16_SFLOAT).is_none());
    }

    #[test]
    fn a_short_buffer_is_refused_rather_than_padded() {
        assert!(to_rgba8(&[0; 4], [2, 2], Format::B8G8R8A8_UNORM).is_none());
    }

    #[test]
    fn the_epoch_formats_as_the_start_of_1970() {
        assert_eq!(timestamp(0), "1970-01-01_00-00-00");
    }

    #[test]
    fn a_known_instant_formats_correctly() {
        // 2026-09-05T14:32:07Z
        assert_eq!(timestamp(1_788_618_727), "2026-09-05_14-32-07");
    }

    /// Leap years are where a hand-rolled calendar goes wrong, so pin one.
    #[test]
    fn the_day_after_a_leap_day_is_the_first_of_march() {
        // 2024-02-29T00:00:00Z
        assert_eq!(timestamp(1_709_164_800), "2024-02-29_00-00-00");
        assert_eq!(timestamp(1_709_164_800 + 86_400), "2024-03-01_00-00-00");
    }
}
