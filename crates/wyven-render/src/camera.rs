//! View/projection matrices and frustum extraction.
//!
//! Uses a right-handed view with a `[0,1]` depth range and a flipped Y to match
//! Vulkan clip space (so no negative-height viewport is needed).

use glam::{Mat4, Vec2, Vec3};

use wyven_core::Frustum;

pub struct Camera {
    pub position: Vec3,
    /// Normalized forward look direction.
    pub forward: Vec3,
    pub fov_y: f32, // radians
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
    /// Lens shift: the projected image is translated by this much in NDC,
    /// `(0, 0)` for an ordinary centred frame. Same convention
    /// [`Camera::project`] reports in, so positive x moves the image right and
    /// positive y moves it down.
    ///
    /// A *translation*, not a rotation or a sideways move of the camera. That
    /// is the whole reason it exists: to put a subject off-centre — the
    /// inventory frames the player on the left — without the two alternatives'
    /// costs. Yawing the camera would push the subject toward the edge of a
    /// 102°-wide frustum and visibly stretch it; trucking sideways would take
    /// the camera off the ray the occlusion trace follows, so terrain would
    /// start deciding where the subject sits in frame. A shift does neither:
    /// whatever is on the optical axis stays there, un-sheared, and the camera
    /// stays exactly where the collision code put it.
    ///
    /// It moves the *whole world*, sky included — it is a camera move, not a
    /// per-object one — and it makes the frustum asymmetric, which is why
    /// [`Camera::frustum`] applies it too.
    pub projection_offset: Vec2,
}

impl Camera {
    pub fn new(fov_y_degrees: f32, aspect: f32) -> Self {
        Self {
            position: Vec3::ZERO,
            forward: Vec3::NEG_Z,
            fov_y: fov_y_degrees.to_radians(),
            aspect,
            near: 0.1,
            far: 1000.0,
            projection_offset: Vec2::ZERO,
        }
    }

    /// Fold [`Camera::projection_offset`] into a projection that is still in
    /// **unflipped** clip space (+Y up), which is where both callers apply it.
    ///
    /// `perspective_rh` leaves `z_axis.x`/`z_axis.y` zero and sets `clip.w` to
    /// `-view.z`, so adding `-offset` there contributes `offset * clip.w` to
    /// `clip.xy` — exactly a constant shift after the perspective divide, at
    /// every depth. The y term is negated because this runs before the Y flip
    /// and the offset is quoted in screen convention.
    fn shift(&self, proj: &mut Mat4) {
        proj.z_axis.x -= self.projection_offset.x;
        proj.z_axis.y += self.projection_offset.y;
    }

    pub fn set_aspect(&mut self, width: f32, height: f32) {
        self.aspect = if height > 0.0 { width / height } else { 1.0 };
    }

    /// Radius of the sphere through the near plane's four corners, measured from
    /// the eye. Nothing nearer than this is drawn, so it is exactly the clearance
    /// a camera needs in front of a surface to keep it out of frame.
    ///
    /// Derived from `near`, `fov_y` and `aspect` rather than tuned: the field of
    /// view is a player setting, and a padding sized for the shipped 70° would
    /// let a wide FOV clip through walls again.
    pub fn near_radius(&self) -> f32 {
        let half_h = self.near * (self.fov_y * 0.5).tan();
        let half_w = half_h * self.aspect;
        (self.near * self.near + half_h * half_h + half_w * half_w).sqrt()
    }

    pub fn view_matrix(&self) -> Mat4 {
        Mat4::look_to_rh(self.position, self.forward, Vec3::Y)
    }

