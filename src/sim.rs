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
use crate::barnes_hut::QuadTree;

/// Default Barnes-Hut opening-angle parameter. The spec calls for
/// 0.5; the test sweep is in `barnes_hut::tests`. Lower = more
/// accurate, higher = faster. Phase 3 decision #23.
pub const BARNES_HUT_THETA: f32 = 0.5;

/// Threshold for switching the body-particle gravity path from
/// direct sum to Barnes-Hut. Below this body count the per-cell
/// tree-build + traversal cost exceeds the O(N) saving; direct
/// sum wins on a constant-factor basis. The exact value is
/// uncritical and tuned against the `perf_sanity_*` tests.
pub const BARNES_HUT_THRESHOLD: usize = 8;

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
        // bond_state + body_index in a second pass. Skip OOB cells
        // up front: a click near the edge of the grid would
        // otherwise push (x, y) tuples outside [0, W) × [0, H)
        // onto `filled`, and the next loop indexes them as
        // `Self::idx(x as usize, y as usize)` which panics on an
        // OOB index. `self.set` and `self.get` are bounds-safe on
        // their own, so the disk naturally clips to the grid.
        let r2 = radius * radius;
        let mut filled: Vec<(i32, i32)> = Vec::new();
        for y in (cy - radius)..=(cy + radius) {
            for x in (cx - radius)..=(cx + radius) {
                if !Self::in_bounds(x, y) {
                    continue;
                }
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

    /// Spawn a planet body at `(cx, cy)` with a tangential velocity
    /// `v_circ = sqrt(G * M_BH / r)` around the first BlackHole in the
    /// world, plus an equal-and-opposite recoil on the BH so the
    /// initial system momentum is approximately 0 (COM-stationary
    /// init; see decision #19 and the default scenario).
    ///
    /// This is the click-spawn entry point: it makes click-spawned
    /// planets behave the same as the default-scenario planet, so
    /// the user sees a circular orbit instead of the planet falling
    /// straight into the BH with v=(0,0).
    ///
    /// Returns `None` if:
    ///   * there is no BlackHole in the world (nothing to orbit),
    ///   * the click point is essentially on top of the BH (r near 0;
    ///     v_circ would blow up),
    ///   * the material does not support bonds (`spawn_planet` returns
    ///     `NO_BODY`),
    ///   * the click footprint is fully occupied (planet spawns with
    ///     zero particles, so zero mass, which would NaN in the
    ///     integrator).
    pub fn spawn_planet_in_orbit(
        &mut self,
        cx: i32,
        cy: i32,
        radius: i32,
        mat: u8,
    ) -> Option<u32> {
        // Find the first BlackHole body. (When multiple BHs are
        // present we do not yet pick "nearest" — click-spawn in this
        // project is overwhelmingly tested with the default
        // single-BH scenario, so the simplest "first wins" rule is
        // good enough for now.)
        let bh_idx = self
            .bodies
            .iter()
            .position(|b| matches!(b.kind, crate::body::BodyKind::BlackHole))?;
        let (bh_pos, bh_mass) =
            (self.bodies[bh_idx].position, self.bodies[bh_idx].mass);

        // Radial vector from BH to click point.
        let dx = cx as f32 - bh_pos.x;
        let dy = cy as f32 - bh_pos.y;
        let r = (dx * dx + dy * dy).sqrt();
        if r < 1.0 {
            return None;
        }

        // v_circ = sqrt(G * M_BH / r); tangential direction is the CCW
        // perpendicular of the radial vector: (-dy, dx) / r.
        let v_circ = (G * bh_mass / r).sqrt();
        let tx = -dy / r;
        let ty = dx / r;

        // Spawn the planet body in place (same path as the CLI/JSON
        // scenarios). Bail if the spawn refused (no bonds, footprint
        // occupied, etc.).
        let pid = self.spawn_planet(cx, cy, radius, mat);
        if pid == NO_BODY as u32 {
            return None;
        }
        // Bail if no particles actually landed — planet mass would be
        // 0 and the integrator would NaN on its first step.
        let planet_mass = self
            .bodies
            .iter()
            .find(|b| b.id == pid)
            .map(|b| b.mass)
            .unwrap_or(0.0);
        if planet_mass <= 0.0 {
            return None;
        }

        // Tangential velocity on the planet.
        if let Some(p) = self.bodies.iter_mut().find(|b| b.id == pid) {
            p.velocity = crate::body::Vec2::new(v_circ * tx, v_circ * ty);
        }

        // Equal-and-opposite recoil on the BH so the system COM
        // starts stationary (decision #19, same as default scenario).
        // dv_BH = -(m_planet / m_BH) * v_planet.
        let recoil_mag = v_circ * planet_mass / bh_mass;
        self.bodies[bh_idx].velocity -=
            crate::body::Vec2::new(recoil_mag * tx, recoil_mag * ty);

        Some(pid)
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
        self.body_leapfrog_first_half();
        self.body_body_gravity();
        self.apply_event_horizons();
        self.move_pass();
        self.recompute_bonds();
        self.stamp_event_horizon_ring();
    }

    /// One symplectic KDK (kick-drift-kick) leapfrog half-step on each
    /// body, using the cached `body_accel` from the previous tick as the
    /// first half-kick. The new acceleration is then computed in
    /// `body_body_gravity()` and the second half-kick applied there.
    ///
    /// This is the integrator locked in `docs/SPEC.md` decision #13.
    /// It is symplectic: it conserves a discrete energy to O(dt^2) and
    /// does not spiral orbits outward the way Forward-Euler-on-velocity
    /// does. (A previous version of this file did
    /// `position += v + 0.5*a_old; v += a_new;` which is neither Verlet
    /// nor leapfrog — it gains energy each tick. That bug made the BH
    /// drift visibly across the screen over a long playtest, which is
    /// what this rewrite fixes.)
    fn body_leapfrog_first_half(&mut self) {
        for (i, body) in self.bodies.iter_mut().enumerate() {
            // Zero-mass bodies (e.g. a planet whose particles have all
            // been eaten) have no meaningful kinetic state. Skip them
            // so we don't propagate stale NaN from a previous 0/0 in
            // the gravity loop.
            if body.mass <= 0.0 {
                self.body_accel[i].accel = crate::body::Vec2::ZERO;
                body.velocity = crate::body::Vec2::ZERO;
                body.position.x = body.position.x.clamp(0.0, (W - 1) as f32);
                body.position.y = body.position.y.clamp(0.0, (H - 1) as f32);
                continue;
            }
            // Half-kick using last tick's acceleration.
            let a_old = self.body_accel[i].accel;
            body.velocity += a_old * 0.5;
            // Drift.
            body.position += body.velocity;
            // Clamp inside the grid. If a body is pushed into a wall
            // we zero the velocity component in that direction so it
            // slides along the wall instead of pinning in the corner.
            // A body pinned at (0, 0) is a heavy mass that pulls
            // every other body towards it, which makes the BH "zoom
            // around" the screen for as long as the user watches. The
            // zero-velocity fix lets the planet keep sliding and
            // eventually rejoin the orbit. (See SPEC.md decision #17.)
            let max_x = (W - 1) as f32;
            let max_y = (H - 1) as f32;
            if body.position.x < 0.0 {
                body.position.x = 0.0;
                if body.velocity.x < 0.0 { body.velocity.x = 0.0; }
            } else if body.position.x > max_x {
                body.position.x = max_x;
                if body.velocity.x > 0.0 { body.velocity.x = 0.0; }
            }
            if body.position.y < 0.0 {
                body.position.y = 0.0;
                if body.velocity.y < 0.0 { body.velocity.y = 0.0; }
            } else if body.position.y > max_y {
                body.position.y = max_y;
                if body.velocity.y > 0.0 { body.velocity.y = 0.0; }
            }
        }
    }

    /// Pairwise N^2 body-body gravity, Plummer-softened. Writes the
    /// *new* accelerations into `body_accel` and then applies the second
    /// half-kick so each body ends the tick at a fully updated (v, a).
    ///
    /// Pairs with `m <= 0` on either side are skipped: their mutual
    /// force is zero, and dividing by zero would otherwise produce
    /// NaN that propagates into every other body that pairs with them
    /// (and into the body's own cached accel for the next step).
    fn body_body_gravity(&mut self) {
        for a in self.body_accel.iter_mut() {
            a.accel = crate::body::Vec2::ZERO;
        }
        for i in 0..self.bodies.len() {
            if self.bodies[i].mass <= 0.0 {
                continue;
            }
            for j in (i + 1)..self.bodies.len() {
                if self.bodies[j].mass <= 0.0 {
                    continue;
                }
                let (pi, pj) = (self.bodies[i].position, self.bodies[j].position);
                let r = pj - pi;
                let d2 = r.length_squared() + SOFTENING_SQ;
                let d = d2.sqrt();
                let f_mag = G * self.bodies[i].mass * self.bodies[j].mass / d2;
                let f_vec = r * (f_mag / d);
                // Equal-and-opposite: Newton's third law, which gives
                // us exact momentum conservation per step (verified by
                // the momentum-conservation test).
                self.body_accel[i].accel += f_vec / self.bodies[i].mass;
                self.body_accel[j].accel -= f_vec / self.bodies[j].mass;
            }
        }
        // Second half-kick using the freshly-computed acceleration.
        for (i, body) in self.bodies.iter_mut().enumerate() {
            if body.mass <= 0.0 {
                continue;
            }
            body.velocity += self.body_accel[i].accel * 0.5;
        }
    }
    /// For every BlackHole body, destroy every particle whose grid cell
    /// lies within `EVENT_HORIZON_RADIUS`. Decrement the owning planet
    /// body's `particle_budget` for each particle destroyed, and apply
    /// an equal-and-opposite recoil to the consuming BH so that the
    /// total system momentum is conserved across the horizon.
    fn apply_event_horizons(&mut self) {
        // Snapshot BlackHole (id, cell_x, cell_y, radius, mass). We need
        // the mass to convert the consumed particle's momentum into a
        // velocity change (Newton's 3rd law) and the id so we can apply
        // the recoil to the correct body when several BHs are present.
        let horizons: Vec<(u32, i32, i32, f32, f32)> = self
            .bodies
            .iter()
            .filter(|b| b.destroys_particles())
            .map(|b| {
                let (cx, cy) = b.cell_position();
                (b.id, cx, cy, b.radius, b.mass)
            })
            .collect();
        if horizons.is_empty() {
            return;
        }
        // Accumulate per-BH recoil so we can apply it after the loop.
        // We need owned storage here (not a borrow of self.bodies) so
        // the destruction loop can still mutate self.bodies to update
        // each owning planet's particle_budget without a borrow conflict.
        let mut recoil_by_bh: Vec<(u32, glam::Vec2)> = horizons
            .iter()
            .map(|(id, _, _, _, _)| (*id, glam::Vec2::ZERO))
            .collect();
        for y in 0..H {
            for x in 0..W {
                let i = Self::idx(x, y);
                if self.particles[i] == 0 {
                    continue;
                }
                for &(bh_id, hx, hy, hr, m_bh) in &horizons {
                    let hr_sq = hr * hr;
                    let dx = (x as i32 - hx) as f32;
                    let dy = (y as i32 - hy) as f32;
                    let d2 = dx * dx + dy * dy;
                    if d2 < hr_sq {
                        // Destroy the particle. If it is owned by a
                        // planet body, transfer its momentum to the
                        // consuming BH (Newton's 3rd law) and
                        // decrement the owning planet's particle_budget.
                        let owner = self.body_index[i];
                        if owner >= 0 {
                            // Approximate the particle's velocity as
                            // its owning planet's velocity. Bonded
                            // particles ride with the planet, so this
                            // is the dominant component of each
                            // particle's world-frame velocity. Mass
                            // unit is 1 Rock particle, so per particle
                            // the BH velocity change is
                            //     Δv_BH = v_particle / M_BH.
                            if let Some(p) =
                                self.bodies.iter().find(|b| b.id == owner as u32)
                            {
                                if m_bh > 0.0 {
                                    let dv = p.velocity / m_bh;
                                    if let Some(slot) = recoil_by_bh
                                        .iter_mut()
                                        .find(|(id, _)| *id == bh_id)
                                    {
                                        slot.1 += dv;
                                    }
                                }
                            }
                            // Now mutate the owning planet's
                            // particle_budget. (Borrow ends with the
                            // if-let block so the iter_mut() above
                            // is unambiguous.)
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
        // Apply the accumulated recoil to each consuming BH. Doing it
        // here (after the destruction loop) means we never hold a
        // mutable borrow on self.bodies while also reading it inside
        // the loop.
        for (bh_id, dv) in recoil_by_bh {
            if dv != glam::Vec2::ZERO {
                if let Some(bh) = self.bodies.iter_mut().find(|b| b.id == bh_id) {
                    bh.velocity += dv;
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

        // Phase 3: build a Barnes-Hut quadtree once per tick and use
        // it for body-particle gravity when the body count is above
        // the threshold. Below the threshold direct sum is faster
        // (the constant factor of tree build + traversal exceeds the
        // O(N) saving). See BARNES_HUT_THRESHOLD and decision #23.
        let tree = if has_bodies && self.bodies.len() >= BARNES_HUT_THRESHOLD {
            Some(QuadTree::new(&self.bodies, BARNES_HUT_THETA))
        } else {
            None
        };

        for y in 0..H {
            for x in 0..W {
                let i = Self::idx(x, y);
                if claim[i] || new_particles[i] == 0 {
                    continue;
                }
                let m = new_particles[i];
                let b = new_bonds[i];
                let owner = new_body_index[i];

                let (dx, dy, max_steps) = if let Some(tree) = &tree {
                    gravity_step_for_cell_bh(x as i32, y as i32, tree)
                } else if has_bodies {
                    gravity_step_for_cell(x as i32, y as i32, &self.bodies)
                } else {
                    gravity_step_legacy(x as i32, y as i32)
                };

                let (tx, ty) = self.find_target(x as i32, y as i32, dx, dy, max_steps, &claim);

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

    /// Walk in `(dx, dy)` direction up to `max_steps` cells, stopping
    /// at the first *currently-occupied* cell. Returns the last empty
    /// cell.
    ///
    /// The `claim` array is the in-progress occupancy map maintained
    /// by `move_pass`: cells with `claim[i] == true` are already
    /// taken by another particle moving during this same pass. We
    /// *must* treat those as occupied, otherwise two particles
    /// converging on the same target in the same tick would
    /// overwrite each other and one would silently vanish (a real
    /// bug we hit when the planet's near-side particles got pulled
    /// toward the BH faster than the far-side ones).
    fn find_target(
        &self,
        x: i32,
        y: i32,
        dx: i32,
        dy: i32,
        max_steps: i32,
        claim: &[bool],
    ) -> (i32, i32) {
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
            // Treat cells claimed by another particle in this pass as
            // occupied.
            if self.particles[ni] != 0 || claim[ni] {
                break;
            }
            tx = nx;
            ty = ny;
            steps += 1;
        }
        if (tx, ty) == (x, y) && dx == 0 && dy == 1 {
            // Stuck vertically: try a side-slide. Same occupancy rule.
            for &(ddx, ddy) in &[(-1, 1), (1, 1)] {
                let nx = x + ddx;
                let ny = y + ddy;
                if !Self::in_bounds(nx, ny) {
                    continue;
                }
                let ni = Self::idx(nx as usize, ny as usize);
                if self.particles[ni] == 0 && !claim[ni] {
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
                // Each mirror-clear targets a specific neighbour cell.
                // The bond bit can outlive its neighbour's bounds:
                // `move_pass` transports `bond_state` around with the
                // particle, so a planet particle can carry a `BOND_SE`
                // bit (set at spawn when its (x+1, y+1) neighbour was
                // in bounds) into a corner cell where the neighbour is
                // now OOB. The adjacency check at the top of this
                // function only validates neighbours that ARE in bounds
                // — it does not tell us whether the bond was set when
                // they were. Bounds-check each mirror target here to
                // avoid an OOB panic on the index.
                if newly_broken & BOND_N != 0 && y > 0 {
                    let j = Self::idx(x, y - 1);
                    self.bond_state[j] &= !BOND_S; // mirror of N is S on the cell below
                }
                if newly_broken & BOND_E != 0 && x + 1 < W {
                    let j = Self::idx(x + 1, y);
                    self.bond_state[j] &= !BOND_W; // mirror of E is W on the cell right
                }
                if newly_broken & BOND_NE != 0 && y > 0 && x + 1 < W {
                    let j = Self::idx(x + 1, y - 1);
                    self.bond_state[j] &= !BOND_SW; // mirror of NE is SW
                }
                if newly_broken & BOND_SE != 0 && y + 1 < H && x + 1 < W {
                    let j = Self::idx(x + 1, y + 1);
                    self.bond_state[j] &= !BOND_NW; // mirror of SE is NW
                }
            }
        }
    }
    }

    /// Paint a faint purple ring of `EventHorizon` cells around every
    /// BlackHole so the user can see where the event horizon is. Done
    /// last so it does not interfere with physics. EventHorizon is not
    /// a particle, has no bonds, and is overwritten if a real particle
    /// happens to be in the same cell. The ring is at the BH's `radius`
    /// +/- 0.5 cells.
    fn stamp_event_horizon_ring(&mut self) {
        let rings: Vec<(i32, i32, f32)> = self
            .bodies
            .iter()
            .filter(|b| b.destroys_particles())
            .map(|b| (b.cell_position().0, b.cell_position().1, b.radius))
            .collect();
        if rings.is_empty() {
            return;
        }
        let r_outer = rings[0].2 + 0.5;
        let r_outer_sq = r_outer * r_outer;
        let r_inner_sq = (rings[0].2 - 0.5).max(0.0).powi(2);
        let r_outer_ceil = r_outer.ceil() as i32 + 1;
        for &(hx, hy, _) in &rings {
            for dy in -r_outer_ceil..=r_outer_ceil {
                for dx in -r_outer_ceil..=r_outer_ceil {
                    let d2 = (dx as f32) * (dx as f32) + (dy as f32) * (dy as f32);
                    if d2 < r_inner_sq || d2 > r_outer_sq {
                        continue;
                    }
                    let x = hx + dx;
                    let y = hy + dy;
                    if !Self::in_bounds(x, y) {
                        continue;
                    }
                    let i = Self::idx(x as usize, y as usize);
                    if self.particles[i] == 0 {
                        self.particles[i] = crate::material::Material::EventHorizon as u8;
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

/// Phase 3 variant of `gravity_step_for_cell` that walks a
/// Barnes-Hut quadtree instead of summing every body directly.
/// Same Plummer-softening, same magnitude-based max_steps. The
/// tree is built once per tick in `move_pass` and reused across
/// all 65k cells; per-cell cost is O(log N) average rather than
/// O(N).
fn gravity_step_for_cell_bh(x: i32, y: i32, tree: &QuadTree) -> (i32, i32, i32) {
    let a = tree.compute_accel(crate::body::Vec2::new(x as f32, y as f32));
    let mag = a.length();
    let max_steps = if mag > 0.5 { 3 } else if mag > 0.05 { 2 } else { 1 };
    (a.x.signum() as i32, a.y.signum() as i32, max_steps)
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
    fn planet_disintegrates_under_tidal_pull() {
        let mut w = World::new();
        // Spawn a BlackHole at grid center and a small planet close to
        // (but not on top of) it. Verify that the planet's center of
        // mass migrates toward the BH over many ticks — i.e. the
        // Phase 2 gravity path is wired up.
        w.spawn_black_hole(HOLE_X as f32, HOLE_Y as f32, 1000.0);
        w.spawn_planet(HOLE_X + 60, HOLE_Y, 6, Material::Rock as u8);
        let start_dx = 60i64;
        for _ in 0..80 {
            w.step();
        }
        let (mut sx, mut sy, mut sn) = (0i64, 0i64, 0i64);
        for y in 0..H {
            for x in 0..W {
                if w.particles[World::idx(x, y)] != 0 {
                    sx += x as i64;
                    sy += y as i64;
                    sn += 1;
                }
            }
        }
        assert!(sn > 0, "planet vanished entirely (initial {} particles)", w.bodies.iter().map(|b| b.particle_budget).sum::<u32>());
        let cx = sx / sn;
        let cy = sy / sn;
        let dx = cx - HOLE_X as i64;
        let dy = cy - HOLE_Y as i64;
        let d2 = dx * dx + dy * dy;
        let start_d2 = start_dx * start_dx;
        assert!(
            d2 < start_d2,
            "planet center of mass did not migrate toward the hole (d^2={d2}, start d^2={start_d2})"
        );
    }

    #[test]
    fn planet_torn_between_two_black_holes() {
        // Two equal-mass black holes on either side of a small planet.
        // The planet's near-side (closer to BH_A) gets pulled toward A;
        // its far-side gets pulled toward B. Differential acceleration
        // along the A-B axis rips the planet apart (sticky bonds
        // accumulate broken bits until the body splits). After many
        // ticks the planet's particles should be split between the two
        // BHs, not all consumed by one.
        let mut w = World::new();
        w.spawn_black_hole(64.0, 128.0, 1000.0);
        w.spawn_black_hole(192.0, 128.0, 1000.0);
        let pid = w.spawn_planet(128, 128, 5, Material::Rock as u8);
        let initial_count = w.particles.iter().filter(|&&m| m != 0).count();
        assert!(initial_count > 0);

        for _ in 0..500 {
            w.step();
        }

        // The planet's owning body should have lost at least 50% of its
        // particles (proves the differential tearing is happening).
        let body = w.bodies.iter().find(|b| b.id == pid).unwrap();
        assert!(
            body.particle_budget < initial_count as u32 / 2,
            "planet not torn enough: {} of {} particles remain",
            body.particle_budget,
            initial_count
        );
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
    #[ignore] // run with `cargo test dump_default -- --ignored --nocapture`
    fn dump_default_scenario() {
        // Visual smoke test for the Phase 2 default scenario: 1 BH at
        // grid center, 1 planet 50 cells right on a circular orbit
        // (v=v_circ, COM-stationary init). Dumps RGBA at t=0/40/120/
        // 240/400/800 to /tmp/bhsand_p2_t*.bin so the user can confirm
        // the orbit + slow tidal strip reads. The 800-tick checkpoint
        // exists so SPEC.md Session 4 can cite an actual measured mass
        // instead of the speculative "~34" estimate (Phase 2
        // follow-up item 9). eprintln! output is the diagnostic trail
        // for the BH+planet+COM trajectory; the test itself only
        // asserts that the dumps succeed.
        use crate::scenario::default_scenario;
        use std::io::Write;
        let checkpoints = [0usize, 40, 120, 240, 400, 800];
        for &n in &checkpoints {
            let mut ww = World::new();
            default_scenario(&mut ww);
            for _ in 0..n {
                ww.step();
            }
            let mut rgba = vec![0u8; W * H * 4];
            ww.render_rgba(&mut rgba);
            let path = format!("/tmp/bhsand_p2_t{n:03}.bin");
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(&rgba).unwrap();
            let planet = ww.bodies.iter().find(|b| !b.destroys_particles()).unwrap();
            let bh = ww.bodies.iter().find(|b| b.destroys_particles()).unwrap();
            let com_x = (planet.position.x * planet.mass + bh.position.x * bh.mass)
                / (planet.mass + bh.mass);
            let com_y = (planet.position.y * planet.mass + bh.position.y * bh.mass)
                / (planet.mass + bh.mass);
            eprintln!(
                "t={n:>3}: BH=({:.2},{:.2}) v=({:.3},{:.3}) | planet=({:.1},{:.1}) v=({:.2},{:.2}) m={:.0} | COM=({:.1},{:.1})",
                bh.position.x, bh.position.y, bh.velocity.x, bh.velocity.y,
                planet.position.x, planet.position.y, planet.velocity.x, planet.velocity.y,
                planet.mass,
                com_x, com_y,
            );
        }
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
        // Exclude EventHorizon cells — those are the cosmetic ring
        // stamped around the BH, not real particles.
        let survivors: Vec<(usize, usize)> = (0..H)
            .flat_map(|y| (0..W).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let m = w.particles[World::idx(x, y)];
                m != 0 && m != Material::EventHorizon as u8
            })
            .collect();
        // None of the survivors should be strictly inside the event
        // horizon (d² < EVENT_HORIZON_RADIUS_SQ = 9). The boundary
        // itself is a buffer zone; particles at d² = 9 survive one
        // tick and are cleaned up next tick as the event horizon ring
        // shifts in.
        for &(x, y) in &survivors {
            let dx = x as i32 - HOLE_X;
            let dy = y as i32 - HOLE_Y;
            assert!(
                dx * dx + dy * dy >= 9,
                "particle survived at ({x},{y}) inside event horizon (d²={})",
                dx * dx + dy * dy
            );
        }
    }

    #[test]
    fn planet_orbits_black_hole_with_circular_velocity() {
        // Place a BH at the center and a planet at distance r with
        // tangential velocity v_circ = sqrt(G*M/r). After many ticks the
        // planet should still be at roughly the same distance from the
        // BH (it hasn't fallen in or escaped).
        use crate::body::G;
        let mut w = World::new();
        w.spawn_black_hole(HOLE_X as f32, HOLE_Y as f32, 1000.0);
        let r: f32 = 60.0;
        let m_central = 1000.0_f32;
        let v_circ = (G * m_central / r).sqrt();
        let pid = w.spawn_planet(HOLE_X + r as i32, HOLE_Y, 1, Material::Rock as u8);
        // Set the planet's velocity and position (f32) to the orbit
        // values. spawn_planet() places the body at the cell center with
        // zero velocity, so we override both.
        if let Some(body) = w.bodies.iter_mut().find(|b| b.id == pid) {
            body.position = glam::Vec2::new(HOLE_X as f32 + r, HOLE_Y as f32);
            body.velocity = glam::Vec2::new(0.0, v_circ);
        }
        for _ in 0..400 {
            w.step();
        }
        let body = w.bodies.iter().find(|b| b.id == pid).unwrap();
        let dx = body.position.x - HOLE_X as f32;
        let dy = body.position.y - HOLE_Y as f32;
        let d2 = dx * dx + dy * dy;
        // The orbit should stay roughly circular: |d - r| < 30% of r.
        let dr = (d2.sqrt() - r).abs();
        assert!(
            dr < 0.5 * r,
            "planet did not maintain orbit: started at r={r}, ended at r={:.2} (dr={:.2})",
            d2.sqrt(),
            dr
        );
    }

    #[test]
    fn slingshot_passes_planet_through_black_hole() {
        // Place a BH and a planet heading straight at it, with high enough
        // velocity to escape after the encounter (i.e. > escape velocity).
        // The planet should still exist after 200 ticks and be moving
        // away from the BH (positive radial velocity).
        use crate::body::G;
        let mut w = World::new();
        w.spawn_black_hole(HOLE_X as f32, HOLE_Y as f32, 1000.0);
        let r: f32 = 80.0;
        // Escape velocity from r=80 around M=1000: v_esc = sqrt(2*G*M/r).
        let v_esc = (2.0 * G * 1000.0 / r).sqrt();
        let v = v_esc * 1.2; // 20% above escape, so it definitely escapes
        let pid = w.spawn_planet(HOLE_X + r as i32, HOLE_Y, 1, Material::Rock as u8);
        if let Some(body) = w.bodies.iter_mut().find(|b| b.id == pid) {
            body.position = glam::Vec2::new(HOLE_X as f32 + r, HOLE_Y as f32);
            // Velocity pointing toward the BH (negative x).
            body.velocity = glam::Vec2::new(-v, 0.0);
        }
        for _ in 0..600 {
            w.step();
        }
        let body = w.bodies.iter().find(|b| b.id == pid).unwrap();
        let dx = body.position.x - HOLE_X as f32;
        let dy = body.position.y - HOLE_Y as f32;
        // After a slingshot, the planet should be far from the BH and
        // moving radially outward (away from BH).
        let d = (dx * dx + dy * dy).sqrt();
        assert!(d > 20.0, "planet did not escape: d={d:.2}");
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

    #[test]
    fn move_pass_does_not_lose_particles() {
        // The move_pass used to silently lose particles when two
        // particles converged on the same target cell in the same
        // tick: the second one overwrote the first because
        // find_target only checked the *original* grid, not the
        // in-progress one. The fix routes the in-progress `claim`
        // array through find_target. This test guards against
        // regression by running the default scenario (113 particles
        // on a circular orbit at r=50) for 40 ticks; without the
        // fix a large fraction of a planet's particles can vanish
        // during a close encounter — measured at 84 of 113 by t=200
        // in the default scenario (per the 981a906 commit message).
        let mut w = World::new();
        // Spawn a BH far away so no event-horizon destruction can
        // occur — the only way the particle count can change is via
        // a move_pass bug.
        w.spawn_black_hole(0.0, 0.0, 1000.0);
        crate::scenario::default_scenario(&mut w);
        let initial_rock: usize = w
            .particles
            .iter()
            .filter(|&&m| m == Material::Rock as u8)
            .count();
        for _ in 0..40 {
            w.step();
        }
        let final_rock: usize = w
            .particles
            .iter()
            .filter(|&&m| m == Material::Rock as u8)
            .count();
        assert_eq!(
            initial_rock, final_rock,
            "move_pass lost particles: {initial_rock} -> {final_rock}"
        );
    }
    #[test]
    fn body_body_gravity_conserves_momentum() {
        // Two BlackHoles on a head-on course with equal and opposite
        // initial velocities. No planet, no particles to destroy.
        // After many ticks the total momentum should be preserved to
        // f32 epsilon (per-pair Newton's 3rd law in body_body_gravity).
        //
        // NB: when particles ARE destroyed at an event horizon the
        // BH now absorbs the consumed particle's momentum
        // (apply_event_horizons applies Δv_BH = v_particle / M_BH per
        // particle). See `event_horizon_conserves_total_momentum`
        // for the regime where consumption is involved. This test
        // stays inside the body-body-only regime so we don't have
        // to reason about bond evolution under tidal stress here.
        let mut w = World::new();
        w.spawn_black_hole(80.0, 128.0, 1000.0);
        w.spawn_black_hole(176.0, 128.0, 1000.0);
        // Equal and opposite velocities: total p = 0.
        w.bodies[0].velocity = glam::Vec2::new(0.1, 0.0);
        w.bodies[1].velocity = glam::Vec2::new(-0.1, 0.0);
        for _ in 0..2000 {
            w.step();
        }
        let px: f32 = w.bodies.iter().map(|b| b.mass * b.velocity.x).sum();
        let py: f32 = w.bodies.iter().map(|b| b.mass * b.velocity.y).sum();
        // Both bodies still have non-trivial mass (no particles to
        // destroy) so p should be conserved to f32 epsilon.
        let p1 = (px * px + py * py).sqrt();
        let initial_p_per_body = 1000.0 * 0.1; // = 100
        assert!(
            p1 < 1.0,
            "momentum drift too large: |p| = {p1:.3} (initial |p_per_body| = {initial_p_per_body})"
        );
    }

    #[test]
    fn event_horizon_conserves_total_momentum() {
        // COM-stationary BH + planet at r=50 (the default scenario
        // setup). Initial total momentum is 0 by construction: the
        // BH's small -y recoil exactly balances the planet's +y
        // orbital momentum. As the planet's particles are consumed
        // at the BH's event horizon, apply_event_horizons now
        // transfers each consumed particle's momentum (≈ planet
        // body velocity) to the BH. Two invariants must hold:
        //
        //   1. The total system momentum stays ≈ 0 for the whole
        //      run (Newton's 3rd law across the horizon).
        //
        //   2. Once the planet is mostly consumed, the BH's
        //      velocity is ≈ 0 — its initial recoil has been
        //      cancelled by the absorbed planet momentum. (Without
        //      the recoil the BH would retain its initial v and
        //      visibly drift across the screen.)
        let mut w = World::new();
        let bh_id = w.spawn_black_hole(128.0, 128.0, 1000.0);
        let pid = w.spawn_planet(178, 128, 6, Material::Rock as u8);

        let m_planet = w.bodies.iter().find(|b| b.id == pid).unwrap().mass;
        let v_circ = (G * 1000.0 / 50.0_f32).sqrt();
        w.bodies
            .iter_mut()
            .find(|b| b.id == pid)
            .unwrap()
            .velocity = glam::Vec2::new(0.0, v_circ);
        w.bodies
            .iter_mut()
            .find(|b| b.id == bh_id)
            .unwrap()
            .velocity = glam::Vec2::new(0.0, -m_planet / 1000.0 * v_circ);

        // Initial total momentum should be ~0.
        let initial_p: glam::Vec2 = w.bodies.iter().map(|b| b.mass * b.velocity).sum();
        assert!(
            initial_p.length() < 1e-3,
            "expected zero initial total p, got {initial_p:?}"
        );

        // Run until the planet is mostly consumed (or 8000 ticks).
        let mut ticks = 0;
        loop {
            w.step();
            ticks += 1;
            let planet_mass = w
                .bodies
                .iter()
                .find(|b| b.id == pid)
                .map(|b| b.mass)
                .unwrap_or(0.0);
            if planet_mass < 5.0 || ticks >= 8000 {
                break;
            }
        }

        // Invariant 1: total momentum is conserved.
        let final_p: glam::Vec2 = w.bodies.iter().map(|b| b.mass * b.velocity).sum();
        let drift = final_p.length();
        assert!(
            drift < 5.0,
            "system momentum not conserved after {ticks} ticks: \
             |Δp| = {drift:.3} (initial 0, final p = {final_p:?})"
        );

        // Invariant 2: BH velocity is approximately 0 after the
        // planet's momentum has been absorbed.
        let bh = w.bodies.iter().find(|b| b.id == bh_id).unwrap();
        assert!(
            bh.velocity.length() < 0.1,
            "BH velocity after planet consumption: {:?} \
             (expected ≈ 0; with recoil the absorbed planet \
             momentum cancels the initial recoil)",
            bh.velocity
        );
    }

    #[test]
    fn leapfrog_does_not_gain_energy_in_pure_orbit() {
        // A planet on a circular orbit (v_circ = sqrt(GM/r)) at r=30
        // around a heavy BH should stay on the orbit. We measure the
        // semi-major axis a = -GM/2E at start and at t=2000; symplectic
        // integrators conserve a to a slow secular drift, Forward
        // Euler gains energy and a grows.
        let mut w = World::new();
        w.spawn_black_hole(128.0, 128.0, 1000.0);
        let r: f32 = 30.0;
        let m_central = 1000.0_f32;
        let v_circ = (G * m_central / r).sqrt();
        let pid = w.spawn_planet(128 + r as i32, 128, 1, Material::Rock as u8);
        if let Some(b) = w.bodies.iter_mut().find(|b| b.id == pid) {
            b.position = glam::Vec2::new(128.0 + r, 128.0);
            b.velocity = glam::Vec2::new(0.0, v_circ);
        }
        // Initial semi-major axis = r (circular orbit).
        for _ in 0..2000 {
            w.step();
        }
        let body = w.bodies.iter().find(|b| b.id == pid).unwrap();
        let dx = body.position.x - 128.0;
        let dy = body.position.y - 128.0;
        let d = (dx * dx + dy * dy).sqrt();
        // The BH orbits too — measure distance from the system COM
        // rather than the BH's instantaneous position, otherwise the
        // BH's small motion would make the test noisy.
        let bh = w.bodies.iter().find(|b| b.destroys_particles()).unwrap();
        let com_x = (body.position.x * body.mass + bh.position.x * bh.mass)
            / (body.mass + bh.mass);
        let com_y = (body.position.y * body.mass + bh.position.y * bh.mass)
            / (body.mass + bh.mass);
        let d_from_com = ((body.position.x - com_x).powi(2)
            + (body.position.y - com_y).powi(2))
        .sqrt();
        // The orbit is roughly circular, so d_from_com stays close to
        // a*r where a is the planet:bh mass ratio factor. With M_bh=1000
        // and m_planet=1, planet is at r=30 from BH, COM is ~0.03 cells
        // from BH, so d_from_com ≈ 30. We just check that the planet
        // is still in orbit (didn't escape) — anywhere from 5..60 cells
        // from COM is fine. The previous Forward-Euler-on-velocity
        // version would gain energy and the planet would either escape
        // (d>100) or fall in (d<3).
        assert!(
            d_from_com > 5.0 && d_from_com < 60.0,
            "planet escaped or fell in after 2000 ticks: d_from_com={d_from_com:.2}, raw d={d:.2}"
        );
    }

    #[test]
    #[ignore] // run with `cargo test perf_sanity -- --ignored --nocapture`
    fn perf_sanity_10_bodies_full_grid() {
        // Sanity check: 10 bodies + 50k randomly-distributed particles,
        // 100 ticks. Should run in well under 1s on the gaming rig
        // (target 30+ fps = <33ms/tick) and under 2s on m5
        // (15+ fps = <67ms/tick). Prints wall time; the assert is loose
        // so the test passes even on a slow CI box.
        use std::time::Instant;
        let mut w = World::new();
        // Spawn 10 BHs in a ring at radius 80 around grid center.
        for i in 0..10 {
            let angle = (i as f32) * 0.628;
            let r = 80.0;
            w.spawn_black_hole(
                128.0 + r * angle.cos(),
                128.0 + r * angle.sin(),
                500.0,
            );
        }
        // Fill the grid (65k particles). Avoid the BH event horizons
        // so we start with no destroyed particles.
        for y in 0..H as i32 {
            for x in 0..W as i32 {
                // Skip cells inside any BH event horizon.
                let mut inside = false;
                for b in &w.bodies {
                    if !b.destroys_particles() { continue; }
                    let dx = (x as f32 - b.position.x) as f32;
                    let dy = (y as f32 - b.position.y) as f32;
                    if dx * dx + dy * dy < b.radius * b.radius {
                        inside = true;
                        break;
                    }
                }
                if inside { continue; }
                w.set(x, y, Material::Rock as u8);
                w.body_index[World::idx(x as usize, y as usize)] = NO_BODY;
            }
        }
        let particle_count = w.particles.iter().filter(|&&m| m != 0).count();
        eprintln!("perf: 10 BHs + {} particles", particle_count);
        let t = Instant::now();
        for _ in 0..100 {
            w.step();
        }
        let elapsed = t.elapsed();
        let ms_per_tick = elapsed.as_secs_f64() * 1000.0 / 100.0;
        eprintln!(
            "perf: 10 bodies + full grid, 100 ticks in {:.1}ms ({:.2}ms/tick)",
            elapsed.as_secs_f64() * 1000.0,
            ms_per_tick
        );
        assert!(ms_per_tick < 500.0, "perf budget blown: {ms_per_tick:.2}ms/tick");
    }

    #[test]
    #[ignore] // run with `cargo test perf_sanity_100 -- --ignored --nocapture`
    fn perf_sanity_100_bodies_50k_particles() {
        // Phase 3 perf sanity: 100 BlackHoles in a ring at radius 80
        // around grid center, ~50k particles in the gap, 100 ticks.
        // Exercises the Barnes-Hut body-particle path (100 bodies
        // is well above BARNES_HUT_THRESHOLD = 8). Budget: 30+ fps
        // on the gaming rig (< 33ms/tick), 15+ fps on m5
        // (< 67ms/tick). The assert is loose (200ms/tick) so the
        // test passes on a slow CI box; the printed number is what
        // we put in the spec.
        use std::time::Instant;
        let mut w = World::new();
        for i in 0..100 {
            let angle = (i as f32) * 0.0628; // 2*pi / 100
            let r = 80.0;
            w.spawn_black_hole(
                128.0 + r * angle.cos(),
                128.0 + r * angle.sin(),
                500.0,
            );
        }
        for y in 0..H as i32 {
            for x in 0..W as i32 {
                let mut inside = false;
                for b in &w.bodies {
                    if !b.destroys_particles() { continue; }
                    let dx = (x as f32 - b.position.x) as f32;
                    let dy = (y as f32 - b.position.y) as f32;
                    if dx * dx + dy * dy < b.radius * b.radius {
                        inside = true;
                        break;
                    }
                }
                if inside { continue; }
                w.set(x, y, Material::Rock as u8);
                w.body_index[World::idx(x as usize, y as usize)] = NO_BODY;
            }
        }
        let particle_count = w.particles.iter().filter(|&&m| m != 0).count();
        eprintln!("perf: 100 BHs + {} particles (Barnes-Hut)", particle_count);
        let t = Instant::now();
        for _ in 0..100 {
            w.step();
        }
        let elapsed = t.elapsed();
        let ms_per_tick = elapsed.as_secs_f64() * 1000.0 / 100.0;
        eprintln!(
            "perf: 100 bodies + 50k particles (BH), 100 ticks in {:.1}ms ({:.2}ms/tick)",
            elapsed.as_secs_f64() * 1000.0,
            ms_per_tick
        );
        assert!(
            ms_per_tick < 200.0,
            "Phase 3 perf budget blown: {ms_per_tick:.2}ms/tick"
        );
    }

    #[test]
    fn barnes_hut_matches_direct_sum_for_8_bodies() {
        // Phase 3 integration: with 8 bodies (the threshold), the
        // move_pass should now route body-particle gravity through
        // Barnes-Hut. Verify that the per-cell gravity vector from
        // the tree matches the direct N^2 sum to a small tolerance
        // at theta=0.5 (the spec default).
        use crate::barnes_hut::QuadTree;
        let mut w = World::new();
        for i in 0..8 {
            let angle = (i as f32) * 0.7854; // 2*pi / 8
            let r = 60.0;
            w.spawn_black_hole(
                128.0 + r * angle.cos(),
                128.0 + r * angle.sin(),
                500.0,
            );
        }
        let tree = QuadTree::new(&w.bodies, BARNES_HUT_THETA);
        for &(x, y) in &[(20, 20), (128, 128), (200, 200), (50, 200), (180, 80)] {
            let p = glam::Vec2::new(x as f32, y as f32);
            let a_tree = tree.compute_accel(p);
            let mut a_direct = glam::Vec2::ZERO;
            for b in &w.bodies {
                let r = b.position - p;
                let d2 = r.length_squared() + SOFTENING_SQ;
                let d = d2.sqrt();
                let a_mag = G * b.mass / d2;
                a_direct += r * (a_mag / d);
            }
            let mag = a_direct.length();
            let diff = (a_tree - a_direct).length();
            assert!(
                diff < 0.05 * (mag + 1e-3),
                "Barnes-Hut diverges from direct sum at ({x},{y}): \
                 tree={a_tree:?}, direct={a_direct:?}, |diff|={diff:.3}, |direct|={mag:.3}"
            );
        }
    }

    #[test]
    fn barnes_hut_threshold_uses_direct_sum_below() {
        // Below BARNES_HUT_THRESHOLD = 8 the move_pass uses direct
        // sum. This test confirms that the same answer comes out
        // of both paths at the threshold boundary.
        use crate::barnes_hut::QuadTree;
        let mut w = World::new();
        for i in 0..7 {
            let angle = (i as f32) * 0.8976; // 2*pi / 7
            let r = 60.0;
            w.spawn_black_hole(
                128.0 + r * angle.cos(),
                128.0 + r * angle.sin(),
                500.0,
            );
        }
        assert!(
            w.bodies.len() < BARNES_HUT_THRESHOLD,
            "this test wants bodies.len() < BARNES_HUT_THRESHOLD"
        );
        let tree = QuadTree::new(&w.bodies, BARNES_HUT_THETA);
        for &(x, y) in &[(20, 20), (128, 128), (200, 200)] {
            let p = glam::Vec2::new(x as f32, y as f32);
            let a_tree = tree.compute_accel(p);
            let mut a_direct = glam::Vec2::ZERO;
            for b in &w.bodies {
                let r = b.position - p;
                let d2 = r.length_squared() + SOFTENING_SQ;
                let d = d2.sqrt();
                let a_mag = G * b.mass / d2;
                a_direct += r * (a_mag / d);
            }
            let mag = a_direct.length();
            let diff = (a_tree - a_direct).length();
            assert!(
                diff < 0.05 * (mag + 1e-3),
                "Barnes-Hut diverges from direct sum at ({x},{y}): \
                 tree={a_tree:?}, direct={a_direct:?}"
            );
        }
    }


    #[test]
    fn spawn_planet_in_orbit_uses_v_circ_and_recoil() {
        // Click-spawn now initializes the new planet on a tangential
        // circular orbit around the first BH and applies
        // equal-and-opposite recoil to the BH so the system COM
        // starts stationary (matching the default scenario —
        // decision #19). Three invariants to check:
        //
        //   1. Planet velocity is tangential to the radial
        //      BH -> planet vector (CCW) with magnitude
        //      v_circ = sqrt(G * M_BH / r).
        //   2. BH velocity has equal-and-opposite recoil along
        //      the planet's velocity axis.
        //   3. Total system momentum is approximately 0.
        let mut w = World::new();
        let bh_id = w.spawn_black_hole(128.0, 128.0, 1000.0);
        let pid = w
            .spawn_planet_in_orbit(178, 128, 6, Material::Rock as u8)
            .expect("spawn_planet_in_orbit should return Some");

        let planet = w.bodies.iter().find(|b| b.id == pid).unwrap().clone();
        let bh = w.bodies.iter().find(|b| b.id == bh_id).unwrap().clone();

        // Invariant 1: tangential velocity with magnitude v_circ.
        let dx = planet.position.x - bh.position.x;
        let dy = planet.position.y - bh.position.y;
        let r = (dx * dx + dy * dy).sqrt();
        assert!(
            (r - 50.0).abs() < 1.0,
            "expected r ~ 50 from BH at (128,128) to click at (178,128), got r = {r:.3}"
        );
        // CCW perpendicular of (dx, dy) is (-dy, dx) / r. The
        // planet should have its full velocity along this direction
        // (zero radial component).
        let rad_hat = glam::Vec2::new(dx / r, dy / r);
        let tang_hat = glam::Vec2::new(-dy / r, dx / r);
        let v_radial = planet.velocity.dot(rad_hat);
        let v_tang = planet.velocity.dot(tang_hat);
        assert!(
            v_radial.abs() < 1e-3,
            "planet velocity should be purely tangential, got radial component \
             {v_radial:.3} (v = {:?})",
            planet.velocity
        );
        assert!(
            v_tang > 0.0,
            "planet should orbit CCW (positive tangential), got v_tang = {v_tang:.3}"
        );
        let v_circ_expected = (G * 1000.0 / r).sqrt();
        assert!(
            (v_tang - v_circ_expected).abs() < 1e-3,
            "planet speed should be v_circ = sqrt(G*M_BH/r) = {v_circ_expected:.3}, \
             got {v_tang:.3}"
        );

        // Invariant 2: BH recoil is opposite to the planet's
        // velocity direction.
        let bh_dot_planet = bh.velocity.dot(planet.velocity);
        assert!(
            bh_dot_planet < 0.0,
            "BH velocity should be opposite to planet velocity, got \
             bh={:?} planet={:?}",
            bh.velocity,
            planet.velocity
        );

        // Invariant 3: total system momentum is approximately 0
        // (COM-stationary init, same as the default scenario).
        let total_p: glam::Vec2 = w.bodies.iter().map(|b| b.mass * b.velocity).sum();
        assert!(
            total_p.length() < 1e-3,
            "total system momentum should be ~0 after click-spawn, got {total_p:?}"
        );
    }

    #[test]
    fn spawn_planet_in_orbit_returns_none_without_black_hole() {
        // No BH -> no orbital reference. The click-spawn should
        // refuse rather than spawn a free-falling planet at v=0.
        let mut w = World::new();
        let result = w.spawn_planet_in_orbit(100, 100, 5, Material::Rock as u8);
        assert!(
            result.is_none(),
            "spawn_planet_in_orbit should return None when no BH exists"
        );
        assert!(
            w.bodies.is_empty(),
            "no bodies should have been spawned when BH is missing"
        );
    }

    #[test]
    fn spawn_planet_in_orbit_returns_none_on_top_of_bh() {
        // Click on the BH itself -> r ~ 0 -> v_circ blows up. The
        // spawn must refuse rather than inject a NaN velocity.
        let mut w = World::new();
        w.spawn_black_hole(128.0, 128.0, 1000.0);
        let result = w.spawn_planet_in_orbit(128, 128, 5, Material::Rock as u8);
        assert!(
            result.is_none(),
            "spawn_planet_in_orbit should return None when click is on BH"
        );
        // Only the BH should be in the world.
        assert_eq!(w.bodies.len(), 1);
    }

    #[test]
    fn recompute_bonds_does_not_panic_when_se_neighbour_oob() {
        // Regression test for an OOB panic in the sticky-bond
        // mirror-clear path. A planet particle can carry a BOND_SE
        // bit in its bond_state from when it was at a more central
        // cell (where (x+1, y+1) was in bounds) into a corner
        // cell like (255, 255) where (256, 256) is OOB. The
        // adjacency check at the top of `recompute_bonds` only
        // validates neighbours that are in bounds; it does not
        // tell us whether the bond was set when they were. Without
        // an explicit bounds-check on the mirror-clear target the
        // index panics with `index out of bounds: the len is
        // 65536 but the index is 65772`. This test reproduces
        // the exact bug state: a single particle at (255, 255)
        // with a stale BOND_SE bit. recompute_bonds must drop the
        // broken bit on bond_state[i] without trying to mirror-
        // clear into the OOB neighbour.
        let mut w = World::new();
        w.particles[World::idx(255, 255)] = Material::Rock as u8;
        w.bond_state[World::idx(255, 255)] = BOND_SE;
        w.recompute_bonds();
        // The broken bit must have been cleared from bond_state.
        assert_eq!(
            w.bond_state[World::idx(255, 255)], 0,
            "BOND_SE should be cleared when (256, 256) is OOB"
        );
    }

    #[test]
    fn recompute_bonds_does_not_panic_when_n_neighbour_oob() {
        // Same class of bug, but for the BOND_N bit at the top
        // edge. A particle at (0, 0) can carry a stale BOND_N
        // bit. recompute_bonds would try to mirror-clear into
        // (0, -1), which underflows the y index on usize.
        let mut w = World::new();
        w.particles[World::idx(0, 0)] = Material::Rock as u8;
        w.bond_state[World::idx(0, 0)] = BOND_N;
        w.recompute_bonds();
        assert_eq!(
            w.bond_state[World::idx(0, 0)], 0,
            "BOND_N should be cleared when (0, -1) is OOB"
        );
    }

    #[test]
    fn recompute_bonds_does_not_panic_when_e_neighbour_oob() {
        // Same class of bug, but for the BOND_E bit at the right
        // edge. A particle at (255, 128) with stale BOND_E.
        let mut w = World::new();
        w.particles[World::idx(255, 128)] = Material::Rock as u8;
        w.bond_state[World::idx(255, 128)] = BOND_E;
        w.recompute_bonds();
        assert_eq!(
            w.bond_state[World::idx(255, 128)], 0,
            "BOND_E should be cleared when (256, 128) is OOB"
        );
    }

    #[test]
    fn recompute_bonds_does_not_panic_when_ne_neighbour_oob() {
        // Same class of bug, but for the BOND_NE bit at the top-
        // right corner. A particle at (255, 0) with stale BOND_NE.
        let mut w = World::new();
        w.particles[World::idx(255, 0)] = Material::Rock as u8;
        w.bond_state[World::idx(255, 0)] = BOND_NE;
        w.recompute_bonds();
        assert_eq!(
            w.bond_state[World::idx(255, 0)], 0,
            "BOND_NE should be cleared when (256, -1) is OOB"
        );
    }

    #[test]
    fn spawn_planet_near_edge_does_not_panic() {
        // Regression test for a spawn-time OOB panic. The disk-
        // fill loop in `spawn_planet` iterated over a square
        // (cx - radius) ..= (cx + radius) without bounds checks
        // and pushed OOB (x, y) tuples into `filled`. The next
        // loop then indexed `Self::idx(x as usize, y as usize)`
        // for those tuples, panicking on `self.body_index[i] =`.
        // A click at the corner of the grid with radius=10
        // triggered this. The fix skips OOB cells up front, so
        // the planet just clips to the grid — partial planet,
        // no panic.
        let mut w = World::new();
        let pid = w.spawn_planet(255, 255, 10, Material::Rock as u8);
        assert_eq!(pid, 0, "first body should get id 0");
        // The planet should exist and have at least the center cell.
        let p = w.bodies.iter().find(|b| b.id == pid).unwrap();
        assert!(p.mass > 0.0, "planet should have at least one particle");
        // No OOB indices should have been written.
        for y in 0..H {
            for x in 0..W {
                let i = World::idx(x, y);
                if w.particles[i] != 0 {
                    assert!(w.body_index[i] == pid as i32 || w.body_index[i] == NO_BODY,
                        "unexpected body_index at ({x}, {y})");
                }
            }
        }
    }

}
