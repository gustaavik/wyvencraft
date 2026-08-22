//! Flat item sprites: an atlas icon extruded one texel deep.
//!
//! A loose stack of a *block* is drawn as a miniature of that block — six faces,
//! each with its own texture. Anything else has only a flat icon, and wrapping
//! that icon around a cube reads as six apples rather than one. Instead the icon
//! is drawn as itself: two quads back to back, one texel apart, plus a rim
//! wherever the drawn shape ends, so the item still has an edge when it spins
//! side-on.
//!
//! The rim is what makes this more than a billboard, and finding it means
//! walking the art's alpha. That is why [`ItemSprite`] is derived once per atlas
//! tile and kept, rather than re-derived for every drop on every frame.

use glam::Vec3;

use wyven_core::Direction;
use wyven_core::math::rotate_y;
use wyven_render::mesh::CpuMesh;
use wyven_render::texture::{TILE_SIZE, atlas_uv};
use wyven_render::tile_registry::TileRgba;
use wyven_render::vertex::{ChunkVertex, NO_TINT};

use super::culled::face_shade;

/// Alpha at or above which a texel counts as part of the shape. The same cutoff
/// `voxel.frag` discards below, so the extruded rim traces exactly the silhouette
/// that ends up drawn.
const ALPHA_CUTOFF: u8 = 26; // 0.1 * 255, rounded up

/// One exposed texel edge: where in the icon, and which way it faces.
type RimEdge = (u16, u16, Direction);

/// The extruded silhouette of one flat item icon.
pub struct ItemSprite {
    tile: u32,
    /// Every texel edge with nothing beside it, as `(x, y, outward direction)`
    /// with `y = 0` at the top of the icon. Only the four in-plane directions
    /// appear; the two flat faces are implicit.
    rim: Vec<RimEdge>,
}

impl ItemSprite {
    /// Trace the silhouette of `art`.
    ///
    /// `None` — a tile with no texture at all — is treated as fully solid, so a
    /// missing icon still reads as a card rather than vanishing.
    pub fn new(tile: u32, art: Option<&TileRgba>) -> Self {
        let n = TILE_SIZE as usize;
        let solid = |x: usize, y: usize| match art {
            Some(art) => art[y][x][3] >= ALPHA_CUTOFF,
            None => true,
        };
        let mut rim = Vec::new();
        for y in 0..n {
            for x in 0..n {
                if !solid(x, y) {
                    continue;
                }
                // `y` counts down the image, so the *previous* row is up.
                let exposed = [
                    (Direction::NegX, x == 0 || !solid(x - 1, y)),
                    (Direction::PosX, x + 1 == n || !solid(x + 1, y)),
                    (Direction::PosY, y == 0 || !solid(x, y - 1)),
                    (Direction::NegY, y + 1 == n || !solid(x, y + 1)),
                ];
                for (dir, is_exposed) in exposed {
                    if is_exposed {
                        rim.push((x as u16, y as u16, dir));
                    }
                }
            }
        }
        Self { tile, rim }
    }

    /// The atlas tile this was traced from.
    pub fn tile(&self) -> u32 {
        self.tile
    }

    /// How many rim quads the sprite contributes — its silhouette's perimeter in
    /// texels.
    pub fn rim_len(&self) -> usize {
        self.rim.len()
    }
}

