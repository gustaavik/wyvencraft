//! A structure's shape: a small grid of blocks authored as text layers.
//!
//! ```toml
//! [structure.template]
//! palette = { "#" = "mossy_cobblestone", "R" = "wayrune", "." = "air" }
//! floor = 0
//! layers = [
//!   ["###", "###", "###"],   # y = 0, the floor (rows are z, chars are x)
//!   ["...", ".R.", "..."],   # y = 1
//! ]
//! ```
//!
//! A space means "leave the terrain alone", `.`-style palette entries naming
//! `air` force a void (clearing grass or a tree trunk out of a doorway). The
//! anchor is the footprint's centre cell; a placed instance is turned a
//! quarter turn at a time about it, so one template yields four layouts.

use std::collections::HashMap;

use crate::domain::core::BlockId;

/// What a template puts in one cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    /// Leave whatever generation put there.
    Keep,
    /// Force this block (air included).
    Block(BlockId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    /// `[x, y, z]` extent in cells.
    size: [i32; 3],
    /// Which layer sits at ground level; layers below it are dug in.
    floor: i32,
    /// `x + size.x * (z + size.z * y)`.
    cells: Vec<Cell>,
    /// Fills the gap from each floor cell down to the ground, so a structure on
    /// a slope stands on a plinth instead of floating.
    pub foundation: Option<BlockId>,
}

/// Turn a horizontal offset by `rot` quarter turns: `(x, z) → (-z, x)` each.
pub fn rotate(rot: u8, x: i32, z: i32) -> (i32, i32) {
    match rot & 3 {
        0 => (x, z),
        1 => (-z, x),
        2 => (-x, -z),
        _ => (z, -x),
    }
}

impl Template {
    /// Build from authored text. `palette` maps a character to a block
    /// (resolved by the caller); a space is always [`Cell::Keep`].
    pub fn parse(
        layers: &[Vec<String>],
        palette: &HashMap<char, BlockId>,
        floor: i32,
        foundation: Option<BlockId>,
    ) -> Result<Self, String> {
        let height = layers.len() as i32;
        let depth = layers.first().map_or(0, |rows| rows.len()) as i32;
        let width = layers
            .first()
            .and_then(|rows| rows.first())
            .map_or(0, |row| row.chars().count()) as i32;
        if height == 0 || depth == 0 || width == 0 {
            return Err("template has no cells".into());
        }
        if width % 2 == 0 || depth % 2 == 0 {
            return Err(format!(
                "template footprint {width}x{depth} must be odd on both axes, so it has a centre"
            ));
        }
        if !(0..height).contains(&floor) {
            return Err(format!("floor {floor} is not one of the {height} layers"));
        }
        let mut cells = Vec::with_capacity((width * height * depth) as usize);
        for (y, rows) in layers.iter().enumerate() {
            if rows.len() as i32 != depth {
                return Err(format!(
                    "layer {y} has {} rows, expected {depth}",
                    rows.len()
                ));
            }
            for (z, row) in rows.iter().enumerate() {
                if row.chars().count() as i32 != width {
                    return Err(format!("layer {y} row {z} is not {width} wide: {row:?}"));
                }
                for ch in row.chars() {
                    let cell =
                        match ch {
                            ' ' => Cell::Keep,
                            _ => Cell::Block(*palette.get(&ch).ok_or_else(|| {
                                format!("character {ch:?} is not in the palette")
                            })?),
                        };
                    cells.push(cell);
                }
            }
        }
        Ok(Self {
            size: [width, height, depth],
            floor,
            cells,
            foundation,
        })
    }

    /// Farthest a cell lies from the anchor horizontally, whatever the turn.
    pub fn reach(&self) -> i32 {
        self.size[0].max(self.size[2]) / 2
    }

    /// Layers below / above the ground cell.
    pub fn below(&self) -> i32 {
        self.floor
    }

    pub fn above(&self) -> i32 {
        self.size[1] - 1 - self.floor
    }

    fn get(&self, tx: i32, ty: i32, tz: i32) -> Cell {
        let [w, h, d] = self.size;
        if !(0..w).contains(&tx) || !(0..h).contains(&ty) || !(0..d).contains(&tz) {
            return Cell::Keep;
        }
        self.cells[(tx + w * (tz + d * ty)) as usize]
    }

