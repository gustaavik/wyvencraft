//! Math helpers layered on top of [`glam`]: bounding boxes, rays, and view
//! frustums. Pure data + geometry, no rendering or world knowledge.

use glam::{Mat4, Vec3, Vec4};

/// Rotate a point about the Y axis by `yaw` radians.
///
/// This is the engine's yaw convention, matching `Player::look_direction`. It is
/// the *opposite* sense to [`glam::Mat3::from_rotation_y`], so everything that
/// turns geometry to face a yaw — box models and file-loaded models alike — must
/// come through here, or the two disagree by a reflection.
pub fn rotate_y(p: Vec3, yaw: f32) -> Vec3 {
    let (s, c) = yaw.sin_cos();
    Vec3::new(p.x * c - p.z * s, p.y, p.x * s + p.z * c)
}

/// [`rotate_y`] as a matrix, for geometry built by composing transforms rather
/// than rotating point by point. The negated angle is what reconciles glam's
/// rotation sense with the engine's yaw.
pub fn yaw_matrix(yaw: f32) -> Mat4 {
    Mat4::from_rotation_y(-yaw)
}

/// Axis-aligned bounding box. Used for entity collision and frustum culling.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }

    /// Box centered on `center` with the given full-extent `size`.
    pub fn from_center_size(center: Vec3, size: Vec3) -> Self {
        let half = size * 0.5;
        Self {
            min: center - half,
            max: center + half,
        }
    }

    /// A unit cube occupying the block at integer corner `pos`.
    pub fn block(pos: Vec3) -> Self {
        Self {
            min: pos,
            max: pos + Vec3::ONE,
        }
    }

    pub fn translate(self, delta: Vec3) -> Self {
        Self {
            min: self.min + delta,
            max: self.max + delta,
        }
    }

    /// Grow the box outward by `amount` on every axis (Minkowski expansion).
    pub fn expand(self, amount: Vec3) -> Self {
        Self {
            min: self.min - amount,
            max: self.max + amount,
        }
    }

    pub fn center(self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    /// True if the two boxes overlap on all three axes.
    pub fn intersects(self, other: Aabb) -> bool {
        self.min.x < other.max.x
            && self.max.x > other.min.x
            && self.min.y < other.max.y
            && self.max.y > other.min.y
            && self.min.z < other.max.z
            && self.max.z > other.min.z
    }

    /// Slab-test ray intersection: the distance along `dir` (normalized) at
    /// which the ray from `origin` enters this box, if it does within
    /// `max_t`. An origin inside the box hits at `0.0`. Used for melee
    /// targeting (crosshair ray vs mob boxes).
    pub fn ray_hit(self, origin: Vec3, dir: Vec3, max_t: f32) -> Option<f32> {
        let mut t_enter: f32 = 0.0;
        let mut t_exit = max_t;
        for axis in 0..3 {
            let (o, d) = (origin[axis], dir[axis]);
            let (lo, hi) = (self.min[axis], self.max[axis]);
            if d.abs() < 1.0e-8 {
                // Parallel to the slab: must already be inside it.
                if o < lo || o > hi {
                    return None;
                }
                continue;
            }
            let (t0, t1) = ((lo - o) / d, (hi - o) / d);
            let (near, far) = if t0 < t1 { (t0, t1) } else { (t1, t0) };
            t_enter = t_enter.max(near);
            t_exit = t_exit.min(far);
            if t_enter > t_exit {
                return None;
            }
        }
        Some(t_enter)
    }
}

/// A ray with a normalized direction, used for block targeting.
#[derive(Debug, Clone, Copy)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

impl Ray {
    pub fn new(origin: Vec3, dir: Vec3) -> Self {
        Self {
            origin,
            dir: dir.normalize_or_zero(),
        }
    }

    pub fn at(self, t: f32) -> Vec3 {
        self.origin + self.dir * t
    }
}

