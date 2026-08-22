//! A simple box-part humanoid model (head, body, arms, legs) used to render the
//! player in third person and remote players in multiplayer.
//!
//! Dimensions follow the classic Minecraft proportions (in pixels / 16 = blocks).

use glam::Vec3;

use crate::art::armor::ArmorKind;
use crate::art::skin::{self, SkinPart};
use crate::core::Direction;
use crate::core::math::rotate_y as rot_y;
use crate::entity::kind::QuadrupedVisual;
use crate::inventory::{ARMOR_SIZE, ArmorSlot, ItemId, ItemRegistry};
use wyven_render::mesh::CpuMesh;
use wyven_render::vertex::{ChunkVertex, NO_TINT};

/// One rectangular box part of a model, in model-local space (origin at feet).
#[derive(Debug, Clone, Copy)]
pub struct ModelBox {
    /// Centre offset from the model origin.
    pub center: Vec3,
    /// Full extents of the box.
    pub size: Vec3,
}

/// Per-part articulation applied (around each joint pivot) before the global
/// yaw/translate. Angles are rotations about the model-local X axis (radians),
/// except `head_yaw` which turns the head about the neck's vertical axis.
/// `Pose::default()` is the rest pose, identical to the original static model.
#[derive(Debug, Clone, Copy, Default)]
pub struct Pose {
    /// Head tilt (look up/down), rotated about the neck.
    pub head_pitch: f32,
    /// Head turn (look left/right), rotated about the neck. Only the inventory
    /// preview uses it (to track the cursor); the world pose leaves it 0.
    pub head_yaw: f32,
    pub left_arm: f32,
    pub right_arm: f32,
    pub left_leg: f32,
    pub right_leg: f32,
}

/// Where an item held in the right hand sits: the fist's world position, plus
/// the rotations it inherits from the body turn and the arm swing.
#[derive(Debug, Clone, Copy)]
pub struct HandAnchor {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
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
    /// articulated by `pose`, sampling the player skin sheet.
    pub fn build_mesh(&self, position: Vec3, yaw: f32, pose: &Pose) -> CpuMesh {
        self.build_mesh_sheet(position, yaw, pose, skin::SKIN_ORIGIN)
    }

    /// Like [`HumanoidModel::build_mesh`] but sampling the 64×64 sheet at
    /// `sheet_origin` — how humanoid mobs reuse this model with their own
    /// skins ([`crate::art::mobskin`]). Each part is drawn twice: the base
    /// box sampling its base region of the sheet (see [`crate::art::skin`]),
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

    /// Like [`HumanoidModel::build_mesh`], plus an inflated shell for each worn
    /// armor piece. `armor` gives the item in each [`ArmorSlot`] (in `ALL`
    /// order); pieces sample their own atlas sheet and their transparent pixels
    /// are alpha-tested away, so partial coverage (boots, gloves) reads right.
    pub fn build_mesh_armored(
        &self,
        position: Vec3,
        yaw: f32,
        pose: &Pose,
        armor: &[Option<ItemId>; ARMOR_SIZE],
        items: &ItemRegistry,
    ) -> CpuMesh {
        let mut mesh = self.build_mesh(position, yaw, pose);
        for slot in ArmorSlot::ALL {
            let Some(id) = armor[slot.index()] else {
                continue;
            };
            // Only draw a piece that actually declares this slot.
            if items.armor(id).map(|a| a.slot) != Some(slot) {
                continue;
            }
            self.push_armor(&mut mesh, slot, position, yaw, pose);
        }
        mesh
    }

    /// Append one armor piece's inflated shells to `mesh`.
    fn push_armor(
        &self,
        mesh: &mut CpuMesh,
        slot: ArmorSlot,
        position: Vec3,
        yaw: f32,
        pose: &Pose,
    ) {
        let kind = armor_kind(slot);
        let origin = kind.origin();
        let inflate = armor_inflation(slot) / 16.0;

        // The cape is a standalone box hung off the shoulders, not a body part.
        if slot == ArmorSlot::Cape {
            let shell = ModelBox {
                center: cape_box().center,
                size: cape_box().size + Vec3::splat(2.0 * inflate),
            };
            let pivot = top_pivot(cape_box());
            push_box(
                mesh,
                shell,
                skin::CAPE,
                origin,
                position,
                yaw,
                pivot,
                0.0,
                0.0,
                0.0,
            );
            return;
        }

        for &index in armor_body_parts(slot) {
            let (part, skin_part, pivot, rot, local_yaw) = self.articulated_part(index, pose);
            let shell = ModelBox {
                center: part.center,
                size: part.size + Vec3::splat(2.0 * inflate),
            };
            push_box(
                mesh, shell, skin_part, origin, position, yaw, pivot, 0.0, rot, local_yaw,
            );
        }
    }