    /// Projection for drawing: **reversed-Z**, so the near plane maps to depth
    /// `1.0` and the far plane to `0.0`.
    ///
    /// Swapping `near` and `far` is all reversing takes — `perspective_rh` maps
    /// its `z_near` argument to 0 and its `z_far` to 1, so handing it the far
    /// plane as "near" inverts the range exactly. Every pipeline that depth-tests
    /// therefore compares with [`CompareOp::Greater`] and every depth attachment
    /// clears to `0.0`.
    ///
    /// This is not a stylistic choice. With a conventional `[0,1]` range and
    /// `near = 0.1`, every depth value in the world crowds into `0.993..=0.9996`,
    /// where a float32's exponent is pinned and only the mantissa separates one
    /// surface from another — so the 2 mm nudge that holds the grass overlay off
    /// the block beneath it is worth 209 ULPs at 4 blocks but 0.2 ULPs at 128,
    /// and coplanar faces z-fight across most of the render distance. Reversed-Z
    /// puts the far plane at 0 where the exponent has room to work, and the
    /// float distribution then cancels the perspective divide's bunching almost
    /// exactly.
    ///
    /// [`CompareOp::Greater`]: vulkano::pipeline::graphics::depth_stencil::CompareOp::Greater
    pub fn projection_matrix(&self) -> Mat4 {
        let mut proj = Mat4::perspective_rh(self.fov_y, self.aspect, self.far, self.near);
        self.shift(&mut proj);
        // Flip Y for Vulkan's clip space (origin top-left, +Y down). This
        // negates the matrix's second *row*; with a lens shift that row has two
        // non-zero entries rather than one, so both have to turn over — miss
        // `z_axis.y` and a vertical shift comes out upside down.
        proj.y_axis.y = -proj.y_axis.y;
        proj.z_axis.y = -proj.z_axis.y;
        proj
    }

    pub fn view_projection(&self) -> Mat4 {
        self.projection_matrix() * self.view_matrix()
    }

    /// Inverse of the projection times a *translation-free* view, for the sky
    /// pass. Unprojecting a clip-space point with this yields a pure world-space
    /// ray direction (independent of camera position), and it uses the same
    /// flipped projection as [`Self::view_projection`] so the sun aligns with the
    /// world geometry.
    pub fn sky_inv_view_proj(&self) -> Mat4 {
        let view_rotation = Mat4::look_to_rh(Vec3::ZERO, self.forward, Vec3::Y);
        (self.projection_matrix() * view_rotation).inverse()
    }

    /// Frustum for culling. Built from the *unflipped, forward-Z* view-proj so
    /// plane math stays in conventional world orientation.
    ///
    /// Deliberately **not** [`Self::projection_matrix`]. `Frustum::from_view_proj`
    /// extracts its near/far pair as `row3 ± row2`, which assumes a depth that
    /// grows with distance; hand it a reversed projection and the far plane comes
    /// out as a second copy of the near one, silently uncapping the frustum and
    /// culling nothing at range. `near`, `far`, `fov_y` and `aspect` are shared
    /// with the drawing projection, so this still bounds exactly what gets drawn.
    /// A lens shift makes this asymmetric — more world on the side the image
    /// moves toward — and it has to be applied here too. Cull with the
    /// unshifted frustum and geometry the shift has just brought on screen is
    /// thrown away, which shows as pop-in along the edge the subject sits on.
    pub fn frustum(&self) -> Frustum {
        let mut proj = Mat4::perspective_rh(self.fov_y, self.aspect, self.near, self.far);
        self.shift(&mut proj);
        Frustum::from_view_proj(proj * self.view_matrix())
    }

