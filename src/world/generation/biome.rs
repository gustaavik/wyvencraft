//! Simple climate-driven biome selection. Maps a temperature sample to a biome
//! and its characteristic surface/sub-surface blocks.

use crate::core::BlockId;
use crate::world::block::blocks;

/// Temperatures below this are snowy.
pub const SNOWY_MAX_TEMP: f32 = -0.35;
/// Temperatures above this are desert.
pub const DESERT_MIN_TEMP: f32 = 0.4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Biome {
    Snowy,
    Plains,
    Desert,
}

impl Biome {
    /// Pick a biome from a temperature sample in roughly `[-1, 1]`.
    pub fn from_temperature(temperature: f32) -> Biome {
        if temperature < SNOWY_MAX_TEMP {
            Biome::Snowy
        } else if temperature > DESERT_MIN_TEMP {
            Biome::Desert
        } else {
            Biome::Plains
        }
    }

    /// Block placed on the very top of the terrain column.
    pub fn surface_block(self) -> BlockId {
        match self {
            Biome::Snowy => blocks::SNOW,
            Biome::Plains => blocks::GRASS,
            Biome::Desert => blocks::SAND,
        }
    }

    /// Block placed in the few layers just beneath the surface.
    pub fn subsurface_block(self) -> BlockId {
        match self {
            Biome::Snowy | Biome::Plains => blocks::DIRT,
            Biome::Desert => blocks::SAND,
        }
    }
}
