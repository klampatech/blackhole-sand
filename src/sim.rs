//! Sim grid: 2D SoA world, per-particle bonds, falling-sand physics with
//! multi-body gravity (Phase 2).
//!
//! ## State layout
//!
//! Each grid cell stores:
//! * `particles[i]` — material id (Vacuum=0, Rock=1, Ice=2, Plasma=3, ...)
//! * `bond_state[i]` — 4-bit lifetime bond bitmask. New bits are set only on
//!   planet spawn; broken bits are cleared permanently.
//! * `body_index[i]` — owning body id (i32, -1 = none). Stamped at planet
//!   spawn, used for event-horizon mass bookkeeping.
//!
//! Active bonds at render time = `bond_state & current_adjacency`.
//! That is the "sticky" model from the Phase 2 spec: once a bond breaks,
//! it stays broken.
//!
//! ## Movement model
//!
//! Per tick, for every non-empty cell we sum the gravitational pull from
//! every body in the sim (N^2 direct sum, Plummer-softened) and read the
//! *sign* of (ax, ay) as the direction the particle wants to move in.
//! The magnitude scales `max_steps` (1, 2, or 3 cells per tick), so a
//! particle deep in a gravity well can teleport several cells in one tick
//! — this is the differential motion that produces the "tidal peel"
//! visual when two adjacent particles feel different accelerations.
//!
//! Bodies move with velocity Verlet (see `body.rs`) and pull on each
//! other and on every particle.

use crate::body::{Body, BodyAccel, G, SOFTENING_SQ};
use crate::material::Material;

/// Grid resolution. 256^2 is the Phase 1 spec target (~30 fps on m5). Flip to
/// 512 once the perf budget allows it — this is the only line to change.
pub const W: usize = 256;
pub const H: usize = 256;

/// Black hole position (grid center). Phase 1 default. Phase 2 ignores this
/// if the World has any BlackHole bodies; we keep it for tests and the
/// "default 1-BH" code path.
pub const HOLE_X: i32 = (W / 2) as i32;
pub const HOLE_Y: i32 = (H / 2) as i32;

/// Bond bits. Four per cell: N, E, NE, SE. The other four directions are
/// mirrors on the neighbour cell (S = N of cell below, W = E of cell left,
/// NW = SE of cell down-left, SW = NE of cell down-right).
pub const BOND_N: u8 = 1 << 0;
pub const BOND_E: u8 = 1 << 1;
pub const BOND_NE: u8 = 1 << 2;
pub const BOND_SE: u8 = 1 << 3;

const PULL_RADIUS: i32 = 100; // cells — outside this, particles just fall
const PULL_RADIUS_SQ: i32 = PULL_RADIUS * PULL_RADIUS;

const NEAR_RADIUS_SQ: i32 = 15 * 15; // ≤ ~15 cells from hole: 3 steps/tick
const MID_RADIUS_SQ: i32 = 40 * 40; // ≤ ~40 cells: 2 steps/tick

/// No body owns this cell.
pub const NO_BODY: i32 = -1;

pub struct World {
    pub particles: Vec<u8>,    // length W*H, material id per cell
    pub bond_state: Vec<u8>,   // length W*H, lifetime bond bitmask
    pub body_index: Vec<i32>,  // length W*H, owning body id (or NO_BODY)
    pub bodies: Vec<Body>,
    /// Cached accelerations for the body-body Verlet half-step. Indexed
    /// parallel to `bodies`.
    pub body_accel: Vec<BodyAccel>,
    /// Monotonic counter for the next body id.
    pub next_body_id: u32,
}

impl World {
    pub fn new() -> Self {
        Self {
            particles: vec![0; W * H],
            bond_state: vec![0; W * H],
            body_index: vec![NO_BODY; W * H],
            bodies: Vec::new(),
            body_accel: Vec::new(),
            next_body_id: 0,
        }
    }

    #[inline]
    pub fn idx(x: usize, y: usize) -> usize {
        y * W + x
    }