    /// Project a world point to normalized screen coordinates.
    ///
    /// Returns `(x, y)` in `[0,1]`, origin **top-left** — ready to multiply by a
    /// widget's size. `None` when the point is at or behind the eye plane, where
    /// the perspective divide is meaningless and would otherwise place a point
    /// behind you cheerfully in front of you.
    ///
    /// No Y flip is applied here because [`projection_matrix`](Self::projection_matrix)
    /// already carries one for Vulkan, which happens to be the same convention
    /// screen-space UI uses. A point may land outside `[0,1]`; that is a caller's
    /// business, since a nameplate slightly off-screen is still worth clipping
    /// rather than discarding.
    ///
    /// Kept in `render` and returning a plain [`Vec2`] so this module stays free
    /// of any UI dependency.
    pub fn project(&self, world: Vec3) -> Option<Vec2> {
        let clip = self.view_projection() * world.extend(1.0);
        if clip.w <= f32::EPSILON {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        Some(Vec2::new(ndc.x * 0.5 + 0.5, ndc.y * 0.5 + 0.5))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Looking down -Z from the origin, which is the orientation the projection
    /// math is written for.
    fn camera() -> Camera {
        let mut camera = Camera::new(70.0, 16.0 / 9.0);
        camera.position = Vec3::ZERO;
        camera.forward = -Vec3::Z;
        camera
    }

    /// The justification for choosing a lens shift over yawing the camera: the
    /// image *translates*. Every point moves by the same amount and the
    /// distances between them are untouched, so the subject keeps its shape.
    #[test]
    fn a_lens_shift_moves_the_image_without_reshaping_it() {
        let points = [
            Vec3::new(0.0, 0.0, -5.0),
            Vec3::new(1.0, 0.5, -5.0),
            Vec3::new(-0.7, 1.2, -9.0),
        ];
        let shift = Vec2::new(-0.4, 0.25);

        let plain = camera();
        let mut shifted = camera();
        shifted.projection_offset = shift;

        let before: Vec<Vec2> = points.iter().map(|p| plain.project(*p).unwrap()).collect();
        let after: Vec<Vec2> = points
            .iter()
            .map(|p| shifted.project(*p).unwrap())
            .collect();

        // `project` reports in [0,1], so an NDC shift of `s` moves it by `s/2`.
        let expected = shift * 0.5;
        for (i, (a, b)) in before.iter().zip(&after).enumerate() {
            assert!(
                (*b - *a - expected).abs().max_element() < 1e-5,
                "point {i} moved by {:?}, expected {expected:?}",
                *b - *a
            );
        }
        // ...and therefore the shape they describe is unchanged.
        for i in 1..points.len() {
            let plain_edge = before[i] - before[0];
            let shifted_edge = after[i] - after[0];
            assert!(
                (plain_edge - shifted_edge).abs().max_element() < 1e-5,
                "edge {i} was resized by the shift"
            );
        }
    }

    /// Sliding the image left brings into view what was off the *right* edge,
    /// and the frustum has to follow. Without this the culler throws away
    /// exactly the geometry the shift just revealed.
    #[test]
    fn a_lens_shift_widens_the_frustum_on_the_side_it_reveals() {
        let mut shifted = camera();
        shifted.projection_offset = Vec2::new(-0.6, 0.0);

        // Far enough right to be off screen unshifted, on screen shifted: at
        // z = -5 with a 70° vertical FOV at 16:9 the half-width is ~6.2.
        let point = Vec3::new(8.0, 0.0, -5.0);
        assert!(
            camera().project(point).unwrap().x > 1.0,
            "the test point must start off the right edge"
        );
        let p = shifted.project(point).unwrap();
        assert!(
            (0.0..=1.0).contains(&p.x),
            "the shift should have brought it on screen, got {p:?}"
        );

        let aabb = wyven_core::Aabb::new(point - Vec3::splat(0.05), point + Vec3::splat(0.05));
        assert!(
            shifted.frustum().intersects_aabb(aabb),
            "the frustum did not follow the shift"
        );
    }

    /// The field is inert when unused, so nothing that does not want it pays
    /// for it — including the sky pass and the nameplates, which both route
    /// through `projection_matrix`.
    #[test]
    fn a_zero_shift_leaves_the_projection_untouched() {
        let c = camera();
        let plain = Mat4::perspective_rh(c.fov_y, c.aspect, c.far, c.near);
        let mut expected = plain;
        expected.y_axis.y *= -1.0;
        assert_eq!(c.projection_matrix(), expected);
    }

    #[test]
    fn a_point_straight_ahead_lands_in_the_centre() {
        let screen = camera().project(Vec3::new(0.0, 0.0, -10.0)).unwrap();

        assert!((screen.x - 0.5).abs() < 1.0e-5, "x was {}", screen.x);
        assert!((screen.y - 0.5).abs() < 1.0e-5, "y was {}", screen.y);
    }

    /// The convention that matters for drawing: y grows *downward*, matching
    /// screen space. Getting this backwards would put every nameplate under its
    /// player's feet.
    #[test]
    fn a_point_above_the_camera_projects_to_the_upper_half() {
        let screen = camera().project(Vec3::new(0.0, 2.0, -10.0)).unwrap();
        assert!(
            screen.y < 0.5,
            "expected the upper half, got y = {}",
            screen.y
        );
    }

    #[test]
    fn a_point_to_the_right_projects_to_the_right_half() {
        let screen = camera().project(Vec3::new(2.0, 0.0, -10.0)).unwrap();
        assert!(
            screen.x > 0.5,
            "expected the right half, got x = {}",
            screen.x
        );
    }

    /// Without the `w` check, a point behind the camera divides through to a
    /// plausible-looking on-screen position — so every player behind you would
    /// get a nameplate, mirrored.
    #[test]
    fn a_point_behind_the_camera_does_not_project() {
        assert!(camera().project(Vec3::new(0.0, 0.0, 10.0)).is_none());
        assert!(camera().project(Vec3::ZERO).is_none());
    }

    #[test]
    fn a_point_off_to_the_side_projects_outside_the_unit_range() {
        // Still a valid projection — clipping is the caller's decision.
        let screen = camera().project(Vec3::new(100.0, 0.0, -1.0)).unwrap();
        assert!(
            screen.x > 1.0,
            "expected off-screen right, got {}",
            screen.x
        );
    }

    /// The whole point of deriving it: a player who widens their FOV must get a
    /// wider berth around walls, or the third-person camera starts clipping again.
    #[test]
    fn the_near_radius_grows_with_the_field_of_view() {
        let narrow = Camera::new(70.0, 16.0 / 9.0).near_radius();
        let wide = Camera::new(110.0, 16.0 / 9.0).near_radius();

        assert!(wide > narrow, "110° gave {wide}, 70° gave {narrow}");
        assert!(
            narrow > 0.1,
            "the corner is further from the eye than the near plane itself, got {narrow}"
        );
    }

    /// Depth in NDC, for the reversed-Z assertions below.
    fn ndc_z(camera: &Camera, world: Vec3) -> f32 {
        let clip = camera.view_projection() * world.extend(1.0);
        clip.z / clip.w
    }

    /// The invariant the whole depth setup rests on: near maps to 1, far to 0.
    /// Flip this and every pipeline's `CompareOp::Greater` silently discards the
    /// world instead of drawing it.
    #[test]
    fn the_projection_is_reversed_z() {
        let camera = camera();
        let near = ndc_z(&camera, Vec3::new(0.0, 0.0, -camera.near));
        let far = ndc_z(&camera, Vec3::new(0.0, 0.0, -camera.far));

        assert!(
            (near - 1.0).abs() < 1.0e-4,
            "near plane gave {near}, want 1.0"
        );
        assert!(far.abs() < 1.0e-4, "far plane gave {far}, want 0.0");
    }

    /// What reversed-Z is actually bought with: at 128 blocks — the default
    /// render distance — the 2 mm coplanar nudge must still be worth many
    /// float32 ULPs, or coplanar faces z-fight. Forward-Z managed 0.2 here.
    #[test]
    fn the_coplanar_nudge_survives_the_far_end_of_the_render_distance() {
        const NUDGE: f32 = 0.002;
        let camera = camera();

        let back = ndc_z(&camera, Vec3::new(0.0, 0.0, -128.0));
        let front = ndc_z(&camera, Vec3::new(0.0, 0.0, -128.0 + NUDGE));
        let ulps = (front - back).abs() / (back * f32::EPSILON);

        assert!(front > back, "the nearer face must win a `Greater` test");
        assert!(ulps > 100.0, "only {ulps} ULPs of separation at 128 blocks");
    }

    #[test]
    fn projection_tracks_the_camera_as_it_turns() {
        let mut camera = camera();
        let behind = Vec3::new(0.0, 0.0, 10.0);
        assert!(camera.project(behind).is_none());

        camera.forward = Vec3::Z;
        let screen = camera.project(behind).expect("now in front");
        assert!((screen.x - 0.5).abs() < 1.0e-5);
    }
}
