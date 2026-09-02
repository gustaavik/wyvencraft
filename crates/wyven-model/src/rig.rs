//! Bones, poses and the skeleton a rigged model file describes.
//!
//! A `.bbmodel` groups its cuboids into a nested outliner. [`super::bbmodel`]
//! has always *walked* that hierarchy — folding each group's pivoted rotation
//! into the vertex positions — but then threw it away, leaving one flat
//! [`ModelMesh`](super::mesh::ModelMesh). That is enough to draw a sword and not
//! enough to bend an elbow.
//!
//! This module keeps what the walk discovers: which joint owns which vertices,
//! and where that joint pivots. Geometry still lives in the same flat mesh —
//! a [`Rig`] only says who owns what — so a rigged model and a flat one are the
//! same type downstream.
//!
//! **The rest pose is the anchor.** The mesh is baked exactly as before, with
//! every authored rest rotation already in the vertex positions, and a bone
//! matrix carries only the animation's *delta* from that. So a [`Pose::rest`]
//! bake is vertex-for-vertex what a flat model produces, which is what lets
//! every model that was loading before this module existed carry on unchanged.
//!
//! Posing happens on the CPU, once per frame per entity, because the renderer
//! has no model matrix — the same reason the box models bake their transforms.

use std::collections::HashMap;

use glam::{EulerRot, Mat4, Quat, Vec3};

use super::clip::Clip;

/// Index of a bone in a [`Rig`]. Cheap to copy and to store on per-entity state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BoneId(pub u16);

impl BoneId {
    fn index(self) -> usize {
        self.0 as usize
    }
}

/// One joint: what it is called, what it hangs off, and where it turns about.
#[derive(Debug, Clone)]
pub struct Bone {
    pub name: String,
    pub parent: Option<BoneId>,
    /// Pivot in model space (blocks), with every ancestor's *rest* rotation
    /// already applied. Storing it post-rest is what makes an identity pose
    /// produce an identity matrix instead of something merely very close.
    pub pivot: Vec3,
}

/// The vertices one bone owns: a half-open range into the rest mesh's parallel
/// arrays. `bone` is `None` for geometry the file left outside every group,
/// which then never moves.
#[derive(Debug, Clone, Copy)]
pub struct BonePart {
    pub bone: Option<BoneId>,
    pub start: u32,
    pub end: u32,
}

/// One bone's offset *from its rest pose* — never its absolute placement.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BoneTransform {
    /// Radians about the bone's pivot, in the same XYZ order the loader uses
    /// for authored rotations.
    pub rotation: Vec3,
    /// Blocks. Shifts the pivot itself, which is what a Blockbench `position`
    /// keyframe means — a bone that moves carries its children with it.
    pub position: Vec3,
}

impl BoneTransform {
    pub const REST: Self = Self {
        rotation: Vec3::ZERO,
        position: Vec3::ZERO,
    };

    /// Straight-line blend. Euler angles rather than quaternions because these
    /// are joint offsets of a few tens of degrees on one axis at a time, where
    /// the two agree and the cheap one is easier to reason about.
    pub fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            rotation: self.rotation.lerp(other.rotation, t),
            position: self.position.lerp(other.position, t),
        }
    }
}

/// Per-bone offsets from the rest pose, indexed by [`BoneId`].
///
/// Deliberately a plain value: clips write into one, procedural layers add to
/// one, and blending two is a lerp. Nothing here knows what any bone is *for*.
#[derive(Debug, Clone, PartialEq)]
pub struct Pose {
    bones: Vec<BoneTransform>,
}

impl Pose {
    /// Every bone at rest — the pose a model with no animation is drawn in.
    pub fn rest(rig: &Rig) -> Self {
        Self {
            bones: vec![BoneTransform::REST; rig.bones.len()],
        }
    }

    pub fn len(&self) -> usize {
        self.bones.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bones.is_empty()
    }

    /// This bone's offset, or the rest offset for an id from another rig.
    pub fn get(&self, bone: BoneId) -> BoneTransform {
        self.bones
            .get(bone.index())
            .copied()
            .unwrap_or(BoneTransform::REST)
    }

    pub fn set(&mut self, bone: BoneId, transform: BoneTransform) {
        if let Some(slot) = self.bones.get_mut(bone.index()) {
            *slot = transform;
        }
    }

    /// Add to what is already there. This is the additive layer a head turn or
    /// an attack swing rides in on, *after* a clip has written the base pose.
    pub fn rotate(&mut self, bone: BoneId, delta: Vec3) {
        if let Some(slot) = self.bones.get_mut(bone.index()) {
            slot.rotation += delta;
        }
    }

    pub fn translate(&mut self, bone: BoneId, delta: Vec3) {
        if let Some(slot) = self.bones.get_mut(bone.index()) {
            slot.position += delta;
        }
    }

