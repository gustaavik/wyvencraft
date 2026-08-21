//! Graphics pipelines, one per render pass:
//!
//! - `sky`          — procedural fullscreen sky (drawn first, no depth write)
//! - `voxel`        — opaque chunk geometry off the 16px atlas
//! - `voxel_array`  — the same, for blocks sampling the block texture array
//! - `transparent`  — water/glass (depth-test, no depth-write, alpha blend)
//! - `entity`       — player/entity models
//! - `debug`        — line list for selection outlines/wireframes
//!
//! Each is created against the swapchain image format with shared camera
//! uniforms. Implemented in M2 (voxel) and the milestones that follow.

pub mod line;
pub mod sky;
pub mod voxel;
pub mod voxel_array;