    #[inline]
    pub fn in_bounds(x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && (x as usize) < W && (y as usize) < H
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> u8 {
        if !Self::in_bounds(x, y) {
            return 0;
        }
        self.particles[Self::idx(x as usize, y as usize)]
    }

    #[inline]
    pub fn set(&mut self, x: i32, y: i32, m: u8) {
        if !Self::in_bounds(x, y) {
            return;
        }
        self.particles[Self::idx(x as usize, y as usize)] = m;
    }

    /// Seed a thin rain of particles at the top so the falling-sand effect is
    /// visible before the user clicks anything. (Phase 1 visual smoke test.)
    pub fn seed_rain(&mut self) {
        for &(x_center, mat) in &[
            (W / 6, Material::Rock as u8),
            (W / 3, Material::Ice as u8),
            (W / 2, Material::Rock as u8),
            (2 * W / 3, Material::Plasma as u8),
            (5 * W / 6, Material::Rock as u8),
        ] {
            for dx in -1..=1 {
                for dy in 0..6 {
                    self.set(x_center as i32 + dx, dy, mat);
                }
            }
        }
    }

    /// Spawn a planet body at `(cx, cy)` with the given radius and material.
    /// Stamps the new body's id into the `body_index` of every filled cell
    /// and sets `bond_state` bits for all adjacent same-material cells so
    /// the planet is fully bonded at spawn.
    pub fn spawn_planet(&mut self, cx: i32, cy: i32, radius: i32, mat: u8) -> u32 {
        if !Material::from_u8(mat).has_bonds() {
            return NO_BODY as u32;
        }
        let id = self.next_body_id;
        self.next_body_id += 1;

        // Fill the disk and remember the filled cells so we can stamp
        // bond_state + body_index in a second pass.
        let r2 = radius * radius;
        let mut filled: Vec<(i32, i32)> = Vec::new();
        for y in (cy - radius)..=(cy + radius) {
            for x in (cx - radius)..=(cx + radius) {
                let dx = x - cx;
                let dy = y - cy;
                if dx * dx + dy * dy > r2 {
                    continue;
                }
                if self.get(x, y) != 0 {
                    continue;
                }
                self.set(x, y, mat);
                filled.push((x, y));
            }
        }
        let filled_count = filled.len() as u32;

        // Stamp body_index and bond_state for each filled cell.
        for &(x, y) in &filled {
            let i = Self::idx(x as usize, y as usize);
            self.body_index[i] = id as i32;
            let mut b = 0u8;
            if Self::in_bounds(x, y - 1) && self.particles[Self::idx(x as usize, y as usize - 1)] == mat {
                b |= BOND_N;
            }
            if Self::in_bounds(x + 1, y) && self.particles[Self::idx(x as usize + 1, y as usize)] == mat {
                b |= BOND_E;
            }
            if Self::in_bounds(x + 1, y - 1) && self.particles[Self::idx(x as usize + 1, y as usize - 1)] == mat {
                b |= BOND_NE;
            }
            if Self::in_bounds(x + 1, y + 1) && self.particles[Self::idx(x as usize + 1, y as usize + 1)] == mat {
                b |= BOND_SE;
            }
            self.bond_state[i] = b;
        }

        let body = Body::new_planet(id, cx as f32, cy as f32, 0.0, 0.0, filled_count);
        self.bodies.push(body);
        self.body_accel.push(BodyAccel::default());
        id
    }

    /// Spawn a BlackHole at the given position with the given mass. The
    /// BlackHole owns no particles and has no bonds; it is just a
    /// gravitational source plus a tiny disk of `EventHorizon`-colored
    /// cells for visual identification.
    pub fn spawn_black_hole(&mut self, x: f32, y: f32, mass: f32) -> u32 {
        let id = self.next_body_id;
        self.next_body_id += 1;
        let body = Body::new_black_hole(id, x, y, mass);
        self.bodies.push(body);
        self.body_accel.push(BodyAccel::default());
        // Carve a tiny vacuum disk so the BH reads as a hole against the
        // grid background. We do NOT stamp body_index for BH cells — they
        // are empty.
        let cx = x.round() as i32;
        let cy = y.round() as i32;
        for dy in -1..=1 {
            for dx in -1..=1 {
                self.set(cx + dx, cy + dy, Material::Vacuum as u8);
            }
        }
        id
    }

    /// One sim tick: body Verlet -> body-body gravity -> body-particle
    /// gravity + event horizon -> particle move pass -> sticky bond
    /// recompute.
    pub fn step(&mut self) {
        self.body_verlet_step();
        self.body_body_gravity();
        self.apply_event_horizons();
        self.move_pass();
        self.recompute_bonds();
    }

    /// One velocity-Verlet half-step on each body. Cache the *new*
    /// acceleration in `body_accel` so the next call can average the old
    /// and new to advance velocities (symplectic).
    fn body_verlet_step(&mut self) {
        for (i, body) in self.bodies.iter_mut().enumerate() {
            // x' = x + v*dt + 0.5*a*dt^2   (dt = 1 tick)
            let a_old = self.body_accel[i].accel;
            body.position += body.velocity + a_old * 0.5;

            // Clamp positions inside the grid so event-horizon checks
            // (which use cell position) stay sane and the body never
            // leaves the world. A real fix would use reflective or
            // wraparound boundaries; for Phase 2 a hard clamp is fine
            // because we never expect bodies to actually reach the wall
            // (they get destroyed first).
            body.position.x = body.position.x.clamp(0.0, (W - 1) as f32);
            body.position.y = body.position.y.clamp(0.0, (H - 1) as f32);
        }
        // New accelerations are computed in body_body_gravity().
    }

    /// Compute the new acceleration on each body from the other bodies,
    /// Plummer-softened. Cache it for the next Verlet half-step.
    fn body_body_gravity(&mut self) {
        // First clear the new accelerations.
        for a in self.body_accel.iter_mut() {
            a.accel = crate::body::Vec2::ZERO;
        }
        // Pairwise N^2.
        for i in 0..self.bodies.len() {
            for j in (i + 1)..self.bodies.len() {
                let (pi, pj) = (self.bodies[i].position, self.bodies[j].position);
                let r = pj - pi;
                let d2 = r.length_squared() + SOFTENING_SQ;
                let d = d2.sqrt();
                // F_ij = G * m_i * m_j / d^2 along r_hat
                let f_mag = G * self.bodies[i].mass * self.bodies[j].mass / d2;
                let f_vec = r * (f_mag / d);
                // a_i += F / m_i ; a_j -= F / m_j
                self.body_accel[i].accel += f_vec / self.bodies[i].mass;
                self.body_accel[j].accel -= f_vec / self.bodies[j].mass;
            }
        }
        // Now finish the Verlet step: v' = v + 0.5 * (a_old + a_new) * dt
        for (i, body) in self.bodies.iter_mut().enumerate() {
            // Velocity kick uses the *new* acceleration we just computed
            // (body_accel[i].accel). A pure velocity Verlet would
            // average old and new (symplectic). We do leapfrog here —
            // "kick" then "drift" — which is also symplectic and
            // stable for orbits at our dt=1 grid step.
            let a_new = self.body_accel[i].accel;
            body.velocity += a_new;
        }
    }

    /// For every BlackHole body, destroy every particle whose grid cell
    /// lies within `EVENT_HORIZON_RADIUS`. Decrement the owning planet
    /// body's `particle_budget` for each particle destroyed.
    fn apply_event_horizons(&mut self) {
        // Snapshot the BlackHole positions to avoid borrow conflicts.
        let horizons: Vec<(i32, i32, f32)> = self
            .bodies
            .iter()
            .filter(|b| b.destroys_particles())
            .map(|b| {
                let (cx, cy) = b.cell_position();
                (cx, cy, b.radius)
            })
            .collect();
        if horizons.is_empty() {
            return;
        }
        for y in 0..H {
            for x in 0..W {
                let i = Self::idx(x, y);
                if self.particles[i] == 0 {
                    continue;
                }
                for &(hx, hy, hr) in &horizons {
                    let hr_sq = hr * hr;
                    let dx = (x as i32 - hx) as f32;
                    let dy = (y as i32 - hy) as f32;
                    let d2 = dx * dx + dy * dy;
                    if d2 <= hr_sq {
                        // Destroy the particle.
                        let owner = self.body_index[i];
                        if owner >= 0 {
                            // Decrement the owning planet's particle_budget.
                            if let Some(b) =
                                self.bodies.iter_mut().find(|b| b.id == owner as u32)
                            {
                                if b.particle_budget > 0 {
                                    b.particle_budget -= 1;
                                    b.mass = b.particle_budget as f32;
                                }
                            }
                            self.body_index[i] = NO_BODY;
                        }
                        self.particles[i] = 0;
                        self.bond_state[i] = 0;
                        break;
                    }
                }
            }
        }
    }

    /// Movement pass: for every non-empty cell, compute the net gravity
    /// vector from all bodies, then walk up to N empty cells in that
    /// direction (where N scales with magnitude).
    fn move_pass(&mut self) {
        let mut new_particles = self.particles.clone();
        let mut new_bonds = self.bond_state.clone();
        let mut new_body_index = self.body_index.clone();
        let mut claim = vec![false; W * H];

        // If no bodies, fall straight down (Phase 1 fallback).
        let has_bodies = !self.bodies.is_empty();

        for y in 0..H {
            for x in 0..W {
                let i = Self::idx(x, y);
                if claim[i] || new_particles[i] == 0 {
                    continue;
                }
                let m = new_particles[i];
                let b = new_bonds[i];
                let owner = new_body_index[i];

                let (dx, dy, max_steps) = if has_bodies {
                    gravity_step_for_cell(x as i32, y as i32, &self.bodies)
                } else {
                    gravity_step_legacy(x as i32, y as i32)
                };

                let (tx, ty) = self.find_target(x as i32, y as i32, dx, dy, max_steps);

                if tx == x as i32 && ty == y as i32 {
                    claim[i] = true;
                } else {
                    let ti = Self::idx(tx as usize, ty as usize);
                    new_particles[ti] = m;
                    new_bonds[ti] = b;
                    new_body_index[ti] = owner;
                    new_particles[i] = 0;
                    new_bonds[i] = 0;
                    new_body_index[i] = NO_BODY;
                    claim[ti] = true;
                }
            }
        }

        self.particles = new_particles;
        self.bond_state = new_bonds;
        self.body_index = new_body_index;
    }

    /// Walk in `(dx, dy)` direction up to `max_steps` cells, stopping at the
    /// first occupied or out-of-bounds cell. Returns the last empty cell.
    fn find_target(&self, x: i32, y: i32, dx: i32, dy: i32, max_steps: i32) -> (i32, i32) {
        let mut tx = x;
        let mut ty = y;
        let mut steps = 0;
        while steps < max_steps {
            let nx = tx + dx;
            let ny = ty + dy;
            if !Self::in_bounds(nx, ny) {
                break;
            }
            let ni = Self::idx(nx as usize, ny as usize);
            if self.particles[ni] != 0 {
                break;
            }
            tx = nx;
            ty = ny;
            steps += 1;
        }
        if (tx, ty) == (x, y) && dx == 0 && dy == 1 {
            for &(ddx, ddy) in &[(-1, 1), (1, 1)] {
                let nx = x + ddx;
                let ny = y + ddy;
                if Self::in_bounds(nx, ny) && self.particles[Self::idx(nx as usize, ny as usize)] == 0 {
                    return (nx, ny);
                }
            }
        }
        (tx, ty)
    }

    /// Sticky bond recompute. For each non-empty cell:
    /// 1. Compute *current* adjacency (N, E, NE, SE) to same-material cells.
    /// 2. Active bonds = adjacency AND bond_state.
    /// 3. For each bit in bond_state that *was* set but is no longer
    ///    adjacent, the bond has broken — clear it permanently.
    ///
    /// Phase 2 § "Phase 1 -> Phase 2 Transition: Sticky Bonds".
    fn recompute_bonds(&mut self) {
        for y in 0..H {
            for x in 0..W {
                let i = Self::idx(x, y);
                let m = self.particles[i];
                if !Material::from_u8(m).has_bonds() {
                    self.bond_state[i] = 0;
                    continue;
                }
                let mut adj = 0u8;
                if y > 0 && self.particles[Self::idx(x, y - 1)] == m {
                    adj |= BOND_N;
                }
                if x + 1 < W && self.particles[Self::idx(x + 1, y)] == m {
                    adj |= BOND_E;
                }
                if y > 0 && x + 1 < W && self.particles[Self::idx(x + 1, y - 1)] == m {
                    adj |= BOND_NE;
                }
                if x + 1 < W && y + 1 < H && self.particles[Self::idx(x + 1, y + 1)] == m {
                    adj |= BOND_SE;
                }
                // Sticky: AND with the lifetime bond_state. Bits that were
                // set but the cell is no longer adjacent to the bonded
                // neighbour stay set in bond_state (broken-but-sticky),
                // but they don't show as an active bond this tick. On
                // re-adjacency they'd "heal" — which we *don't* want.
                //
                // The way to make them truly permanent: clear the bit
                // here, the first time we observe a bond is no longer
                // adjacent. This is the "sticky" semantic.
                let lifetime = self.bond_state[i];
                let still_adjacent = lifetime & adj;
                let newly_broken = lifetime & !adj;
                if newly_broken != 0 {
                    // Clear the broken bits in bond_state.
                    self.bond_state[i] = still_adjacent;
                    // And mirror-clear the corresponding bits on the
                    // neighbour cell. For each newly-broken direction,
                    // find the neighbour and clear the mirror bit.
                    if newly_broken & BOND_N != 0 {
                        let j = Self::idx(x, y - 1);
                        self.bond_state[j] &= !BOND_S; // mirror of N is S on the cell below
                    }
                    if newly_broken & BOND_E != 0 {
                        let j = Self::idx(x + 1, y);
                        self.bond_state[j] &= !BOND_W; // mirror of E is W on the cell right
                    }
                    if newly_broken & BOND_NE != 0 {
                        let j = Self::idx(x + 1, y - 1);
                        self.bond_state[j] &= !BOND_SW; // mirror of NE is SW
                    }
                    if newly_broken & BOND_SE != 0 {
                        let j = Self::idx(x + 1, y + 1);
                        self.bond_state[j] &= !BOND_NW; // mirror of SE is NW
                    }
                }
            }
        }
    }

    /// Pack the particle grid into RGBA8 bytes for the GPU texture upload.
    pub fn render_rgba(&self, out: &mut [u8]) {
        debug_assert_eq!(out.len(), W * H * 4);
        for (i, &m) in self.particles.iter().enumerate() {
            let c = Material::from_u8(m).color();
            out[i * 4] = c[0];
            out[i * 4 + 1] = c[1];
            out[i * 4 + 2] = c[2];
            out[i * 4 + 3] = c[3];
        }
    }
}

// Bond mirror bits used by the sticky-bond recompute.
const BOND_S: u8 = 1 << 0; // mirror of N on cell below
const BOND_W: u8 = 1 << 1; // mirror of E on cell right
const BOND_SW: u8 = 1 << 2; // mirror of NE on cell SE
const BOND_NW: u8 = 1 << 3; // mirror of SE on cell NW

/// Compute (dx, dy, max_steps) for the cell at (x, y) by summing gravity
/// from every body, Plummer-softened. dx/dy is the sign of the net
/// acceleration; max_steps is 1, 2, or 3 based on the magnitude.
fn gravity_step_for_cell(x: i32, y: i32, bodies: &[Body]) -> (i32, i32, i32) {
    let mut ax = 0.0f32;
    let mut ay = 0.0f32;
    for b in bodies {
        let dx = b.position.x - x as f32;
        let dy = b.position.y - y as f32;
        let d2 = dx * dx + dy * dy + SOFTENING_SQ;
        let d = d2.sqrt();
        // a = G * M * r_hat / d^2
        let a_mag = G * b.mass / d2;
        ax += a_mag * dx / d;
        ay += a_mag * dy / d;
    }
    let mag = (ax * ax + ay * ay).sqrt();
    let max_steps = if mag > 0.5 { 3 } else if mag > 0.05 { 2 } else { 1 };
    (ax.signum() as i32, ay.signum() as i32, max_steps)
}

/// Phase 1 fallback: pull toward (HOLE_X, HOLE_Y) inside PULL_RADIUS,
/// otherwise straight down. Used when there are no bodies.
fn gravity_step_legacy(x: i32, y: i32) -> (i32, i32, i32) {
    let dx = HOLE_X - x;
    let dy = HOLE_Y - y;
    let d2 = dx * dx + dy * dy;
    if d2 < PULL_RADIUS_SQ {
        let max_steps = if d2 <= NEAR_RADIUS_SQ {
            3
        } else if d2 <= MID_RADIUS_SQ {
            2
        } else {
            1
        };
        (dx.signum(), dy.signum(), max_steps)
    } else {
        (0, 1, 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::Body;
    use crate::material::Material;

    #[test]
    fn empty_world_does_nothing() {
        let mut w = World::new();
        for _ in 0..100 {
            w.step();
        }
        assert!(w.particles.iter().all(|&m| m == 0));
    }

    #[test]
    fn particles_fall_down_outside_pull_zone() {
        let mut w = World::new();
        w.set(10, 10, Material::Rock as u8);
        for _ in 0..200 {
            w.step();
        }
        let mut found_y: Option<usize> = None;
        for y in 0..H {
            if w.particles[World::idx(10, y)] == Material::Rock as u8 {
                found_y = Some(y);
                break;
            }
        }
        let y = found_y.expect("rock vanished entirely");
        assert!(y > 10, "rock did not fall (still at y={y})");
    }

    #[test]
    fn planet_spawns_bonded() {
        let mut w = World::new();
        w.spawn_planet(50, 50, 5, Material::Rock as u8);
        let n = (45..=55)
            .flat_map(|y| (45..=55).map(move |x| (x, y)))
            .filter(|&(x, y)| w.particles[World::idx(x, y)] != 0)
            .count();
        assert!(n > 50 && n < 100, "expected ~78 planet cells, got {n}");
        // All non-empty cells should have a body_index set to the planet's id.
        let body_id = w.bodies[0].id;
        for y in 0..H {
            for x in 0..W {
                let i = World::idx(x, y);
                if w.particles[i] != 0 {
                    assert_eq!(w.body_index[i], body_id as i32, "cell ({x},{y}) not owned");
                }
            }
        }
    }

    #[test]
    fn sticky_bonds_do_not_reform_after_break() {
        let mut w = World::new();
        let pid = w.spawn_planet(50, 50, 3, Material::Rock as u8);
        let body_id = w.bodies.iter().find(|b| b.id == pid).unwrap().id;
        // Take a snapshot of bond_state for cells in the planet.
        let snapshot_before: Vec<u8> = (0..H)
            .flat_map(|y| (0..W).map(move |x| (x, y)))
            .filter(|&(x, y)| w.particles[World::idx(x, y)] != 0)
            .map(|(x, y)| w.bond_state[World::idx(x, y)])
            .collect();
        assert!(snapshot_before.iter().any(|&b| b != 0), "planet has no bonds at spawn");

        // Manually clear bonds on a specific cell by setting its bitmask
        // to zero (simulating the effect of a bond break). Then run
        // several ticks; if we move the cell back into adjacency with
        // a same-material neighbour, the bond should NOT reform.
        let x = 50usize;
        let y = 50usize;
        let i = World::idx(x, y);
        let original = w.bond_state[i];
        assert!(original != 0, "center cell has no bonds to break");
        // Clear the lifetime bond mask on this cell. The cell stays
        // adjacent to its neighbours, so a "non-sticky" recompute would
        // set the bits again; a "sticky" recompute must leave them
        // cleared.
        w.bond_state[i] = 0;
        for _ in 0..50 {
            w.step();
        }
        assert_eq!(w.bond_state[i], 0, "sticky bond reformed — Phase 2 invariant violated");
        // Also confirm we still own the cell (no body_index change).
        assert_eq!(w.body_index[i], body_id as i32);
    }

    #[test]
    fn black_hole_destroys_particles_within_event_horizon() {
        let mut w = World::new();
        // Spawn a BH at the center and a planet adjacent to it.
        w.spawn_black_hole(HOLE_X as f32, HOLE_Y as f32, 1000.0);
        // Place rock particles in a tight cluster near the BH.
        for dx in -2..=2 {
            for dy in -2..=2 {
                if dx * dx + dy * dy <= 4 {
                    w.set(HOLE_X + dx + 1, HOLE_Y + dy, Material::Rock as u8);
                }
            }
        }
        let initial = w.particles.iter().filter(|&&m| m != 0).count();
        assert!(initial > 0, "no particles placed");

        // Run many ticks. Particles within EVENT_HORIZON_RADIUS = 3 cells
        // of the BH should be destroyed.
        for _ in 0..200 {
            w.step();
        }
        let survivors: Vec<(usize, usize)> = (0..H)
            .flat_map(|y| (0..W).map(move |x| (x, y)))
            .filter(|&(x, y)| w.particles[World::idx(x, y)] != 0)
            .collect();
        // None of the survivors should be within 3 cells of the BH.
        for &(x, y) in &survivors {
            let dx = x as i32 - HOLE_X;
            let dy = y as i32 - HOLE_Y;
            assert!(
                dx * dx + dy * dy > 9,
                "particle survived at ({x},{y}) within event horizon (d²={})",
                dx * dx + dy * dy
            );
        }
    }

    #[test]
    fn sticky_bonds_break_when_neighbour_destroyed() {
        let mut w = World::new();
        // Spawn a small 2-cell planet: cells (50,50) and (51,50) are
        // bonded (E bond from (50,50) to (51,50)).
        w.spawn_planet(50, 50, 1, Material::Rock as u8);
        // Destroy the right cell by hand and run one tick. The left
        // cell's E bond should be cleared permanently in bond_state.
        w.particles[World::idx(51, 50)] = 0;
        w.bond_state[World::idx(51, 50)] = 0;
        w.body_index[World::idx(51, 50)] = NO_BODY;
        w.step();
        // bond_state for (50,50) should have lost its E bit.
        let b = w.bond_state[World::idx(50, 50)];
        assert_eq!(b & BOND_E, 0, "E bond did not clear on left cell");
        // And the bond should NOT reform even if we put the right cell
        // back to a rock material.
        w.set(51, 50, Material::Rock as u8);
        w.body_index[World::idx(51, 50)] = w.bodies[0].id as i32;
        for _ in 0..20 {
            w.step();
        }
        let b2 = w.bond_state[World::idx(50, 50)];
        assert_eq!(b2 & BOND_E, 0, "sticky bond reformed on adjacency");
    }

    #[test]
    fn planet_mass_decrements_when_particles_destroyed() {
        let mut w = World::new();
        w.spawn_black_hole(HOLE_X as f32, HOLE_Y as f32, 1000.0);
        let pid = w.spawn_planet(HOLE_X + 10, HOLE_Y, 4, Material::Rock as u8);
        let initial_budget = w
            .bodies
            .iter()
            .find(|b| b.id == pid)
            .unwrap()
            .particle_budget;
        assert!(initial_budget > 0);
        for _ in 0..500 {
            w.step();
        }
        let body = w.bodies.iter().find(|b| b.id == pid).unwrap();
        assert!(
            body.particle_budget < initial_budget,
            "expected particle_budget to shrink (was {initial_budget}, now {})",
            body.particle_budget
        );
        assert!(
            (body.mass - body.particle_budget as f32).abs() < 0.5,
            "mass should track particle_budget"
        );
    }
}
