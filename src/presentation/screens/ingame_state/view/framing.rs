//! The world camera: where it sits for the player's perspective, the
//! inventory's framing shot, and how the world pulls it in.

use super::*;

impl super::super::InGameState {
    /// How far the third-person camera sits from the eye this frame: the desired
    /// [`THIRD_PERSON_DISTANCE`], pulled in so nothing solid ends up between the
    /// camera and the player.
    ///
    /// Lives here rather than on [`SceneCache`] because it needs the world, which
    /// the view deliberately cannot reach. Recomputed per call rather than
    /// cached: it is a pure function of the world and the player, so the
    /// nameplate camera and the world camera work out the same answer within a
    /// frame without having to share state to do it.
    ///
    /// The predicate is `is_solid_for_collision`, the one player physics uses:
    /// it counts an unloaded chunk as solid, so at the streaming edge the camera
    /// pulls in rather than drifting into terrain that has not arrived yet.
    pub(in super::super) fn world_camera(&self, aspect: f32) -> Camera {
        let shot = self.camera_shot();
        let yaw = self.framing_yaw();
        let eye = self
            .sim
            .player
            .interpolated_eye_position(self.view.render_alpha);

        let distance = if shot.distance <= 0.0 {
            // First person, or a sweep that has not left the eye yet: there is
            // no gap between camera and player for anything to get into.
            0.0
        } else {
            let clearance = Camera::new(self.view.fov_degrees, aspect).near_radius();
            camera::clear_distance(eye, shot.offset(yaw), shot.distance, clearance, |p| {
                self.sim.world.is_solid_for_collision(p)
            })
        };

        shot.camera(eye, yaw, distance, self.view.fov_degrees, aspect)
    }

    /// The yaw the shot is framed on.
    ///
    /// The *body* yaw while the inventory is up, not the look yaw: the model is
    /// drawn at `AnimationState::body_yaw`, which lags the look yaw and can sit
    /// a good way off it when the player is standing still. Framing on the look
    /// yaw would show a model visibly turned away from the camera.
    pub(in super::super) fn framing_yaw(&self) -> f32 {
        if self.inventory_anim.active() {
            self.sim.player_anim.body_yaw()
        } else {
            self.sim.player.yaw
        }
    }

    /// This frame's shot: the player's chosen perspective, blended toward the
    /// inventory's framing shot by however far through the sweep we are.
    ///
    /// Blending happens in [`Shot`]'s polar form, so a swing from behind the
    /// player to in front of them orbits around them instead of passing through
    /// their head at the halfway point.
    fn camera_shot(&self) -> Shot {
        let gameplay = self
            .sim
            .player
            .perspective
            .shot(self.sim.player.pitch, THIRD_PERSON_DISTANCE);
        let t = self.inventory_anim.progress();
        if t <= 0.0 {
            return gameplay;
        }
        // `layout` reasons in points — it subtracts a panel some hundreds of
        // points wide from the screen — so it has to be handed the *real*
        // screen rect. Handing it a normalised one collapses the whole stage to
        // zero width and slams the model into the left edge.
        let stage = crate::presentation::ui::inventory::layout(
            self.screen,
            self.sim.player.mode.is_creative(),
        )
        .stage_center_x;
        gameplay.blend(Shot::inspect(self.view.fov_degrees.to_radians(), stage), t)
    }
}