/// View frustum as six inward-pointing planes, extracted from a combined
/// view-projection matrix (Gribb–Hartmann method). Used to cull chunks.
#[derive(Debug, Clone, Copy)]
pub struct Frustum {
    /// Each plane is `(a, b, c, d)` with `a*x + b*y + c*z + d >= 0` inside.
    planes: [Vec4; 6],
}

impl Frustum {
    pub fn from_view_proj(view_proj: Mat4) -> Self {
        // Rows of the matrix (glam stores column-major).
        let m = view_proj.to_cols_array_2d();
        let row = |i: usize| Vec4::new(m[0][i], m[1][i], m[2][i], m[3][i]);
        let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));

        let mut planes = [
            r3 + r0, // left
            r3 - r0, // right
            r3 + r1, // bottom
            r3 - r1, // top
            r3 + r2, // near
            r3 - r2, // far
        ];
        for p in &mut planes {
            // Normalize so distance comparisons are in world units.
            let len = p.truncate().length();
            if len > 0.0 {
                *p /= len;
            }
        }
        Self { planes }
    }

    /// Conservative test: returns `false` only if the box is fully outside the
    /// frustum. False positives (kept though off-screen) are acceptable.
    pub fn intersects_aabb(&self, aabb: Aabb) -> bool {
        for p in &self.planes {
            let normal = p.truncate();
            // The box corner furthest along the plane normal ("positive vertex").
            let positive = Vec3::new(
                if normal.x >= 0.0 {
                    aabb.max.x
                } else {
                    aabb.min.x
                },
                if normal.y >= 0.0 {
                    aabb.max.y
                } else {
                    aabb.min.y
                },
                if normal.z >= 0.0 {
                    aabb.max.z
                } else {
                    aabb.min.z
                },
            );
            if normal.dot(positive) + p.w < 0.0 {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_hits_a_box_ahead_and_misses_beside() {
        let cube = Aabb::new(Vec3::new(-0.5, 0.0, -4.5), Vec3::new(0.5, 1.0, -3.5));
        // Straight down -Z from the origin: enter at 3.5.
        let hit = cube.ray_hit(Vec3::new(0.0, 0.5, 0.0), Vec3::NEG_Z, 10.0);
        assert!((hit.unwrap() - 3.5).abs() < 1e-6);
        // Beyond max_t, or aimed elsewhere: no hit.
        assert!(
            cube.ray_hit(Vec3::new(0.0, 0.5, 0.0), Vec3::NEG_Z, 3.0)
                .is_none()
        );
        assert!(
            cube.ray_hit(Vec3::new(0.0, 0.5, 0.0), Vec3::Z, 10.0)
                .is_none()
        );
        assert!(
            cube.ray_hit(Vec3::new(2.0, 0.5, 0.0), Vec3::NEG_Z, 10.0)
                .is_none()
        );
    }

    #[test]
    fn ray_from_inside_hits_at_zero() {
        let cube = Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0));
        assert_eq!(cube.ray_hit(Vec3::ZERO, Vec3::X, 5.0), Some(0.0));
    }

    #[test]
    fn parallel_ray_outside_the_slab_misses() {
        let cube = Aabb::new(Vec3::ZERO, Vec3::ONE);
        // Runs parallel to the box along Z, two units above it.
        assert!(
            cube.ray_hit(Vec3::new(0.5, 3.0, -5.0), Vec3::Z, 20.0)
                .is_none()
        );
    }

    #[test]
    fn yaw_matrix_agrees_with_rotate_y() {
        // The two must stay interchangeable: box models rotate points with
        // `rotate_y`, loaded models compose `yaw_matrix` into a transform, and
        // a sign slip between them would mirror every imported model.
        for yaw in [0.0, 0.7, -1.3, std::f32::consts::PI, 4.9] {
            for p in [Vec3::X, Vec3::Z, Vec3::new(0.3, -2.0, 1.7)] {
                let a = rotate_y(p, yaw);
                let b = yaw_matrix(yaw).transform_point3(p);
                assert!(a.abs_diff_eq(b, 1e-5), "yaw {yaw}, point {p}: {a} vs {b}");
            }
        }
    }
}
