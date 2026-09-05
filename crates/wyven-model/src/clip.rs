//! Keyframe animation: what a clip *is*, and how to sample one.
//!
//! Deliberately format-agnostic. Blockbench spells a clip one way and glTF
//! another; both reduce to "per bone, per channel, a list of keyframes", and
//! that is all this module knows. Reading a particular file's spelling belongs
//! to that file's loader ([`super::bbmodel`]), which is also where the unit
//! conversions happen — everything here is already radians and blocks.
//!
//! Nothing here knows what a clip is *for*. A clip is found by the name its
//! author gave it, so the game decides that "walk" means walking and the engine
//! never learns the word.

use glam::Vec3;

use super::rig::{BoneId, Pose};

/// What a keyframe drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Rotation,
    Position,
}

/// How to get from one keyframe to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Interpolation {
    #[default]
    Linear,
    /// Smooth through the neighbouring keyframes — Blockbench's default for
    /// organic motion, and what the cow's clips are authored with.
    CatmullRom,
    /// Hold the value until the next keyframe.
    Step,
}

/// What happens once a clip reaches its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopMode {
    /// Snap back to the rest pose. Distinguished from [`LoopMode::Hold`]
    /// because the caller, not the clip, decides when a one-shot is over.
    Once,
    /// Stay in the last pose.
    Hold,
    #[default]
    Loop,
}

#[derive(Debug, Clone, Copy)]
pub struct Keyframe {
    pub time: f32,
    /// Radians for [`Channel::Rotation`], blocks for [`Channel::Position`].
    pub value: Vec3,
    /// How to leave this keyframe — interpolation describes the segment that
    /// *starts* here, which is the convention every editor's timeline shows.
    pub interpolation: Interpolation,
}

/// One bone's keyframes on one channel.
pub struct Track {
    pub bone: BoneId,
    pub channel: Channel,
    keys: Vec<Keyframe>,
}

impl Track {
    /// Keys are sorted here rather than trusted: a `.bbmodel` stores them in
    /// whatever order the author last touched them, and a binary search over an
    /// unsorted list silently returns the wrong pose rather than failing.
    pub fn new(bone: BoneId, channel: Channel, mut keys: Vec<Keyframe>) -> Self {
        keys.sort_by(|a, b| a.time.total_cmp(&b.time));
        Self {
            bone,
            channel,
            keys,
        }
    }

    pub fn keys(&self) -> &[Keyframe] {
        &self.keys
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The last keyframe's time, or 0 for an empty track.
    pub fn end(&self) -> f32 {
        self.keys.last().map(|k| k.time).unwrap_or(0.0)
    }

    /// The value at `time`, clamped to the track's own ends.
    pub fn sample(&self, time: f32) -> Vec3 {
        let keys = &self.keys;
        let Some(first) = keys.first() else {
            return Vec3::ZERO;
        };
        if time <= first.time || keys.len() == 1 {
            return first.value;
        }
        let last = keys[keys.len() - 1];
        if time >= last.time {
            return last.value;
        }
        // The segment starting at the last key at or before `time`.
        let i = keys.partition_point(|k| k.time <= time) - 1;
        let (a, b) = (keys[i], keys[i + 1]);
        let span = b.time - a.time;
        if span <= f32::EPSILON {
            return b.value;
        }
        let t = (time - a.time) / span;
        match a.interpolation {
            Interpolation::Step => a.value,
            Interpolation::Linear => a.value.lerp(b.value, t),
            Interpolation::CatmullRom => {
                let before = keys[i.saturating_sub(1)].value;
                let after = keys[(i + 2).min(keys.len() - 1)].value;
                catmull_rom(before, a.value, b.value, after, t)
            }
        }
    }
}

/// Uniform Catmull-Rom through `p1`→`p2`, using the neighbours as tangents.
/// Duplicating the endpoint at either end is what keeps the first and last
/// segments from flying off toward a control point that does not exist.
fn catmull_rom(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f32) -> Vec3 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (p2 - p0) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (p3 - 3.0 * p2 + 3.0 * p1 - p0) * t3)
}

