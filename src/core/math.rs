//! Math helpers layered on top of [`glam`]: bounding boxes, rays, and view
//! frustums. Pure data + geometry, no rendering or world knowledge.

use glam::{Mat4, Vec3, Vec4};

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
                if normal.x >= 0.0 { aabb.max.x } else { aabb.min.x },
                if normal.y >= 0.0 { aabb.max.y } else { aabb.min.y },
                if normal.z >= 0.0 { aabb.max.z } else { aabb.min.z },
            );
            if normal.dot(positive) + p.w < 0.0 {
                return false;
            }
        }
        true
    }
}
