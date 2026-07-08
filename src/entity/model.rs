//! A simple box-part humanoid model (head, body, arms, legs) used to render the
//! player in third person and remote players in multiplayer.
//!
//! Dimensions follow the classic Minecraft proportions (in pixels / 16 = blocks).

use glam::Vec3;

use crate::core::Direction;
use crate::render::mesh::CpuMesh;
use crate::render::skin::{self, SkinPart};
use crate::render::vertex::ChunkVertex;

/// One rectangular box part of a model, in model-local space (origin at feet).
#[derive(Debug, Clone, Copy)]
pub struct ModelBox {
    /// Centre offset from the model origin.
    pub center: Vec3,
    /// Full extents of the box.
    pub size: Vec3,
}

/// Per-part articulation applied (around each joint pivot) before the global
/// yaw/translate. All angles are rotations about the model-local X axis, in radians.
/// `Pose::default()` is the rest pose, identical to the original static model.
#[derive(Debug, Clone, Copy, Default)]
pub struct Pose {
    /// Head tilt (look up/down), rotated about the neck.
    pub head_pitch: f32,
    pub left_arm: f32,
    pub right_arm: f32,
    pub left_leg: f32,
    pub right_leg: f32,
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
            // The model faces -Z, so the character's right side is +X (matches
            // `Player::right()` at yaw 0) — NOT the Minecraft skin convention,
            // whose model faces +Z with the right arm at -X.
            right_leg: ModelBox {
                center: Vec3::new(2.0 * px, 6.0 * px, 0.0),
                size: leg,
            },
            left_leg: ModelBox {
                center: Vec3::new(-2.0 * px, 6.0 * px, 0.0),
                size: leg,
            },
            body: ModelBox {
                center: Vec3::new(0.0, 18.0 * px, 0.0),
                size: body,
            },
            right_arm: ModelBox {
                center: Vec3::new(6.0 * px, 18.0 * px, 0.0),
                size: arm,
            },
            left_arm: ModelBox {
                center: Vec3::new(-6.0 * px, 18.0 * px, 0.0),
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

    /// Build a renderable mesh for this model at `position` (feet) facing `yaw`,
    /// articulated by `pose`. Each part is drawn twice: the base box sampling its
    /// base region of the player skin sheet (see [`crate::render::skin`]), then a
    /// slightly inflated overlay box sampling the hat/jacket/sleeve/pants region —
    /// its transparent pixels are alpha-tested away in the shader, giving a 3D
    /// layered look. With `Pose::default()` the base geometry matches the original
    /// static model.
    pub fn build_mesh(&self, position: Vec3, yaw: f32, pose: &Pose) -> CpuMesh {
        // Overlay-shell inflation per side (Minecraft `CubeDeformation`), in blocks.
        const HAT: f32 = 0.5 / 16.0;
        const LAYER: f32 = 0.25 / 16.0;

        let mut mesh = CpuMesh::new();
        // (part, base skin, overlay skin, overlay inflation, joint pivot, rotation
        // about local X). Limbs swing about their top (shoulder/hip); the head tilts
        // about its bottom (neck); the body is fixed. The overlay shares the base
        // part's pivot/rotation so it stays locked to the limb.
        let parts: [(ModelBox, SkinPart, SkinPart, f32, Vec3, f32); 6] = [
            (
                self.head,
                skin::HEAD,
                skin::HAT,
                HAT,
                bottom_pivot(self.head),
                pose.head_pitch,
            ),
            (
                self.body,
                skin::BODY,
                skin::JACKET,
                LAYER,
                self.body.center,
                0.0,
            ),
            (
                self.left_arm,
                skin::LEFT_ARM,
                skin::LEFT_SLEEVE,
                LAYER,
                top_pivot(self.left_arm),
                pose.left_arm,
            ),
            (
                self.right_arm,
                skin::RIGHT_ARM,
                skin::RIGHT_SLEEVE,
                LAYER,
                top_pivot(self.right_arm),
                pose.right_arm,
            ),
            (
                self.left_leg,
                skin::LEFT_LEG,
                skin::LEFT_PANTS,
                LAYER,
                top_pivot(self.left_leg),
                pose.left_leg,
            ),
            (
                self.right_leg,
                skin::RIGHT_LEG,
                skin::RIGHT_PANTS,
                LAYER,
                top_pivot(self.right_leg),
                pose.right_leg,
            ),
        ];
        for (part, base, overlay, inflate, pivot, rot) in parts {
            push_box(&mut mesh, part, base, position, yaw, pivot, rot);
            // Overlay shell: the same box grown by `inflate` on every side, sharing
            // the base part's pivot so it articulates locked to the limb.
            let shell = ModelBox {
                center: part.center,
                size: part.size + Vec3::splat(2.0 * inflate),
            };
            push_box(&mut mesh, shell, overlay, position, yaw, pivot, rot);
        }
        mesh
    }
}

/// Joint pivot at the top centre of a box (shoulder / hip).
fn top_pivot(b: ModelBox) -> Vec3 {
    b.center + Vec3::new(0.0, b.size.y * 0.5, 0.0)
}

/// Joint pivot at the bottom centre of a box (neck).
fn bottom_pivot(b: ModelBox) -> Vec3 {
    b.center - Vec3::new(0.0, b.size.y * 0.5, 0.0)
}

/// Rotate a point around the Y axis (matches `Player::look_direction`).
fn rot_y(p: Vec3, yaw: f32) -> Vec3 {
    let (s, c) = yaw.sin_cos();
    Vec3::new(p.x * c - p.z * s, p.y, p.x * s + p.z * c)
}

/// Rotate a point around the X axis (limb swing / head tilt).
fn rot_x(p: Vec3, a: f32) -> Vec3 {
    let (s, c) = a.sin_cos();
    Vec3::new(p.x, p.y * c - p.z * s, p.y * s + p.z * c)
}

fn face_shade(dir: Direction) -> f32 {
    match dir {
        Direction::PosY => 1.0,
        Direction::NegY => 0.5,
        Direction::PosX | Direction::NegX => 0.78,
        Direction::PosZ | Direction::NegZ => 0.66,
    }
}

/// Emit the 6 faces of one model box into `mesh`. Each vertex is rotated about
/// `pivot` by `rot` (the joint articulation), then by `yaw`, then offset by `origin`.
fn push_box(
    mesh: &mut CpuMesh,
    part: ModelBox,
    skin_part: SkinPart,
    origin: Vec3,
    yaw: f32,
    pivot: Vec3,
    rot: f32,
) {
    let half = part.size * 0.5;
    let lo = part.center - half;
    let hi = part.center + half;

    for dir in Direction::ALL {
        let corners = box_face_corners(dir, lo, hi);
        let uv = face_local_uv(dir);
        let normal = rot_y(rot_x(dir.normal(), rot), yaw).to_array();
        let ao = face_shade(dir);
        let rect = skin_part.face_rect(dir);
        let quad = std::array::from_fn(|i| {
            let local = rot_x(corners[i] - pivot, rot) + pivot;
            let world = rot_y(local, yaw) + origin;
            ChunkVertex {
                position: world.to_array(),
                normal,
                uv: skin::face_uv(rect, uv[i]),
                ao,
                flags: 0,
            }
        });
        mesh.push_quad(quad);
    }
}

/// Local skin UVs (`u` right, `v` down) for the four corners that
/// [`box_face_corners`] returns, oriented so the Minecraft skin sheet reads
/// correctly on our model frame (front = -Z, character's right = +X, up = +Y).
///
/// Our model is the skin's authoring frame (front +Z, right arm -X) rotated 180°
/// about Y, so the sides map straight through, but the front/back faces are
/// horizontally reversed and the top/bottom caps run front-to-back the other way.
fn face_local_uv(dir: Direction) -> [[f32; 2]; 4] {
    match dir {
        // Sides: u runs back→front (+X) / front→back (-X), v top→bottom.
        Direction::PosX | Direction::NegX => [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
        // Front/back: u runs +X→-X (front) / -X→+X (back), v top→bottom.
        Direction::NegZ | Direction::PosZ => [[1.0, 1.0], [0.0, 1.0], [0.0, 0.0], [1.0, 0.0]],
        // Caps: u runs +X→-X, v runs back→front.
        Direction::PosY | Direction::NegY => [[1.0, 0.0], [0.0, 0.0], [0.0, 1.0], [1.0, 1.0]],
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn rot_x_rotates_up_to_back() {
        // +Y rotated by +90° about X lands on +Z — the model's BACK (the front is
        // -Z). A hanging limb (-Y) under a positive angle thus swings forward (-Z).
        let r = rot_x(Vec3::Y, FRAC_PI_2);
        assert!((r - Vec3::Z).length() < 1e-6, "got {r:?}");
    }

    #[test]
    fn right_limbs_sit_on_the_characters_right() {
        // Facing -Z with +Y up, the character's right side is +X.
        let model = HumanoidModel::player();
        assert!(model.right_arm.center.x > 0.0);
        assert!(model.right_leg.center.x > 0.0);
        assert!(model.left_arm.center.x < 0.0);
        assert!(model.left_leg.center.x < 0.0);
    }

    #[test]
    fn rest_pose_matches_static_layout() {
        let model = HumanoidModel::player();
        let mesh = model.build_mesh(Vec3::ZERO, 0.0, &Pose::default());
        // 6 parts, each drawn as a base + an inflated overlay box:
        // 12 boxes × 6 faces × 4 vertices.
        assert_eq!(mesh.vertices.len(), 288);
        // At origin with zero yaw and a rest pose the geometry is untransformed: the
        // base head top sits at 32px = 2.0 units and the base feet at 0, and the
        // overlay shell inflates the extremes by the hat (+0.5px) / pants (-0.25px)
        // deformation.
        let (min_y, max_y) = mesh
            .vertices
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), v| {
                (lo.min(v.position[1]), hi.max(v.position[1]))
            });
        assert!((max_y - (2.0 + 0.5 / 16.0)).abs() < 1e-6, "max_y={max_y}");
        assert!((min_y - (-0.25 / 16.0)).abs() < 1e-6, "min_y={min_y}");
    }

    #[test]
    fn head_front_face_wears_the_face_texture() {
        let model = HumanoidModel::player();
        let mesh = model.build_mesh(Vec3::ZERO, 0.0, &Pose::default());
        // The head is the first part (24 vertices); its front face (-Z, the 5th
        // in Direction::ALL order) must sample UVs inside the head's front rect
        // on the skin sheet.
        let front = &mesh.vertices[16..20];
        let rect = skin::HEAD.face_rect(Direction::NegZ);
        let uv0 = skin::face_uv(rect, [0.0, 0.0]);
        let uv1 = skin::face_uv(rect, [1.0, 1.0]);
        for v in front {
            assert_eq!(v.normal, [0.0, 0.0, -1.0], "front face points -Z");
            assert!(
                v.uv[0] >= uv0[0] && v.uv[0] <= uv1[0],
                "u in tile: {:?}",
                v.uv
            );
            assert!(
                v.uv[1] >= uv0[1] && v.uv[1] <= uv1[1],
                "v in tile: {:?}",
                v.uv
            );
        }
    }

    #[test]
    fn limb_rotates_about_top_pivot() {
        let leg = HumanoidModel::player().right_leg;
        let pivot = top_pivot(leg);
        let half = leg.size * 0.5;
        let top = leg.center + Vec3::new(0.0, half.y, 0.0); // hip == pivot
        let foot = leg.center - Vec3::new(0.0, half.y, 0.0);

        let angle = 0.6;
        let rot_top = rot_x(top - pivot, angle) + pivot;
        let rot_foot = rot_x(foot - pivot, angle) + pivot;

        // The hip stays anchored at the pivot; the foot swings out along Z.
        assert!((rot_top - top).length() < 1e-6, "hip moved to {rot_top:?}");
        assert!(
            (rot_foot.z - foot.z).abs() > 0.1,
            "foot z barely moved: {rot_foot:?}"
        );
    }
}