    /// The cell at offset `(dx, dy, dz)` from the anchor of an instance turned
    /// `rot` quarter turns; `dy = 0` is the ground layer.
    pub fn cell(&self, rot: u8, dx: i32, dy: i32, dz: i32) -> Cell {
        // Undo the instance's turn to find the authored cell.
        let (ox, oz) = rotate(4 - (rot & 3), dx, dz);
        self.get(
            ox + self.size[0] / 2,
            dy + self.floor,
            oz + self.size[2] / 2,
        )
    }

    /// Whether any cell of the lowest layer in this column is a solid block —
    /// the columns a foundation holds up.
    pub fn rests_on_ground(&self, rot: u8, dx: i32, dz: i32) -> bool {
        matches!(self.cell(rot, dx, -self.floor, dz), Cell::Block(b) if !b.is_air())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROCK: BlockId = BlockId(1);
    const RUNE: BlockId = BlockId(2);

    fn palette() -> HashMap<char, BlockId> {
        HashMap::from([('#', ROCK), ('R', RUNE), ('.', BlockId::AIR)])
    }

    fn rows(rows: &[&str]) -> Vec<String> {
        rows.iter().map(|r| (*r).to_string()).collect()
    }

    /// An L-shaped marker so every turn is distinguishable.
    fn marker() -> Template {
        let layers = vec![rows(&["###", "#  ", "#  "]), rows(&["...", ".R.", "..."])];
        Template::parse(&layers, &palette(), 0, Some(ROCK)).unwrap()
    }

    #[test]
    fn four_quarter_turns_are_the_identity() {
        for (x, z) in [(1, 0), (2, -3), (-1, 5)] {
            let mut p = (x, z);
            for _ in 0..4 {
                p = rotate(1, p.0, p.1);
            }
            assert_eq!(p, (x, z));
        }
    }

    #[test]
    fn an_unturned_instance_reads_the_template_as_authored() {
        let t = marker();
        assert_eq!(t.cell(0, -1, 0, -1), Cell::Block(ROCK));
        assert_eq!(t.cell(0, 1, 0, -1), Cell::Block(ROCK));
        assert_eq!(t.cell(0, 1, 0, 1), Cell::Keep);
        assert_eq!(t.cell(0, 0, 1, 0), Cell::Block(RUNE));
        assert_eq!(t.cell(0, 0, 1, 1), Cell::Block(BlockId::AIR));
        assert_eq!(t.cell(0, 5, 0, 0), Cell::Keep, "outside the footprint");
    }

    /// A turned instance is the authored one turned: the cell authored at
    /// offset `o` appears at `rotate(rot, o)`.
    #[test]
    fn a_turned_instance_moves_every_cell_by_the_turn() {
        let t = marker();
        for rot in 0..4 {
            for dx in -1..=1 {
                for dz in -1..=1 {
                    let (wx, wz) = rotate(rot, dx, dz);
                    assert_eq!(t.cell(rot, wx, 0, wz), t.cell(0, dx, 0, dz), "rot {rot}");
                }
            }
        }
        // And the turns really differ, or the test above proves nothing.
        assert_ne!(t.cell(1, 1, 0, 1), t.cell(0, 1, 0, 1));
    }

    #[test]
    fn ragged_layers_are_rejected() {
        let err = Template::parse(&[rows(&["###", "##", "###"])], &palette(), 0, None);
        assert!(err.unwrap_err().contains("not 3 wide"));
        let err = Template::parse(
            &[rows(&["###", "###", "###"]), rows(&["###", "###"])],
            &palette(),
            0,
            None,
        );
        assert!(err.unwrap_err().contains("rows"));
    }

    #[test]
    fn unknown_characters_and_even_footprints_are_rejected() {
        let err = Template::parse(&[rows(&["#?#", "###", "###"])], &palette(), 0, None);
        assert!(err.unwrap_err().contains("palette"));
        let err = Template::parse(&[rows(&["##", "##"])], &palette(), 0, None);
        assert!(err.unwrap_err().contains("odd"));
    }

    #[test]
    fn the_floor_layer_sits_at_ground_level() {
        let layers = vec![rows(&["#"]), rows(&["R"]), rows(&["."])];
        let t = Template::parse(&layers, &palette(), 1, None).unwrap();
        assert_eq!(t.cell(0, 0, 0, 0), Cell::Block(RUNE));
        assert_eq!(t.cell(0, 0, -1, 0), Cell::Block(ROCK));
        assert_eq!((t.below(), t.above()), (1, 1));
        assert!(t.rests_on_ground(0, 0, 0));
    }
}
