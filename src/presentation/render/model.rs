//! Box-part mob models: a humanoid (head, body, arms, legs) and a quadruped.
//!
//! Dimensions follow the classic Minecraft proportions (in pixels / 16 =
//! blocks), and both sample a 64×64 skin sheet with Minecraft's unwrap
//! ([`crate::presentation::art::skin`]), which is what mob art is drawn against.
//!
//! **The player is not built here any more.** It comes from
//! `assets/models/entity/player/player.bbmodel` through
//! [`crate::presentation::render::rigged`] — a real skeleton with elbows, knees and keyframe
//! clips, which a one-pivot-per-part box model cannot express. What is left is
//! the mobs that are still authored as boxes.

use glam::{Mat4, Vec3};

use crate::domain::core::Direction;
use crate::domain::core::math::yaw_matrix;
use crate::domain::entity::Pose;
use crate::domain::entity::kind::QuadrupedVisual;
use crate::presentation::art::skin::{self, SkinPart};
use wyven_render::mesh::CpuMesh;
use wyven_render::vertex::{ChunkVertex, NO_OVERLAY, NO_TINT};

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

    /// Build a renderable mesh for this model at `position` (feet) facing
    /// `yaw`, articulated by `pose`, sampling the 64×64 sheet at
    /// `sheet_origin` — how humanoid mobs reuse this model with their own
    /// skins ([`crate::presentation::art::mobskin`]). Each part is drawn twice: the base
    /// box sampling its base region of the sheet (see [`crate::presentation::art::skin`]),
    /// then a slightly inflated overlay box sampling the
    /// hat/jacket/sleeve/pants region — its transparent pixels are alpha-tested
    /// away in the shader, giving a 3D layered look. With `Pose::default()` the
    /// base geometry matches the original static model.
    pub fn build_mesh_sheet(
        &self,
        position: Vec3,
        yaw: f32,
        pose: &Pose,
        sheet_origin: [u32; 2],
    ) -> CpuMesh {
        // Overlay-shell inflation per side (Minecraft `CubeDeformation`), in blocks.
        const HAT: f32 = 0.5 / 16.0;
        const LAYER: f32 = 0.25 / 16.0;

        let mut mesh = CpuMesh::new();
        // (part, base skin, overlay skin, overlay inflation, joint pivot, rotation
        // about local X, extra turn about local Y). Limbs swing about their top
        // (shoulder/hip); the head tilts + turns about its bottom (neck); the body
        // is fixed. The overlay shares the base part's pivot/rotation so it stays
        // locked to the limb.
        let parts: [(ModelBox, SkinPart, SkinPart, f32, Vec3, f32, f32); 6] = [
            (
                self.head,
                skin::HEAD,
                skin::HAT,
                HAT,
                bottom_pivot(self.head),
                pose.head_pitch,
                pose.head_yaw,
            ),
            (
                self.body,
                skin::BODY,
                skin::JACKET,
                LAYER,
                self.body.center,
                0.0,
                0.0,
            ),
            (
                self.left_arm,
                skin::LEFT_ARM,
                skin::LEFT_SLEEVE,
                LAYER,
                top_pivot(self.left_arm),
                pose.left_arm,
                0.0,
            ),
            (
                self.right_arm,
                skin::RIGHT_ARM,
                skin::RIGHT_SLEEVE,
                LAYER,
                top_pivot(self.right_arm),
                pose.right_arm,
                0.0,
            ),
            (
                self.left_leg,
                skin::LEFT_LEG,
                skin::LEFT_PANTS,
                LAYER,
                top_pivot(self.left_leg),
                pose.left_leg,
                0.0,
            ),
            (
                self.right_leg,
                skin::RIGHT_LEG,
                skin::RIGHT_PANTS,
                LAYER,
                top_pivot(self.right_leg),
                pose.right_leg,
                0.0,
            ),
        ];
        for (part, base, overlay, inflate, pivot, rot, local_yaw) in parts {
            push_box(
                &mut mesh,
                part,
                base,
                sheet_origin,
                position,
                yaw,
                pivot,
                0.0,
                rot,
                local_yaw,
            );
            // Overlay shell: the same box grown by `inflate` on every side, sharing
            // the base part's pivot so it articulates locked to the limb.
            let shell = ModelBox {
                center: part.center,
                size: part.size + Vec3::splat(2.0 * inflate),
            };
            push_box(
                &mut mesh,
                shell,
                overlay,
                sheet_origin,
                position,
                yaw,
                pivot,
                0.0,
                rot,
                local_yaw,
            );
        }
        mesh
    }
}

/// A four-legged box model (cow, sheep): a horizontal body slab on four leg
/// posts with a head at the front (-Z). Proportions come from the kind's
/// `[entity.visual]` data ([`QuadrupedVisual`]); textures from a mob skin
/// sheet ([`crate::presentation::art::mobskin`]'s quadruped unwrap).
pub struct QuadrupedModel {
    pub body: ModelBox,
    pub head: ModelBox,
    /// Front-left, front-right, hind-left, hind-right.
    pub legs: [ModelBox; 4],
    /// Where each part reads from the sheet. The body's is the *unrotated* box
    /// it was drawn as — see [`QuadrupedModel::build_mesh`].
    body_part: SkinPart,
    head_part: SkinPart,
    leg_part: SkinPart,
}

