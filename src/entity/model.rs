//! A simple box-part humanoid model (head, body, arms, legs) used to render the
//! player in third person and remote players in multiplayer.
//!
//! Dimensions follow the classic Minecraft proportions (in pixels / 16 = blocks).

use glam::Vec3;

use crate::core::Direction;
use crate::render::mesh::CpuMesh;
use crate::render::texture::atlas_uv;
use crate::render::vertex::ChunkVertex;

/// Atlas tiles used by the humanoid model.
pub const PLAYER_SKIN_TILE: u32 = 13;
pub const PLAYER_SHIRT_TILE: u32 = 14;

/// One rectangular box part of a model, in model-local space (origin at feet).
#[derive(Debug, Clone, Copy)]
pub struct ModelBox {
    /// Centre offset from the model origin.
    pub center: Vec3,
    /// Full extents of the box.
    pub size: Vec3,
}

/// Static layout of the humanoid model parts.
pub struct HumanoidModel {
    pub head: ModelBox,
    pub body: ModelBox,
    pub left_arm: ModelBox,
    pub right_arm: ModelBox,
    pub left_leg: ModelBox,
    pub right_leg: ModelBox,
}

impl HumanoidModel {
    /// Standard player proportions (1 block = 16px).
    pub fn player() -> Self {
        let px = 1.0 / 16.0;
        let leg = Vec3::new(4.0, 12.0, 4.0) * px;
        let arm = Vec3::new(4.0, 12.0, 4.0) * px;
        let body = Vec3::new(8.0, 12.0, 4.0) * px;
        let head = Vec3::splat(8.0) * px;

        Self {
            // Heights stack: legs (0..12px), body (12..24px), head (24..32px).
            right_leg: ModelBox {
                center: Vec3::new(-2.0 * px, 6.0 * px, 0.0),
                size: leg,
            },
            left_leg: ModelBox {
                center: Vec3::new(2.0 * px, 6.0 * px, 0.0),
                size: leg,
            },
            body: ModelBox {
                center: Vec3::new(0.0, 18.0 * px, 0.0),
                size: body,
            },
            right_arm: ModelBox {
                center: Vec3::new(-6.0 * px, 18.0 * px, 0.0),
                size: arm,
            },
            left_arm: ModelBox {
                center: Vec3::new(6.0 * px, 18.0 * px, 0.0),
                size: arm,
            },
            head: ModelBox {
                center: Vec3::new(0.0, 28.0 * px, 0.0),
                size: head,
            },
        }
    }

    pub fn parts(&self) -> [ModelBox; 6] {
        [
            self.head,
            self.body,
            self.left_arm,
            self.right_arm,
            self.left_leg,
            self.right_leg,
        ]
    }

    /// Build a renderable mesh for this model at `position` (feet) facing `yaw`.
    /// Uses the skin tile for the head and the shirt tile for the rest.
    pub fn build_mesh(&self, position: Vec3, yaw: f32) -> CpuMesh {
        let mut mesh = CpuMesh::new();
        let tiles = [
            PLAYER_SKIN_TILE,  // head
            PLAYER_SHIRT_TILE, // body
            PLAYER_SHIRT_TILE, // left arm
            PLAYER_SHIRT_TILE, // right arm
            PLAYER_SHIRT_TILE, // left leg
            PLAYER_SHIRT_TILE, // right leg
        ];
        for (part, tile) in self.parts().iter().zip(tiles) {
            push_box(&mut mesh, *part, tile, position, yaw);
        }
        mesh
    }
}

/// Rotate a point around the Y axis (matches `Player::look_direction`).
fn rot_y(p: Vec3, yaw: f32) -> Vec3 {
    let (s, c) = yaw.sin_cos();
    Vec3::new(p.x * c - p.z * s, p.y, p.x * s + p.z * c)
}

fn face_shade(dir: Direction) -> f32 {
    match dir {
        Direction::PosY => 1.0,
        Direction::NegY => 0.5,
        Direction::PosX | Direction::NegX => 0.78,
        Direction::PosZ | Direction::NegZ => 0.66,
    }
}

/// Emit the 6 faces of one model box into `mesh`, rotated by `yaw` and offset by
/// `origin`.
fn push_box(mesh: &mut CpuMesh, part: ModelBox, tile: u32, origin: Vec3, yaw: f32) {
    let half = part.size * 0.5;
    let lo = part.center - half;
    let hi = part.center + half;
    let uv = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];

    for dir in Direction::ALL {
        let corners = box_face_corners(dir, lo, hi);
        let normal = rot_y(dir.normal(), yaw).to_array();
        let ao = face_shade(dir);
        let quad = std::array::from_fn(|i| {
            let world = rot_y(corners[i], yaw) + origin;
            ChunkVertex {
                position: world.to_array(),
                normal,
                uv: atlas_uv(tile, uv[i]),
                ao,
            }
        });
        mesh.push_quad(quad);
    }
}

/// The four corners of a box face, CCW from outside (min/max combinations).
fn box_face_corners(dir: Direction, lo: Vec3, hi: Vec3) -> [Vec3; 4] {
    match dir {
        Direction::PosX => [
            Vec3::new(hi.x, lo.y, hi.z),
            Vec3::new(hi.x, lo.y, lo.z),
            Vec3::new(hi.x, hi.y, lo.z),
            Vec3::new(hi.x, hi.y, hi.z),
        ],
        Direction::NegX => [
            Vec3::new(lo.x, lo.y, lo.z),
            Vec3::new(lo.x, lo.y, hi.z),
            Vec3::new(lo.x, hi.y, hi.z),
            Vec3::new(lo.x, hi.y, lo.z),
        ],
        Direction::PosY => [
            Vec3::new(lo.x, hi.y, hi.z),
            Vec3::new(hi.x, hi.y, hi.z),
            Vec3::new(hi.x, hi.y, lo.z),
            Vec3::new(lo.x, hi.y, lo.z),
        ],
        Direction::NegY => [
            Vec3::new(lo.x, lo.y, lo.z),
            Vec3::new(hi.x, lo.y, lo.z),
            Vec3::new(hi.x, lo.y, hi.z),
            Vec3::new(lo.x, lo.y, hi.z),
        ],
        Direction::PosZ => [
            Vec3::new(hi.x, lo.y, hi.z),
            Vec3::new(lo.x, lo.y, hi.z),
            Vec3::new(lo.x, hi.y, hi.z),
            Vec3::new(hi.x, hi.y, hi.z),
        ],
        Direction::NegZ => [
            Vec3::new(lo.x, lo.y, lo.z),
            Vec3::new(hi.x, lo.y, lo.z),
            Vec3::new(hi.x, hi.y, lo.z),
            Vec3::new(lo.x, hi.y, lo.z),
        ],
    }
}
