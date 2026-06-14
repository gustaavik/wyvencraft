//! Graphics pipelines, one per render pass:
//!
//! - `voxel`        — opaque chunk geometry (depth-tested, back-face culled)
//! - `transparent`  — water/glass (depth-test, no depth-write, alpha blend)
//! - `entity`       — player/entity models
//! - `debug`        — line list for selection outlines/wireframes
//!
//! Each is created against the swapchain image format with shared camera
//! uniforms. Implemented in M2 (voxel) and the milestones that follow.

pub mod voxel;