impl QuadrupedModel {
    /// Assemble the part boxes from pixel dimensions (16 px = 1 block).
    /// Legs stand at the body's corners; the body overlaps their tops by 2 px
    /// so swinging legs never open a gap; the head sits proud at the front.
    pub fn new(v: &QuadrupedVisual) -> Self {
        let px = 1.0 / 16.0;
        let (bw, bh, bd) = (v.body[0] * px, v.body[1] * px, v.body[2] * px);
        let (hw, hh, hd) = (v.head[0] * px, v.head[1] * px, v.head[2] * px);
        let (lw, lh, ld) = (v.leg[0] * px, v.leg[1] * px, v.leg[2] * px);

        let body_bottom = lh - 2.0 * px;
        let body = ModelBox {
            center: Vec3::new(0.0, body_bottom + bh * 0.5, 0.0),
            size: Vec3::new(bw, bh, bd),
        };
        let head = ModelBox {
            // Nose forward of the body, eyes level with the body's top.
            center: Vec3::new(
                0.0,
                body_bottom + bh - hh * 0.5 + 1.0 * px,
                -(bd + hd) * 0.5 + 1.0 * px,
            ),
            size: Vec3::new(hw, hh, hd),
        };
        let (lx, lz) = ((bw - lw) * 0.5, (bd - ld) * 0.5);
        let leg = |x: f32, z: f32| ModelBox {
            center: Vec3::new(x, lh * 0.5, z),
            size: Vec3::new(lw, lh, ld),
        };
        // Mob art unwraps a quadruped's body as an *upright* box that the model
        // then tips onto its side, so its unwrap is `[width, depth, height]`
        // where the standing parts are plain `[width, height, depth]`.
        let px16 = |v: f32| v.round() as u32;
        Self {
            body,
            head,
            legs: [leg(-lx, -lz), leg(lx, -lz), leg(-lx, lz), leg(lx, lz)],
            body_part: SkinPart::new(
                v.body_uv,
                [px16(v.body[0]), px16(v.body[2]), px16(v.body[1])],
            ),
            head_part: SkinPart::new(v.head_uv, v.head.map(px16)),
            leg_part: SkinPart::new(v.leg_uv, v.leg.map(px16)),
        }
    }