    /// Blend every bone toward `other`. `t = 0` keeps this pose, `t = 1` takes
    /// the other — the idle↔walk↔run crossfade.
    pub fn blend(&mut self, other: &Pose, t: f32) {
        for (i, slot) in self.bones.iter_mut().enumerate() {
            *slot = slot.lerp(other.bones.get(i).copied().unwrap_or_default(), t);
        }
    }
}

/// A model's skeleton and the clips authored against it.
pub struct Rig {
    /// Parents always come before their children, so one forward pass resolves
    /// every matrix.
    bones: Vec<Bone>,
    parts: Vec<BonePart>,
    clips: Vec<Clip>,
    by_name: HashMap<String, BoneId>,
}

impl Rig {
    pub fn bones(&self) -> &[Bone] {
        &self.bones
    }

    pub fn bone_count(&self) -> usize {
        self.bones.len()
    }

    /// Look a bone up by the name its author gave it. Callers resolve names
    /// once, at load, and carry [`BoneId`]s per frame.
    pub fn bone(&self, name: &str) -> Option<BoneId> {
        self.by_name.get(name).copied()
    }

    pub fn name(&self, bone: BoneId) -> &str {
        self.bones
            .get(bone.index())
            .map(|b| b.name.as_str())
            .unwrap_or("")
    }

    pub fn pivot(&self, bone: BoneId) -> Vec3 {
        self.bones
            .get(bone.index())
            .map(|b| b.pivot)
            .unwrap_or(Vec3::ZERO)
    }

    pub fn parts(&self) -> &[BonePart] {
        &self.parts
    }

    pub fn clips(&self) -> &[Clip] {
        &self.clips
    }

    pub fn clip(&self, name: &str) -> Option<&Clip> {
        self.clips.iter().find(|c| c.name == name)
    }

    /// Resolve a clip name to an index once, so a per-frame path can hold the
    /// index instead of comparing strings every time it draws.
    pub fn clip_index(&self, name: &str) -> Option<usize> {
        self.clips.iter().position(|c| c.name == name)
    }

    pub fn clip_at(&self, index: usize) -> Option<&Clip> {
        self.clips.get(index)
    }

    /// `root` and everything hanging off it. Bones are stored parents-first, so
    /// one forward pass is enough — this is how a caller selects a limb (the
    /// first-person arm) out of a whole body.
    pub fn subtree(&self, root: BoneId) -> Vec<BoneId> {
        let mut inside = vec![false; self.bones.len()];
        let mut out = Vec::new();
        for (i, bone) in self.bones.iter().enumerate() {
            let included = i == root.index()
                || bone
                    .parent
                    .is_some_and(|p| inside.get(p.index()).copied().unwrap_or(false));
            inside[i] = included;
            if included {
                out.push(BoneId(i as u16));
            }
        }
        out
    }

    /// One model→model matrix per bone under `pose`.
    ///
    /// A bone that is exactly at rest contributes the identity rather than a
    /// rotation of zero degrees, so the rest pose is bit-identical to no pose
    /// at all and a flat bake can never drift from a posed one.
    pub fn matrices(&self, pose: &Pose) -> Vec<Mat4> {
        let mut out: Vec<Mat4> = Vec::with_capacity(self.bones.len());
        for (i, bone) in self.bones.iter().enumerate() {
            let parent = bone
                .parent
                .and_then(|p| out.get(p.index()).copied())
                .unwrap_or(Mat4::IDENTITY);
            let local = local_matrix(bone.pivot, pose.get(BoneId(i as u16)));
            out.push(parent * local);
        }
        out
    }
}

/// `translate(pivot + move) · rotate · translate(-pivot)` — the same pivoted
/// composition [`super::bbmodel::Transform`] uses for authored rotations, which
/// is why an animation and a rest rotation stack without special cases.
fn local_matrix(pivot: Vec3, transform: BoneTransform) -> Mat4 {
    if transform == BoneTransform::REST {
        return Mat4::IDENTITY;
    }
    let rotation = Mat4::from_quat(Quat::from_euler(
        EulerRot::XYZ,
        transform.rotation.x,
        transform.rotation.y,
        transform.rotation.z,
    ));
    Mat4::from_translation(pivot + transform.position) * rotation * Mat4::from_translation(-pivot)
}

/// Accumulates a [`Rig`] while a loader walks a file's hierarchy.
///
/// Separate from `Rig` so the finished rig is immutable, and so the walk does
/// not have to know the final bone count up front.
#[derive(Default)]
pub struct RigBuilder {
    bones: Vec<Bone>,
    parts: Vec<BonePart>,
    by_name: HashMap<String, BoneId>,
}