    /// Where a held item sits, in world space, for a model standing at
    /// `position` facing `yaw` in `pose`.
    ///
    /// The anchor is the centre of the right arm's bottom face — the fist —
    /// carried through the same pivot-then-yaw chain [`push_box`] uses, so a
    /// held model tracks the arm exactly as the arm swings. The returned yaw is
    /// the model's, since the only articulation a hand inherits is the body
    /// turn; the arm's own pitch comes back separately.
    pub fn hand_anchor(&self, position: Vec3, yaw: f32, pose: &Pose) -> HandAnchor {
        let arm = self.right_arm;
        let pivot = top_pivot(arm);
        // Fist: the far end of the arm, a little inside the cuff.
        let fist = arm.center - Vec3::new(0.0, arm.size.y * 0.5 - 1.0 / 16.0, 0.0);
        let local = rot_x(fist - pivot, pose.right_arm) + pivot;
        HandAnchor {
            position: rot_y(local, yaw) + position,
            yaw,
            pitch: pose.right_arm,
        }
    }

    /// The `index`-th body part (in `ARTICULATION` order) with its base skin
    /// unwrap, joint pivot, pitch, and head-turn (only the head turns).
    fn articulated_part(&self, index: usize, pose: &Pose) -> (ModelBox, SkinPart, Vec3, f32, f32) {
        match index {
            0 => (
                self.head,
                skin::HEAD,
                bottom_pivot(self.head),
                pose.head_pitch,
                pose.head_yaw,
            ),
            1 => (self.body, skin::BODY, self.body.center, 0.0, 0.0),
            2 => (
                self.left_arm,
                skin::LEFT_ARM,
                top_pivot(self.left_arm),
                pose.left_arm,
                0.0,
            ),
            3 => (
                self.right_arm,
                skin::RIGHT_ARM,
                top_pivot(self.right_arm),
                pose.right_arm,
                0.0,
            ),
            4 => (
                self.left_leg,
                skin::LEFT_LEG,
                top_pivot(self.left_leg),
                pose.left_leg,
                0.0,
            ),
            _ => (
                self.right_leg,
                skin::RIGHT_LEG,
                top_pivot(self.right_leg),
                pose.right_leg,
                0.0,
            ),
        }
    }
}

/// A four-legged box model (cow, sheep): a horizontal body slab on four leg
/// posts with a head at the front (-Z). Proportions come from the kind's
/// `[entity.visual]` data ([`QuadrupedVisual`]); textures from a mob skin
/// sheet ([`crate::art::mobskin`]'s quadruped unwrap).
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
    /// legs the hind pair, so [`super::AnimationState`]'s anti-phase arm/leg
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

/// The cape's model box: a thin, tall slab hung off the shoulders, back face
/// flush behind the body (which sits at `z = +2px`, since the model faces −Z).
fn cape_box() -> ModelBox {
    ModelBox {
        center: Vec3::new(0.0, 16.0, 2.5) / 16.0,
        size: Vec3::new(10.0, 16.0, 1.0) / 16.0,
    }
}

/// Map an inventory armor slot to its render sheet.
fn armor_kind(slot: ArmorSlot) -> ArmorKind {
    match slot {
        ArmorSlot::Helmet => ArmorKind::Helmet,
        ArmorSlot::Chestplate => ArmorKind::Chestplate,
        ArmorSlot::Leggings => ArmorKind::Leggings,
        ArmorSlot::Boots => ArmorKind::Boots,
        ArmorSlot::Glove => ArmorKind::Glove,
        ArmorSlot::Cape => ArmorKind::Cape,
    }
}

/// Which body parts (indices into [`HumanoidModel::articulated_part`]) each slot
/// covers. The cape is handled separately (its own box).
fn armor_body_parts(slot: ArmorSlot) -> &'static [usize] {
    match slot {
        ArmorSlot::Helmet => &[0],
        ArmorSlot::Chestplate => &[1, 2, 3],
        ArmorSlot::Leggings => &[1, 4, 5],
        ArmorSlot::Boots => &[4, 5],
        ArmorSlot::Glove => &[2, 3],
        ArmorSlot::Cape => &[],
    }
}

