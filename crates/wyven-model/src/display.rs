//! Minecraft's `display` block: where an item sits in each context it is drawn.
//!
//! A Blockbench "Java Block/Item" export carries, alongside its geometry, up to
//! eight named placements — one for the hand in first person, one for the hand
//! in third person, one for the inventory icon, one for the ground, and so on.
//! Each is a `rotation`/`translation`/`scale` triple in the model's own space.
//!
//! This exists because one placement cannot serve every context. A sword needs
//! to lie along the fist in the hand, stand on its point on the ground, and run
//! corner-to-corner in a square inventory slot; before this, `ModelSpec` carried
//! a single `scale`/`offset`/`rotation` that had to compromise between all
//! three. A model that declares nothing here keeps that compromise — see
//! [`crate::mesh::local_transform`], which is the one place the choice is made.
//!
//! Boundaries: pure data plus one matrix. Nothing here knows what an item is.

use glam::{EulerRot, Mat4, Vec3};

/// How far a `translation` may reach, in blocks. Minecraft clamps to this, and
/// a model with a runaway number should be visibly wrong rather than able to
/// place itself across the world.
const MAX_TRANSLATION: f32 = 5.0;

/// Sixteenths of a block, the unit `translation` is authored in.
const PIXELS_PER_BLOCK: f32 = 16.0;

/// One `display` entry: where the model sits in one context, in its own space.
///
/// The defaults are the identity, so a partial entry — `{"scale": [0,0,0]}`,
/// which is how an author hides a model in one context — reads correctly.
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ItemTransform {
    /// Degrees about the model's own X, Y and Z, applied in that order.
    pub rotation: [f32; 3],
    /// Shift in sixteenths of a block, clamped to ±[`MAX_TRANSLATION`] blocks.
    pub translation: [f32; 3],
    /// Per-axis scale. Not uniform, unlike [`crate::ModelSpec::scale`].
    pub scale: [f32; 3],
}

impl Default for ItemTransform {
    fn default() -> Self {
        Self {
            rotation: [0.0; 3],
            translation: [0.0; 3],
            scale: [1.0; 3],
        }
    }
}

impl ItemTransform {
    /// The model→context transform this entry describes.
    ///
    /// Minecraft applies the triple about the *centre* of the block a model is
    /// authored in, which is why the model is shifted by `-0.5` on every axis
    /// first and the translation lands the centre exactly where it asks. The
    /// rotation order is the one Minecraft's `rotationXYZ` means: X, then Y,
    /// then Z, each about the axes the previous rotation left behind — which is
    /// what glam spells [`EulerRot::XYZ`].
    pub fn matrix(&self) -> Mat4 {
        let translation = (Vec3::from(self.translation) / PIXELS_PER_BLOCK)
            .clamp(Vec3::splat(-MAX_TRANSLATION), Vec3::splat(MAX_TRANSLATION));
        let rotation = Vec3::from(self.rotation.map(f32::to_radians));
        Mat4::from_translation(translation)
            * Mat4::from_euler(EulerRot::XYZ, rotation.x, rotation.y, rotation.z)
            * Mat4::from_scale(Vec3::from(self.scale))
            * Mat4::from_translation(Vec3::splat(-0.5))
    }

    /// Whether this entry scales the model away to nothing — Minecraft's way of
    /// saying "do not draw me here" (`"head": {"scale": [0, 0, 0]}`).
    pub fn is_hidden(&self) -> bool {
        self.scale.iter().any(|s| s.abs() <= f32::EPSILON)
    }
}

/// Which placement of a model is wanted.
///
/// Minecraft names a left-hand variant of each hand context too; we have no
/// off-hand, so those are parsed and ignored rather than given a variant that
/// nothing could ever ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DisplayContext {
    /// The viewmodel: the item in your own hand, seen down the camera.
    FirstPersonRightHand,
    /// The item in a player model's fist, seen from outside.
    ThirdPersonRightHand,
    /// The inventory icon.
    Gui,
    /// Lying on the floor as a dropped item.
    Ground,
    /// Mounted flat, as in an item frame.
    Fixed,
    /// Worn on the head.
    Head,
}

/// Every placement a model declares. Absent entries fall back to the data
/// file's own [`crate::ModelSpec`] numbers, which is what every model authored
/// before this feature — every `.bbmodel` and `.gltf` — still uses.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct DisplayTransforms {
    pub firstperson_righthand: Option<ItemTransform>,
    pub thirdperson_righthand: Option<ItemTransform>,
    pub gui: Option<ItemTransform>,
    pub ground: Option<ItemTransform>,
    pub fixed: Option<ItemTransform>,
    pub head: Option<ItemTransform>,
}