impl RigBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare a bone. Must be called before any of its children, which a
    /// depth-first walk gives for free.
    pub fn push_bone(&mut self, name: &str, parent: Option<BoneId>, pivot: Vec3) -> BoneId {
        let id = BoneId(self.bones.len() as u16);
        self.bones.push(Bone {
            name: name.to_string(),
            parent,
            pivot,
        });
        // First name wins: a duplicate would otherwise silently steal every
        // lookup its twin was meant to answer.
        self.by_name.entry(name.to_string()).or_insert(id);
        id
    }

    /// Attribute the vertex range `start..end` to `bone` (`None` = static).
    /// Empty ranges are dropped rather than stored.
    pub fn attach(&mut self, bone: Option<BoneId>, start: u32, end: u32) {
        if end > start {
            self.parts.push(BonePart { bone, start, end });
        }
    }

    pub fn is_empty(&self) -> bool {
        self.bones.is_empty()
    }

    pub fn build(self, clips: Vec<Clip>) -> Rig {
        Rig {
            bones: self.bones,
            parts: self.parts,
            clips,
            by_name: self.by_name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-link chain: `upper` at the origin, `lower` pivoting a block below.
    fn chain() -> Rig {
        let mut builder = RigBuilder::new();
        let upper = builder.push_bone("upper", None, Vec3::ZERO);
        let lower = builder.push_bone("lower", Some(upper), Vec3::new(0.0, -1.0, 0.0));
        builder.attach(Some(upper), 0, 4);
        builder.attach(Some(lower), 4, 8);
        builder.build(Vec::new())
    }

    #[test]
    fn the_rest_pose_is_the_identity_on_every_bone() {
        let rig = chain();
        for matrix in rig.matrices(&Pose::rest(&rig)) {
            assert_eq!(matrix, Mat4::IDENTITY, "rest must not move anything at all");
        }
    }

    #[test]
    fn a_parent_carries_its_children() {
        let rig = chain();
        let mut pose = Pose::rest(&rig);
        pose.rotate(
            rig.bone("upper").unwrap(),
            Vec3::new(std::f32::consts::FRAC_PI_2, 0.0, 0.0),
        );
        let matrices = rig.matrices(&pose);
        // The lower bone declares no rotation of its own, yet its pivot has
        // swung a quarter turn about the upper bone's.
        let moved = matrices[1].transform_point3(Vec3::new(0.0, -1.0, 0.0));
        assert!(moved.y.abs() < 1e-5, "y should have rotated away: {moved}");
        assert!(
            (moved.z - -1.0).abs() < 1e-5,
            "should point along -Z: {moved}"
        );
    }

    #[test]
    fn a_bone_turns_about_its_own_pivot() {
        let rig = chain();
        let mut pose = Pose::rest(&rig);
        pose.rotate(
            rig.bone("lower").unwrap(),
            Vec3::new(std::f32::consts::FRAC_PI_2, 0.0, 0.0),
        );
        let matrices = rig.matrices(&pose);
        let pivot = Vec3::new(0.0, -1.0, 0.0);
        let stayed = matrices[1].transform_point3(pivot);
        assert!(
            (stayed - pivot).length() < 1e-5,
            "the pivot itself must not move"
        );
    }

    #[test]
    fn a_position_offset_moves_the_bone_and_its_children() {
        let rig = chain();
        let mut pose = Pose::rest(&rig);
        pose.translate(rig.bone("upper").unwrap(), Vec3::new(0.0, -0.5, 0.0));
        let matrices = rig.matrices(&pose);
        let moved = matrices[1].transform_point3(Vec3::ZERO);
        assert!(
            (moved.y - -0.5).abs() < 1e-5,
            "the child rode along: {moved}"
        );
    }

    #[test]
    fn a_subtree_is_the_bone_and_its_descendants() {
        let rig = chain();
        let upper = rig.bone("upper").unwrap();
        let lower = rig.bone("lower").unwrap();
        assert_eq!(rig.subtree(upper), vec![upper, lower]);
        assert_eq!(rig.subtree(lower), vec![lower], "a leaf is only itself");
    }

    #[test]
    fn blending_walks_from_one_pose_to_the_other() {
        let rig = chain();
        let upper = rig.bone("upper").unwrap();
        let rest = Pose::rest(&rig);
        let mut turned = Pose::rest(&rig);
        turned.rotate(upper, Vec3::new(1.0, 0.0, 0.0));

        let mut half = rest.clone();
        half.blend(&turned, 0.5);
        assert!((half.get(upper).rotation.x - 0.5).abs() < 1e-6);

        let mut none = rest.clone();
        none.blend(&turned, 0.0);
        assert_eq!(none, rest, "a zero blend must change nothing");
    }

    #[test]
    fn an_unknown_bone_name_is_reported_not_guessed() {
        assert_eq!(chain().bone("elbow"), None);
    }
}
