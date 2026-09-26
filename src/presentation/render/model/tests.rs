//! Tests for [`super`]: `model.rs`.

use super::*;

/// Rotate a point about the X axis — the limb swing these tests check,
/// spelled out here rather than reached for in the model, which composes
/// its rotations as matrices.
fn rot_x(p: Vec3, a: f32) -> Vec3 {
    let (s, c) = a.sin_cos();
    Vec3::new(p.x, p.y * c - p.z * s, p.y * s + p.z * c)
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
    let mesh = model.build_mesh_sheet(Vec3::ZERO, 0.0, &Pose::default(), skin::SKIN_ORIGIN);
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
    let mesh = model.build_mesh_sheet(Vec3::ZERO, 0.0, &Pose::default(), skin::SKIN_ORIGIN);
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
    let mesh = QuadrupedModel::new(&visual).build_mesh(Vec3::ZERO, 0.0, &Pose::default(), origin);

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
    let default_sheet =
        model.build_mesh_sheet(Vec3::ZERO, 0.0, &Pose::default(), skin::SKIN_ORIGIN);
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
fn a_head_yaw_turns_the_head_without_turning_the_body() {
    // Parts are pushed base-then-overlay in table order (head, body, ...), 24
    // vertices to a box, so the head is the first two boxes and the torso the
    // next two.
    const BOX: usize = 24;
    let model = HumanoidModel::player();
    let rest = model.build_mesh_sheet(Vec3::ZERO, 0.0, &Pose::default(), skin::SKIN_ORIGIN);
    let turned = model.build_mesh_sheet(
        Vec3::ZERO,
        0.0,
        &Pose {
            head_yaw: std::f32::consts::FRAC_PI_2,
            ..Pose::default()
        },
        skin::SKIN_ORIGIN,
    );
    assert_eq!(rest.vertices.len(), turned.vertices.len());

    let moved = |i: usize| {
        let a = Vec3::from_array(rest.vertices[i].position);
        let b = Vec3::from_array(turned.vertices[i].position);
        (a - b).length()
    };

    // The head (and its hat shell) swung round.
    let head_shift = (0..2 * BOX).map(moved).fold(0.0f32, f32::max);
    assert!(head_shift > 0.1, "the head barely moved: {head_shift}");

    // Everything from the torso down stayed exactly put.
    for i in 2 * BOX..rest.vertices.len() {
        assert!(
            moved(i) < 1e-5,
            "vertex {i} moved {} — head yaw must not turn the body",
            moved(i)
        );
    }

    // And the turn is about the neck, so the head stays on the centre line
    // rather than orbiting the model.
    let centre = |m: &CpuMesh| {
        (0..2 * BOX)
            .map(|i| Vec3::from_array(m.vertices[i].position))
            .fold(Vec3::ZERO, |a, b| a + b)
            / (2 * BOX) as f32
    };
    assert!(
        (centre(&rest) - centre(&turned)).length() < 1e-5,
        "the head orbited instead of turning: {} vs {}",
        centre(&rest),
        centre(&turned)
    );
}