impl DisplayTransforms {
    /// The placement for `context`, or `None` when the model does not declare
    /// one — including when it declares one scaled away to nothing, which means
    /// the same thing to every caller here.
    pub fn get(&self, context: DisplayContext) -> Option<ItemTransform> {
        let entry = match context {
            DisplayContext::FirstPersonRightHand => self.firstperson_righthand,
            DisplayContext::ThirdPersonRightHand => self.thirdperson_righthand,
            DisplayContext::Gui => self.gui,
            DisplayContext::Ground => self.ground,
            DisplayContext::Fixed => self.fixed,
            DisplayContext::Head => self.head,
        };
        entry.filter(|t| !t.is_hidden())
    }

    /// Whether the model declares no placement at all.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: Vec3, b: Vec3) -> bool {
        (a - b).abs().max_element() < 1e-4
    }

    #[test]
    fn an_identity_transform_only_recentres_the_model() {
        let m = ItemTransform::default().matrix();
        assert!(approx(m.transform_point3(Vec3::splat(0.5)), Vec3::ZERO));
    }

    #[test]
    fn the_model_centre_lands_on_the_translation() {
        // Whatever the rotation and scale, the point the transform turns about
        // is the model's centre, so it can only ever end up at `translation`.
        let t = ItemTransform {
            rotation: [-99.9, 87.78, 95.45],
            translation: [0.0, 1.0, 1.0],
            scale: [0.79883; 3],
        };
        let centre = t.matrix().transform_point3(Vec3::splat(0.5));
        assert!(approx(centre, Vec3::new(0.0, 1.0 / 16.0, 1.0 / 16.0)));
    }

    #[test]
    fn rotation_is_x_then_y_then_z_about_the_moving_axes() {
        // A quarter turn about Y alone sends the model's +X to -Z; pinned
        // because glam's `EulerRot::XYZ` is intrinsic, matching Minecraft's
        // `rotationXYZ`, and an extrinsic reading would turn the other way.
        let t = ItemTransform {
            rotation: [0.0, 90.0, 0.0],
            ..Default::default()
        };
        let m = t.matrix();
        let along_x = m.transform_point3(Vec3::new(1.5, 0.5, 0.5));
        assert!(approx(along_x, Vec3::new(0.0, 0.0, -1.0)), "{along_x}");
    }

    #[test]
    fn the_three_axes_compose_in_order() {
        // 90° about X, then 90° about the Y the first rotation left behind:
        // +X swings to -Z, and the tilted turn lifts that to +Y. Taking the
        // axes in the other order would leave it at -Z, so this pins the order
        // and not merely the handedness.
        let t = ItemTransform {
            rotation: [90.0, 90.0, 0.0],
            ..Default::default()
        };
        let along_x = t.matrix().transform_point3(Vec3::new(1.5, 0.5, 0.5));
        assert!(approx(along_x, Vec3::new(0.0, 1.0, 0.0)), "{along_x}");
    }

    #[test]
    fn a_translation_cannot_reach_past_five_blocks() {
        let t = ItemTransform {
            translation: [1000.0, 0.0, 0.0],
            ..Default::default()
        };
        let centre = t.matrix().transform_point3(Vec3::splat(0.5));
        assert!(approx(centre, Vec3::new(MAX_TRANSLATION, 0.0, 0.0)));
    }

    #[test]
    fn a_context_scaled_to_nothing_reads_as_absent() {
        let display = DisplayTransforms {
            head: Some(ItemTransform {
                scale: [0.0; 3],
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(display.get(DisplayContext::Head), None);
    }

    #[test]
    fn an_undeclared_context_is_absent() {
        let display = DisplayTransforms::default();
        assert!(display.is_empty());
        for context in [
            DisplayContext::FirstPersonRightHand,
            DisplayContext::ThirdPersonRightHand,
            DisplayContext::Gui,
            DisplayContext::Ground,
            DisplayContext::Fixed,
            DisplayContext::Head,
        ] {
            assert_eq!(display.get(context), None);
        }
    }

    #[test]
    fn the_left_hand_entries_parse_without_claiming_a_context() {
        // Minecraft writes both hands; we have no off-hand, so the left-hand
        // keys must not make the parse fail either.
        let display: DisplayTransforms = serde_json::from_str(
            r#"{"firstperson_lefthand": {"rotation": [1, 2, 3]},
                "firstperson_righthand": {"translation": [0, 1, 1]}}"#,
        )
        .expect("parse");
        assert_eq!(
            display.get(DisplayContext::FirstPersonRightHand),
            Some(ItemTransform {
                translation: [0.0, 1.0, 1.0],
                ..Default::default()
            })
        );
    }
}