/// Per-side shell inflation (in pixels) so overlapping pieces don't z-fight and
/// all sit outside the skin overlay (0.25 limbs / 0.5 hat). Bigger = further out.
fn armor_inflation(slot: ArmorSlot) -> f32 {
    match slot {
        ArmorSlot::Leggings => 0.75,
        ArmorSlot::Helmet | ArmorSlot::Chestplate => 1.0,
        ArmorSlot::Boots | ArmorSlot::Glove => 1.25,
        ArmorSlot::Cape => 0.5,
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

/// Rotate a point around the X axis (limb swing / head tilt).
fn rot_x(p: Vec3, a: f32) -> Vec3 {
    let (s, c) = a.sin_cos();
    Vec3::new(p.x, p.y * c - p.z * s, p.y * s + p.z * c)
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
    let half = part.size * 0.5;
    let lo = part.center - half;
    let hi = part.center + half;

    for dir in Direction::ALL {
        let corners = box_face_corners(dir, lo, hi);
        let uv = face_local_uv(dir);
        // local_yaw and the global yaw are both about Y, so they compose for the
        // (translation-free) normal; positions rotate about their own centres.
        // `tilt` is how the part is *built* — a quadruped's body is drawn
        // upright and laid on its side — and `rot` is what the animation does to
        // it this frame. Both turn the geometry; only the tilt is shaded for, or
        // a leg swinging past 45° would pop between two shades mid-stride.
        let normal = rot_y(rot_x(dir.normal(), tilt + rot), local_yaw + yaw).to_array();
        let ao = shade_for(rot_x(dir.normal(), tilt));
        let rect = skin_part.face_rect(dir);
        let quad = std::array::from_fn(|i| {
            let local = rot_y(rot_x(corners[i] - pivot, tilt + rot), local_yaw) + pivot;
            let world = rot_y(local, yaw) + origin;
            ChunkVertex {
                position: world.to_array(),
                normal,
                uv: skin::sheet_uv(sheet_origin, rect, uv[i]),
                ao,
                flags: 0,
                // Skin sheets live in the atlas, not the block texture array.
                layer: 0,
                tint: NO_TINT,
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

    /// The shoulder is the arm's pivot, so a *positive* angle carries the fist
    /// toward the model's front (-Z). Every arm pose is written against this
    /// sign — the walk swing, the one-shot mining swing, the zombie shamble —
    /// so it is worth pinning where the sign is actually applied.
    #[test]
    fn a_positive_arm_angle_puts_the_fist_in_front() {
        let model = HumanoidModel::player();
        let rest = model.hand_anchor(Vec3::ZERO, 0.0, &Pose::default());
        let raised = model.hand_anchor(
            Vec3::ZERO,
            0.0,
            &Pose {
                right_arm: 1.4,
                ..Default::default()
            },
        );
        assert!(
            raised.position.z < rest.position.z - 0.2,
            "a positive angle should reach forward (-Z): rest {}, raised {}",
            rest.position.z,
            raised.position.z
        );
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
    fn armor_adds_one_box_per_covered_part() {
        let blocks = crate::world::block::BlockRegistry::with_builtins();
        let items = crate::inventory::ItemRegistry::from_blocks(&blocks);
        let model = HumanoidModel::player();
        let bare = model
            .build_mesh(Vec3::ZERO, 0.0, &Pose::default())
            .vertices
            .len();

        // A chestplate covers body + both arms: three extra boxes, 24 verts each.
        let mut armor = [None; ARMOR_SIZE];
        armor[ArmorSlot::Chestplate.index()] = items.find("copper_chestplate");
        let mesh = model.build_mesh_armored(Vec3::ZERO, 0.0, &Pose::default(), &armor, &items);
        assert_eq!(mesh.vertices.len(), bare + 3 * 24, "chestplate = 3 boxes");

        // A helmet adds one box; the cape adds its own standalone box.
        let mut armor = [None; ARMOR_SIZE];
        armor[ArmorSlot::Helmet.index()] = items.find("copper_helmet");
        armor[ArmorSlot::Cape.index()] = items.find("cape");
        let mesh = model.build_mesh_armored(Vec3::ZERO, 0.0, &Pose::default(), &armor, &items);
        assert_eq!(
            mesh.vertices.len(),
            bare + 2 * 24,
            "helmet + cape = 2 boxes"
        );

        // An item that isn't the slot's armor is ignored.
        let mut armor = [None; ARMOR_SIZE];
        armor[ArmorSlot::Helmet.index()] = items.find("stone");
        let mesh = model.build_mesh_armored(Vec3::ZERO, 0.0, &Pose::default(), &armor, &items);
        assert_eq!(mesh.vertices.len(), bare, "a non-armor item equips nothing");
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
    fn quadruped_builds_six_boxes_grounded_at_the_feet() {
        let visual = QuadrupedVisual {
            skin: "cow".into(),
            body: [12.0, 10.0, 18.0],
            head: [8.0, 8.0, 6.0],
            leg: [4.0, 12.0, 4.0],
            head_uv: [0, 0],
            body_uv: [18, 4],
            leg_uv: [0, 16],
        };
        let model = QuadrupedModel::new(&visual);
        let mesh = model.build_mesh(Vec3::ZERO, 0.0, &Pose::default(), [0, 12]);
        // 6 boxes (body, head, 4 legs) × 6 faces × 4 vertices, no overlays.
        assert_eq!(mesh.vertices.len(), 144);
        // Feet on the ground; the body slab overlaps the leg tops.
        let min_y = mesh
            .vertices
            .iter()
            .fold(f32::MAX, |lo, v| lo.min(v.position[1]));
        assert!(min_y.abs() < 1e-6, "legs stand on the origin: {min_y}");
        let px = 1.0 / 16.0;
        let leg_top = 12.0 * px;
        let body_bottom = model.body.center.y - model.body.size.y * 0.5;
        assert!(body_bottom < leg_top, "body overlaps the legs");
        // Head is forward of the body (model faces -Z).
        assert!(model.head.center.z < model.body.center.z - model.body.size.z * 0.4);
        // Legs at the four corners: two forward, two back, mirrored in X.
        let (front, hind): (Vec<&ModelBox>, Vec<&ModelBox>) =
            model.legs.iter().partition(|l| l.center.z < 0.0);
        assert_eq!(front.len(), 2);
        assert_eq!(hind.len(), 2);
        assert!(front.iter().any(|l| l.center.x < 0.0) && front.iter().any(|l| l.center.x > 0.0));
    }

    /// The body's unwrap is drawn as an upright box and the model tips it over,
    /// so which drawn face ends up as the animal's *back* is a property of that
    /// turn. Pinned against the cow's own art: the sheet rect at (50, 14) is the
    /// brown spine, (28, 14) the pale belly with the udder. Getting the turn
    /// backwards renders a cow inside out, and nothing else would catch it.
    #[test]
    fn a_quadrupeds_back_comes_from_the_upright_boxs_back_face() {
        let visual = QuadrupedVisual {
            skin: "cow".into(),
            body: [12.0, 10.0, 18.0],
            head: [8.0, 8.0, 6.0],
            leg: [4.0, 12.0, 4.0],
            head_uv: [0, 0],
            body_uv: [18, 4],
            leg_uv: [0, 16],
        };
        let origin = [0, 12];
        let mesh =
            QuadrupedModel::new(&visual).build_mesh(Vec3::ZERO, 0.0, &Pose::default(), origin);

        // Which sheet rect does the face pointing `n` sample?
        let sheet_x = |uv: [f32; 2]| {
            uv[0] * wyven_render::texture::ATLAS_SIZE as f32
                - (origin[0] * wyven_render::texture::TILE_SIZE) as f32
        };
        let face_span = |ny: f32| {
            // The body is the first box pushed: six faces, four vertices each.
            let quad = mesh.vertices[..24]
                .chunks(4)
                .find(|q| (q[0].normal[1] - ny).abs() < 1e-5)
                .unwrap_or_else(|| panic!("no body face with normal y = {ny}"));
            let xs: Vec<f32> = quad.iter().map(|v| sheet_x(v.uv)).collect();
            (
                xs.iter().cloned().fold(f32::MAX, f32::min),
                xs.iter().cloned().fold(f32::MIN, f32::max),
            )
        };

        let (top_lo, top_hi) = face_span(1.0);
        assert!(
            (top_lo - 50.0).abs() < 0.5 && (top_hi - 62.0).abs() < 0.5,
            "the back should read the sheet at x 50..62, got {top_lo}..{top_hi}"
        );
        let (belly_lo, belly_hi) = face_span(-1.0);
        assert!(
            (belly_lo - 28.0).abs() < 0.5 && (belly_hi - 40.0).abs() < 0.5,
            "the belly should read the sheet at x 28..40, got {belly_lo}..{belly_hi}"
        );
    }

    /// A tipped face must be lit as the face it has become: the cow's back is a
    /// top, not a flank, even though it was drawn as the box's back.
    #[test]
    fn a_tipped_face_is_shaded_as_where_it_points() {
        assert_eq!(shade_for(Vec3::Y), 1.0);
        assert_eq!(shade_for(rot_x(Vec3::Z, -std::f32::consts::FRAC_PI_2)), 1.0);
        assert_eq!(
            shade_for(rot_x(Vec3::NEG_Z, -std::f32::consts::FRAC_PI_2)),
            0.68
        );
        // Animation never reaches here — a swinging limb keeps its authored
        // shade however far it swings.
        assert_eq!(shade_for(Vec3::NEG_Y), 0.68);
    }

    #[test]
    fn quadruped_legs_swing_about_their_hips() {
        let visual = QuadrupedVisual {
            skin: "sheep".into(),
            body: [8.0, 6.0, 16.0],
            head: [6.0, 6.0, 8.0],
            leg: [4.0, 12.0, 4.0],
            head_uv: [0, 0],
            body_uv: [28, 8],
            leg_uv: [0, 16],
        };
        let model = QuadrupedModel::new(&visual);
        let rest = model.build_mesh(Vec3::ZERO, 0.0, &Pose::default(), [4, 12]);
        let swung = model.build_mesh(
            Vec3::ZERO,
            0.0,
            &Pose {
                left_arm: 0.8,
                ..Default::default()
            },
            [4, 12],
        );
        // Body + head (first two boxes, 48 verts) are unaffected...
        for (a, b) in rest.vertices[..48].iter().zip(&swung.vertices[..48]) {
            assert_eq!(a.position, b.position);
        }
        // ...while the front-left leg (third box) moved.
        assert!(
            rest.vertices[48..72]
                .iter()
                .zip(&swung.vertices[48..72])
                .any(|(a, b)| a.position != b.position),
            "front-left leg should swing with the left_arm channel"
        );
    }

    #[test]
    fn humanoid_sheet_origin_shifts_the_uvs() {
        let model = HumanoidModel::player();
        let default_sheet = model.build_mesh(Vec3::ZERO, 0.0, &Pose::default());
        let mob_sheet = model.build_mesh_sheet(Vec3::ZERO, 0.0, &Pose::default(), [12, 4]);
        assert_eq!(default_sheet.vertices.len(), mob_sheet.vertices.len());
        // Identical geometry, different texture region.
        for (a, b) in default_sheet.vertices.iter().zip(&mob_sheet.vertices) {
            assert_eq!(a.position, b.position);
        }
        assert!(
            default_sheet
                .vertices
                .iter()
                .zip(&mob_sheet.vertices)
                .any(|(a, b)| a.uv != b.uv),
            "a different sheet origin must move the UVs"
        );
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

    #[test]
    fn hand_anchor_sits_at_the_end_of_the_right_arm() {
        let model = HumanoidModel::player();
        let at = Vec3::new(10.0, 64.0, -3.0);
        let anchor = model.hand_anchor(at, 0.0, &Pose::default());

        // On the character's right (+X at yaw 0, see `HumanoidModel::player`),
        // and down near the cuff rather than up at the shoulder.
        let local = anchor.position - at;
        assert!(local.x > 0.0, "hand should be on the right, got {local}");
        let shoulder = top_pivot(model.right_arm).y;
        let cuff = model.right_arm.center.y - model.right_arm.size.y * 0.5;
        assert!(
            local.y < shoulder && local.y < cuff + 2.0 / 16.0,
            "hand at {local} should be near the cuff ({cuff}), not the shoulder ({shoulder})"
        );
        assert_eq!(anchor.yaw, 0.0);
        assert_eq!(anchor.pitch, 0.0, "rest pose has no arm swing");
    }

    #[test]
    fn hand_anchor_follows_the_arm_swing_and_the_body_yaw() {
        let model = HumanoidModel::player();
        let rest = model.hand_anchor(Vec3::ZERO, 0.0, &Pose::default());

        // Swinging the arm forward carries the hand with it, about the shoulder.
        let swung = model.hand_anchor(
            Vec3::ZERO,
            0.0,
            &Pose {
                right_arm: -1.2,
                ..Pose::default()
            },
        );
        assert!(
            (swung.position.z - rest.position.z).abs() > 0.2,
            "hand should swing along Z, moved to {}",
            swung.position
        );
        assert!(swung.position.y > rest.position.y, "and lift as it swings");
        assert_eq!(swung.pitch, -1.2, "the held model inherits the arm pitch");

        // Turning the body carries the hand around, keeping it the same
        // distance from the model's centre line.
        let turned = model.hand_anchor(Vec3::ZERO, std::f32::consts::FRAC_PI_2, &Pose::default());
        assert!(
            (turned.position.length() - rest.position.length()).abs() < 1e-5,
            "yaw must not move the hand relative to the body"
        );
        assert!(
            turned.position.z > 0.1 && turned.position.x.abs() < 1e-5,
            "a quarter turn should send the right hand to +Z, got {}",
            turned.position
        );
    }
}
