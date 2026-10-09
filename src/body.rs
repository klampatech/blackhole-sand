//! Phase 2 body types: BlackHole and Planet.
//!
//! Bodies are first-class objects with f32 sub-cell positions and velocities.
//! Particles still live on integer grid cells (see [`crate::sim`]). Each
//! body owns the particles spawned into its grid footprint; the body's mass
//! tracks that particle count in real time so orbits decay as the body is
//! stripped.
//!
//! See `docs/phase-2-multi-body-gravity.md` for the design rationale.

// (H, W are referenced from sim.rs via the W/H re-exports; this import is unused.)

/// f32 2D vector. Bodies live in *grid* coordinates, so position `(128.0,
/// 64.5)` means "cell column 128, halfway between rows 64 and 65". The
/// particle grid is coarser — particles snap to the nearest integer cell.
pub type Vec2 = glam::Vec2;

/// Gravitational constant in sim units. Tuned so a BlackHole of mass
/// 1000 at the grid center produces a visually compelling slingshot when
/// a 10-mass planet passes within ~30 cells. Units are
/// "cells^3 / (mass * tick^2)" — we keep tick = 1, so velocities are in
/// cells/tick.
///
/// Worked example for a circular orbit around a central mass M:
///     v_orbit = sqrt(G * M / r)
/// At M=1000, r=80, G=8.0e-3 -> v ~ 1.0 cell/tick (1 orbit ~ 500 ticks ~ 8s
/// at 60 fps). Comfortable.
pub const G: f32 = 8.0e-3;

/// Plummer softening. `r^2 -> r^2 + eps^2` in the gravity formula. Without
/// it, a particle exactly on a body's position sees infinite acceleration
/// and the sim explodes. eps=0.5 cell is small enough to keep the visual
/// tidal gradient intact, large enough to absorb numerical noise.
pub const SOFTENING: f32 = 0.5;
pub const SOFTENING_SQ: f32 = SOFTENING * SOFTENING;

/// Event horizon radius in cells. Particles within this radius of a
/// BlackHole body are destroyed (cell becomes Vacuum, owning body loses
/// one from `particle_budget`).
pub const EVENT_HORIZON_RADIUS: f32 = 3.0;

/// Bodies in the sim.
#[derive(Clone, Debug)]
pub struct Body {
    /// Stable identity. Used to assign particles to bodies via
    /// `body_index[]` in the grid. Starts at 0, grows monotonically.
    pub id: u32,
    /// Sub-cell grid coordinates.
    pub position: Vec2,
    /// Grid cells per tick. Integrated with velocity Verlet each frame.
    pub velocity: Vec2,
    /// Body mass in mass units (1 mass unit = 1 Rock particle).
    /// For BlackHoles: set explicitly at spawn.
    /// For Planets: kept in sync with `particle_budget`.
    pub mass: f32,
    /// Visual radius. **Not a collision radius** — body-body collision
    /// is Phase 4. Currently only the event-horizon ring uses this for
    /// BlackHole rendering.
    pub radius: f32,
    /// Distinguishes BlackHole (destroys particles, has event horizon)
    /// from Planet (passive, owns particles, mass = particle count).
    pub kind: BodyKind,
    /// How many particles this body currently owns. Planets start at
    /// their spawn count and decrement as particles are destroyed. BHs
    /// stay at 0.
    pub particle_budget: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BodyKind {
    BlackHole,
    Planet,
}

impl Body {
    pub fn new_black_hole(id: u32, x: f32, y: f32, mass: f32) -> Self {
        Self {
            id,
            position: Vec2::new(x, y),
            velocity: Vec2::ZERO,
            mass,
            radius: EVENT_HORIZON_RADIUS,
            kind: BodyKind::BlackHole,
            particle_budget: 0,
        }
    }

    pub fn new_planet(id: u32, x: f32, y: f32, vx: f32, vy: f32, particle_count: u32) -> Self {
        Self {
            id,
            position: Vec2::new(x, y),
            velocity: Vec2::new(vx, vy),
            // Mass = particle count * 1 mass unit per particle (Rock).
            mass: particle_count as f32,
            radius: 0.0,
            kind: BodyKind::Planet,
            particle_budget: particle_count,
        }
    }

    /// True when this body destroys any particle whose grid cell lies
    /// within `EVENT_HORIZON_RADIUS`.
    #[inline]
    pub fn destroys_particles(&self) -> bool {
        matches!(self.kind, BodyKind::BlackHole)
    }

    /// Snap an f32 body position to the nearest integer grid cell.
    /// Used for event-horizon checks: the sim is cell-grained, so
    /// "particles within radius R of the body" means cells within R
    /// of the body's snapped cell + a half-cell tolerance on each
    /// axis (to handle sub-cell offsets).
    #[inline]
    pub fn cell_position(&self) -> (i32, i32) {
        (self.position.x.round() as i32, self.position.y.round() as i32)
    }
}

/// Cached acceleration on each body from the previous Verlet step.
/// Used to make velocity Verlet symplectic: we average old and new
/// accelerations rather than guessing the new one mid-step.
#[derive(Clone, Debug, Default)]
pub struct BodyAccel {
    pub accel: Vec2,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_hole_has_zero_particle_budget_and_event_horizon() {
        let bh = Body::new_black_hole(1, 100.0, 100.0, 1000.0);
        assert!(bh.destroys_particles());
        assert_eq!(bh.particle_budget, 0);
        assert!(bh.mass > 0.0);
        assert_eq!(bh.cell_position(), (100, 100));
    }

    #[test]
    fn planet_mass_equals_particle_count() {
        let p = Body::new_planet(2, 50.0, 50.0, 0.5, 0.0, 78);
        assert_eq!(p.particle_budget, 78);
        assert_eq!(p.mass, 78.0);
        assert!(!p.destroys_particles());
    }

    #[test]
    fn sub_cell_position_rounds_to_nearest_cell() {
        let bh = Body::new_black_hole(1, 100.6, 100.4, 1.0);
        assert_eq!(bh.cell_position(), (101, 100));
    }
}
