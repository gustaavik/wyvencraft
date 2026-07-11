//! A simple box-part humanoid model (head, body, arms, legs) used to render the
//! player in third person and remote players in multiplayer.
//!
//! Dimensions follow the classic Minecraft proportions (in pixels / 16 = blocks).

use glam::Vec3;

use crate::core::Direction;
use crate::inventory::{ARMOR_SIZE, ArmorSlot, ItemId, ItemRegistry};
use crate::render::armor::ArmorKind;
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
                skin::SKIN_ORIGIN,
                position,
                yaw,
                pivot,
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
                skin::SKIN_ORIGIN,
                position,
                yaw,
                pivot,
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
                mesh, shell, skin_part, origin, position, yaw, pivot, rot, local_yaw,
            );
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
        Direction::NegY => 0.68,
        Direction::PosX | Direction::NegX => 0.86,
        Direction::PosZ | Direction::NegZ => 0.80,
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
        let normal = rot_y(rot_x(dir.normal(), rot), local_yaw + yaw).to_array();
        let ao = face_shade(dir);
        let rect = skin_part.face_rect(dir);
        let quad = std::array::from_fn(|i| {
            let local = rot_y(rot_x(corners[i] - pivot, rot), local_yaw) + pivot;
            let world = rot_y(local, yaw) + origin;
            ChunkVertex {
                position: world.to_array(),
                normal,
                uv: skin::sheet_uv(sheet_origin, rect, uv[i]),
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
        armor[ArmorSlot::Chestplate.index()] = items.find("chestplate");
        let mesh = model.build_mesh_armored(Vec3::ZERO, 0.0, &Pose::default(), &armor, &items);
        assert_eq!(mesh.vertices.len(), bare + 3 * 24, "chestplate = 3 boxes");

        // A helmet adds one box; the cape adds its own standalone box.
        let mut armor = [None; ARMOR_SIZE];
        armor[ArmorSlot::Helmet.index()] = items.find("helmet");
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