/// Append `sprite` to `mesh`: the icon `size` across, one texel deep, centred on
/// `center` and turned `yaw` radians about Y.
pub fn push_item_sprite(
    mesh: &mut CpuMesh,
    sprite: &ItemSprite,
    center: Vec3,
    size: f32,
    yaw: f32,
) {
    let n = TILE_SIZE as f32;
    let texel = size / n;
    let (half, depth) = (size / 2.0, texel / 2.0);

    // Local space is the icon upright in XY, facing +Z: `u` runs with +X and `v`
    // runs *against* +Y, which is what puts the sprite the same way round as the
    // inventory icon it is drawn from. Spelled out here rather than borrowed from
    // the cube mesher, whose UV corners are the block convention.
    let px = |x: f32| (x / n - 0.5) * size;
    let py = |y: f32| (0.5 - y / n) * size;

    let mut quad = |dir: Direction, corners: [([f32; 3], [f32; 2]); 4]| {
        let normal = rotate_y(Vec3::from(dir.normal().to_array()), yaw).to_array();
        let ao = face_shade(dir);
        let verts = std::array::from_fn(|i| {
            let (local, uv) = corners[i];
            let p = center + rotate_y(Vec3::from(local), yaw);
            ChunkVertex {
                position: p.to_array(),
                normal,
                uv: atlas_uv(sprite.tile, uv),
                ao,
                flags: 0,
                layer: 0,
                tint: NO_TINT,
            }
        });
        mesh.push_quad(verts);
    };

    // The two flat faces carry the whole icon; the alpha test cuts the shape out
    // of each. Both map the texture the same way, so the back is the mirror image
    // — which is what looking through a paper-thin object should give you.
    for (dir, z) in [(Direction::PosZ, depth), (Direction::NegZ, -depth)] {
        quad(
            dir,
            [
                ([-half, -half, z], [0.0, 1.0]),
                ([half, -half, z], [1.0, 1.0]),
                ([half, half, z], [1.0, 0.0]),
                ([-half, half, z], [0.0, 0.0]),
            ],
        );
    }

    // The rim. Each quad is one texel's exposed edge, and takes that texel's own
    // colour flat — sampling the centre rather than a sliver keeps it clear of
    // its neighbours whatever the sampler does at the boundary.
    for &(x, y, dir) in &sprite.rim {
        let (x, y) = (f32::from(x), f32::from(y));
        let uv = [(x + 0.5) / n, (y + 0.5) / n];
        let (x0, x1) = (px(x), px(x + 1.0));
        let (y0, y1) = (py(y), py(y + 1.0)); // y0 is the *upper* edge
        let corners = match dir {
            Direction::NegX => [
                [x0, y1, -depth],
                [x0, y1, depth],
                [x0, y0, depth],
                [x0, y0, -depth],
            ],
            Direction::PosX => [
                [x1, y1, depth],
                [x1, y1, -depth],
                [x1, y0, -depth],
                [x1, y0, depth],
            ],
            Direction::PosY => [
                [x0, y0, depth],
                [x1, y0, depth],
                [x1, y0, -depth],
                [x0, y0, -depth],
            ],
            _ => [
                [x0, y1, -depth],
                [x1, y1, -depth],
                [x1, y1, depth],
                [x0, y1, depth],
            ],
        };
        quad(dir, corners.map(|c| (c, uv)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: usize = TILE_SIZE as usize;

    /// Art that is opaque only inside `[lo, hi)` on both axes.
    fn block_art(lo: usize, hi: usize) -> Box<TileRgba> {
        let mut art = Box::new([[[0u8; 4]; N]; N]);
        for (y, row) in art.iter_mut().enumerate() {
            for (x, px) in row.iter_mut().enumerate() {
                if (lo..hi).contains(&x) && (lo..hi).contains(&y) {
                    *px = [255, 255, 255, 255];
                }
            }
        }
        art
    }

    /// A solid square's rim is its perimeter: four edges per texel on the border,
    /// with the corners contributing two each.
    #[test]
    fn a_square_silhouette_traces_its_own_perimeter() {
        let side = 8;
        let art = block_art(4, 4 + side);
        let sprite = ItemSprite::new(1, Some(&art));
        assert_eq!(sprite.rim_len(), 4 * side);
    }

    /// Fully transparent art has no shape, so it extrudes nothing at all.
    #[test]
    fn empty_art_has_no_rim() {
        let art = Box::new([[[0u8; 4]; N]; N]);
        assert_eq!(ItemSprite::new(1, Some(&art)).rim_len(), 0);
    }

    /// A tile with no art is a solid card: only the outer border is exposed.
    #[test]
    fn missing_art_is_a_full_card() {
        assert_eq!(ItemSprite::new(0, None).rim_len(), 4 * N);
    }

    /// The sprite is a card, not a cube: it fills its footprint in X and Y but
    /// stays one texel thick in Z, and every vertex sits inside that slab.
    #[test]
    fn the_mesh_is_one_texel_thick() {
        let art = block_art(0, N);
        let sprite = ItemSprite::new(1, Some(&art));
        let mut mesh = CpuMesh::new();
        let size = 0.25;
        push_item_sprite(&mut mesh, &sprite, Vec3::ZERO, size, 0.0);

        let texel = size / TILE_SIZE as f32;
        for v in &mesh.vertices {
            assert!(v.position[0].abs() <= size / 2.0 + 1e-5, "x out of bounds");
            assert!(v.position[1].abs() <= size / 2.0 + 1e-5, "y out of bounds");
            assert!(v.position[2].abs() <= texel / 2.0 + 1e-5, "z is not thin");
        }
        // Two flat faces plus the border, six indices each.
        assert_eq!(mesh.indices.len(), (2 + 4 * N) * 6);
    }

    /// The icon reads the same way round as it does in the inventory: the texel
    /// at the top-left of the image sits at the top-left of the front face.
    #[test]
    fn the_front_face_is_not_mirrored() {
        let art = block_art(0, N);
        let sprite = ItemSprite::new(1, Some(&art));
        let mut mesh = CpuMesh::new();
        push_item_sprite(&mut mesh, &sprite, Vec3::ZERO, 1.0, 0.0);

        // The front face is the first quad pushed; find its uv (0, 0) corner.
        let top_left = mesh.vertices[..4]
            .iter()
            .find(|v| v.uv == atlas_uv(1, [0.0, 0.0]))
            .expect("a uv origin corner");
        assert!(top_left.position[0] < 0.0, "uv u=0 should be at -X");
        assert!(top_left.position[1] > 0.0, "uv v=0 should be at +Y");
    }
}