    /// Build the mesh at `position` (feet) facing `yaw`, sampling the sheet at
    /// `sheet_origin`. Pose channels are reused: arms drive the front legs and
    /// legs the hind pair, so [`crate::domain::entity::AnimationState`]'s anti-phase arm/leg
    /// swing yields a natural diagonal trot with no quadruped-specific
    /// animation code.
    pub fn build_mesh(
        &self,
        position: Vec3,
        yaw: f32,
        pose: &Pose,
        sheet_origin: [u32; 2],
    ) -> CpuMesh {
        let mut mesh = CpuMesh::new();
        let swings = [pose.left_arm, pose.right_arm, pose.left_leg, pose.right_leg];
        // The body is drawn as the upright box its unwrap was authored on, then
        // tipped a quarter turn onto its side — the same trick the art assumes,
        // and the reason its `SkinPart` swaps height and depth. Tipping the
        // geometry rather than the UVs is what keeps every face rect plain.
        let upright = ModelBox {
            center: self.body.center,
            size: Vec3::new(self.body.size.x, self.body.size.z, self.body.size.y),
        };
        // (box, unwrap, pivot, tilt, animation, local yaw)
        let leg = |i: usize| {
            (
                self.legs[i],
                self.leg_part,
                top_pivot(self.legs[i]),
                0.0,
                swings[i],
                0.0,
            )
        };
        let parts = [
            (
                upright,
                self.body_part,
                self.body.center,
                -std::f32::consts::FRAC_PI_2,
                0.0,
                0.0,
            ),
            (
                self.head,
                self.head_part,
                // The neck: where the head meets the body's front face.
                self.head.center + Vec3::new(0.0, 0.0, self.head.size.z * 0.5),
                0.0,
                pose.head_pitch,
                pose.head_yaw,
            ),
            leg(0),
            leg(1),
            leg(2),
            leg(3),
        ];
        for (part, skin_part, pivot, tilt, rot, local_yaw) in parts {
            push_box(
                &mut mesh,
                part,
                skin_part,
                sheet_origin,
                position,
                yaw,
                pivot,
                tilt,
                rot,
                local_yaw,
            );
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

/// Baked face shade: how much a face is dimmed for the direction it points,
/// picked by whichever axis its normal points most nearly along.
///
/// Taking the *turned* normal rather than the face's authored direction is what
/// lets a part be tipped — a quadruped's body is drawn upright and laid on its
/// side — and still be lit as the face it has become. Rotations smaller than 45°
/// keep the same dominant axis, so a pitching head and swinging limbs are shaded
/// exactly as a static model would be.
fn shade_for(normal: Vec3) -> f32 {
    let a = normal.abs();
    if a.y >= a.x && a.y >= a.z {
        if normal.y >= 0.0 { 1.0 } else { 0.68 }
    } else if a.x >= a.z {
        0.86
    } else {
        0.80
    }
}

/// Emit the 6 faces of one model box into `mesh`, sampling the 64×64 sheet at
/// atlas `sheet_origin` (the player skin, or an armor sheet). Each vertex is
/// pitched about `pivot` by `rot`, turned about the pivot's vertical axis by
/// `local_yaw` (head look), then rotated by the global `yaw` and offset by
/// `origin`. `local_yaw` is 0 for every part but the head.
#[allow(clippy::too_many_arguments)]
fn push_box(
    mesh: &mut CpuMesh,
    part: ModelBox,
    skin_part: SkinPart,
    sheet_origin: [u32; 2],
    origin: Vec3,
    yaw: f32,
    pivot: Vec3,
    tilt: f32,
    rot: f32,
    local_yaw: f32,
) {
    // The pivot-then-yaw chain the parts articulate through, written as one
    // matrix so a caller with a transform of its own — a view model hanging off
    // the camera rather than off a body — can use the same face emitter.
    let transform = Mat4::from_translation(origin)
        * yaw_matrix(yaw)
        * Mat4::from_translation(pivot)
        * yaw_matrix(local_yaw)
        * Mat4::from_rotation_x(tilt + rot)
        * Mat4::from_translation(-pivot);
    // `tilt` is how the part is *built* — a quadruped's body is drawn upright
    // and laid on its side — and `rot` is what the animation does to it this
    // frame. Both turn the geometry; only the tilt is shaded for, or a leg
    // swinging past 45° would pop between two shades mid-stride.
    push_box_with(
        mesh,
        part,
        skin_part,
        sheet_origin,
        BoxPlacement::new(transform).shaded_as(Mat4::from_rotation_x(tilt)),
    );
}

/// Where one box goes, and which way it faces for lighting.
///
/// Three matrices rather than one because they answer different questions, and
/// only for a view model do the answers differ:
///
/// - `transform` decides where a face ends up. Always model→world.
/// - `normal_basis` orients the normal the shader dots against the sun. For
///   world geometry that is the same transform; for geometry carried by the
///   camera it must **not** be, or the hand would brighten and dim every time
///   the player turned on the spot.
/// - `shade_basis` orients the normal the *baked* face shade comes from. A
///   quadruped's body is drawn upright and tipped a quarter turn, and must read
///   as the face it has become rather than the face it was drawn as.
#[derive(Debug, Clone, Copy)]
pub struct BoxPlacement {
    pub transform: Mat4,
    pub normal_basis: Mat4,
    pub shade_basis: Mat4,
}

impl BoxPlacement {
    /// Position, light and shade all taken from one model→world transform —
    /// what every piece of world geometry wants.
    pub fn new(transform: Mat4) -> Self {
        Self {
            transform,
            normal_basis: transform,
            shade_basis: transform,
        }
    }

    /// Take the baked face shade from `basis` instead of the transform.
    pub fn shaded_as(mut self, basis: Mat4) -> Self {
        self.shade_basis = basis;
        self
    }

    /// Take the lighting normal from `basis` instead of the transform, for
    /// geometry that moves with the camera rather than with the world.
    pub fn lit_as(mut self, basis: Mat4) -> Self {
        self.normal_basis = basis;
        self
    }
}

/// Emit the 6 faces of one model box into `mesh` under an arbitrary
/// [`BoxPlacement`], sampling the 64×64 sheet at atlas `sheet_origin`.
pub fn push_box_with(
    mesh: &mut CpuMesh,
    part: ModelBox,
    skin_part: SkinPart,
    sheet_origin: [u32; 2],
    placement: BoxPlacement,
) {
    let half = part.size * 0.5;
    let lo = part.center - half;
    let hi = part.center + half;

    for dir in Direction::ALL {
        let corners = box_face_corners(dir, lo, hi);
        let uv = face_local_uv(dir);
        let normal = placement
            .normal_basis
            .transform_vector3(dir.normal())
            .to_array();
        let ao = shade_for(placement.shade_basis.transform_vector3(dir.normal()));
        let rect = skin_part.face_rect(dir);
        let quad = std::array::from_fn(|i| {
            ChunkVertex {
                position: placement.transform.transform_point3(corners[i]).to_array(),
                normal,
                uv: skin::sheet_uv(sheet_origin, rect, uv[i]),
                ao,
                flags: 0,
                // Skin sheets live in the atlas, not the block texture array.
                layer: 0,
                tint: NO_TINT,
                overlay_layer: NO_OVERLAY,
                overlay_tint: NO_TINT,
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
mod tests;