/// One named animation over a rig.
pub struct Clip {
    pub name: String,
    /// Seconds, as authored. A clip whose keyframes end early genuinely holds
    /// its last pose for the remainder — that is what its editor plays back,
    /// and second-guessing it here would make the file and the game disagree.
    pub length: f32,
    pub loop_mode: LoopMode,
    tracks: Vec<Track>,
}

impl Clip {
    pub fn new(name: String, length: f32, loop_mode: LoopMode, tracks: Vec<Track>) -> Self {
        // A zero length would make looping a division by nothing; fall back to
        // the last keyframe so a clip is always playable.
        let end = tracks.iter().map(Track::end).fold(0.0f32, f32::max);
        let length = if length > 0.0 { length } else { end };
        Self {
            name,
            length,
            loop_mode,
            tracks,
        }
    }

    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.iter().all(Track::is_empty)
    }

    /// The last keyframe's time, which may be earlier than [`Self::length`].
    ///
    /// A clip whose keyframes stop early genuinely holds its last pose for the
    /// rest of its length, and playing it back on a wall clock should do that.
    /// A caller driving a clip from something *other* than a clock — a stride
    /// phase, where a frozen tail would be a frozen character — wants the span
    /// that is actually animated, and this is it.
    pub fn end(&self) -> f32 {
        self.tracks.iter().map(Track::end).fold(0.0f32, f32::max)
    }

    /// `time` folded into the clip's own span according to its loop mode.
    pub fn wrap(&self, time: f32) -> f32 {
        if self.length <= 0.0 {
            return 0.0;
        }
        match self.loop_mode {
            LoopMode::Loop => time.rem_euclid(self.length),
            LoopMode::Once | LoopMode::Hold => time.clamp(0.0, self.length),
        }
    }

    /// Whether a one-shot started at time 0 has run out.
    pub fn finished(&self, time: f32) -> bool {
        matches!(self.loop_mode, LoopMode::Once) && time >= self.length
    }

    /// Write this clip's pose at `time` into `out`, leaving bones it does not
    /// animate exactly as it found them — so a caller can start from the rest
    /// pose, or from another clip, and layer.
    pub fn sample(&self, time: f32, out: &mut Pose) {
        let time = self.wrap(time);
        for track in &self.tracks {
            if track.is_empty() {
                continue;
            }
            let value = track.sample(time);
            let mut transform = out.get(track.bone);
            match track.channel {
                Channel::Rotation => transform.rotation = value,
                Channel::Position => transform.position = value,
            }
            out.set(track.bone, transform);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig::{Rig, RigBuilder};

    fn key(time: f32, x: f32, interpolation: Interpolation) -> Keyframe {
        Keyframe {
            time,
            value: Vec3::new(x, 0.0, 0.0),
            interpolation,
        }
    }

    fn one_bone() -> Rig {
        let mut builder = RigBuilder::new();
        builder.push_bone("limb", None, Vec3::ZERO);
        builder.build(Vec::new())
    }

    #[test]
    fn keyframes_are_sorted_however_the_file_stored_them() {
        // A .bbmodel lists keyframes in edit order, not time order.
        let track = Track::new(
            BoneId(0),
            Channel::Rotation,
            vec![
                key(1.0, 10.0, Interpolation::Linear),
                key(0.0, 0.0, Interpolation::Linear),
                key(0.5, 5.0, Interpolation::Linear),
            ],
        );
        let times: Vec<f32> = track.keys().iter().map(|k| k.time).collect();
        assert_eq!(times, vec![0.0, 0.5, 1.0]);
        assert!(
            (track.sample(0.25).x - 2.5).abs() < 1e-5,
            "and it samples right"
        );
    }

    #[test]
    fn sampling_lands_exactly_on_each_keyframe() {
        let track = Track::new(
            BoneId(0),
            Channel::Rotation,
            vec![
                key(0.0, -3.0, Interpolation::CatmullRom),
                key(0.5, 7.0, Interpolation::CatmullRom),
                key(1.0, -3.0, Interpolation::CatmullRom),
            ],
        );
        for (time, expected) in [(0.0, -3.0), (0.5, 7.0), (1.0, -3.0)] {
            assert!(
                (track.sample(time).x - expected).abs() < 1e-4,
                "t={time} gave {}",
                track.sample(time).x
            );
        }
    }

    #[test]
    fn catmull_rom_overshoots_where_linear_would_not() {
        let keys = vec![
            key(0.0, 0.0, Interpolation::CatmullRom),
            key(1.0, 1.0, Interpolation::CatmullRom),
            key(2.0, 1.0, Interpolation::CatmullRom),
            key(3.0, 0.0, Interpolation::CatmullRom),
        ];
        let smooth = Track::new(BoneId(0), Channel::Rotation, keys.clone());
        let linear = Track::new(
            BoneId(0),
            Channel::Rotation,
            keys.into_iter()
                .map(|mut k| {
                    k.interpolation = Interpolation::Linear;
                    k
                })
                .collect(),
        );
        // Between the two equal middle keys the smooth curve bulges past them.
        assert!(smooth.sample(1.5).x > linear.sample(1.5).x);
    }

    #[test]
    fn a_step_keyframe_holds_until_the_next() {
        let track = Track::new(
            BoneId(0),
            Channel::Rotation,
            vec![
                key(0.0, 1.0, Interpolation::Step),
                key(1.0, 9.0, Interpolation::Step),
            ],
        );
        assert_eq!(track.sample(0.99).x, 1.0);
        assert_eq!(track.sample(1.0).x, 9.0);
    }

    #[test]
    fn sampling_outside_the_track_clamps_to_its_ends() {
        let track = Track::new(
            BoneId(0),
            Channel::Rotation,
            vec![
                key(1.0, 4.0, Interpolation::Linear),
                key(2.0, 8.0, Interpolation::Linear),
            ],
        );
        assert_eq!(track.sample(0.0).x, 4.0);
        assert_eq!(track.sample(99.0).x, 8.0);
    }

    #[test]
    fn a_looping_clip_wraps_and_a_one_shot_clamps() {
        let looping = Clip::new("walk".into(), 2.0, LoopMode::Loop, Vec::new());
        assert_eq!(looping.wrap(2.5), 0.5);
        assert_eq!(looping.wrap(-0.5), 1.5, "and wraps backwards too");

        let once = Clip::new("hurt".into(), 2.0, LoopMode::Once, Vec::new());
        assert_eq!(once.wrap(2.5), 2.0);
        assert!(once.finished(2.0));
        assert!(!looping.finished(99.0), "a loop is never finished");
    }

    #[test]
    fn a_clip_leaves_bones_it_does_not_animate_alone() {
        let mut builder = RigBuilder::new();
        let moved = builder.push_bone("moved", None, Vec3::ZERO);
        let still = builder.push_bone("still", None, Vec3::ZERO);
        let rig = builder.build(Vec::new());

        let clip = Clip::new(
            "twitch".into(),
            1.0,
            LoopMode::Loop,
            vec![Track::new(
                moved,
                Channel::Rotation,
                vec![key(0.0, 1.0, Interpolation::Linear)],
            )],
        );
        let mut pose = Pose::rest(&rig);
        pose.rotate(still, Vec3::new(0.25, 0.0, 0.0));
        clip.sample(0.0, &mut pose);

        assert_eq!(pose.get(moved).rotation.x, 1.0);
        assert_eq!(pose.get(still).rotation.x, 0.25, "the layer below survives");
    }

    #[test]
    fn a_clip_with_no_declared_length_takes_its_last_keyframe() {
        let clip = Clip::new(
            "walk".into(),
            0.0,
            LoopMode::Loop,
            vec![Track::new(
                BoneId(0),
                Channel::Rotation,
                vec![
                    key(0.0, 0.0, Interpolation::Linear),
                    key(1.5, 1.0, Interpolation::Linear),
                ],
            )],
        );
        assert_eq!(clip.length, 1.5);
        assert_eq!(one_bone().bone("limb"), Some(BoneId(0)));
    }
}
