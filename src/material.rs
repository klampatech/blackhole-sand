//! Material definitions: enum + per-material color and bond rules.
//!
//! Phase 1 materials: Rock, Ice, Plasma, Vacuum. Plasma has no bonds
//! (diffuses like a gas); Vacuum is the empty cell.

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Material {
    Vacuum = 0,
    Rock = 1,
    Ice = 2,
    Plasma = 3,
}

impl Material {
    pub const fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Rock,
            2 => Self::Ice,
            3 => Self::Plasma,
            _ => Self::Vacuum,
        }
    }

    /// RGBA8 cell color for this material. Black for Vacuum.
    pub const fn color(self) -> [u8; 4] {
        match self {
            Self::Vacuum => [0, 0, 0, 255],
            Self::Rock => [150, 140, 130, 255],
            Self::Ice => [180, 220, 240, 255],
            Self::Plasma => [255, 130, 50, 255],
        }
    }

    /// Plasma has no bonds; everything else (Rock, Ice) bonds.
    pub const fn has_bonds(self) -> bool {
        !matches!(self, Self::Vacuum | Self::Plasma)
    }

    /// Max-strain threshold (in cells) for the bond-stress pass. Phase 1 uses
    /// a 1-cell resolution grid (no sub-cell positions), so any non-adjacency
    /// breaks a bond — `max_strain` is effectively always 1. The threshold is
    /// kept per-material for Phase 2+ when sub-cell positions arrive and
    /// different materials can tolerate different amounts of stretch.
    #[allow(dead_code)] // surfaced in Phase 2 with sub-cell positions
    pub const fn max_strain(self) -> u8 {
        match self {
            Self::Rock => 1,
            Self::Ice => 1,
            _ => 0,
        }
    }
}
