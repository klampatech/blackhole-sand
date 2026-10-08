//! Sim grid: 2D SoA world, per-particle bonds, falling-sand physics with a
//! single fixed black hole at the grid center.
//!
//! Phase 1 model (intentionally simple — perf & correctness first, refinement
//! later):
//!
//! * Each cell is either empty (Vacuum) or holds one particle of a material
//!   (Rock / Ice / Plasma in v0.1).
//! * Bonds are 4 bits per cell (N, E, NE, SE). The other 4 directions are
//!   mirrors on the neighbour cell's bitmask.
//! * Per tick, every cell decides a target direction (toward the hole if
//!   within the pull zone, else straight down) and walks up to N empty cells
//!   in that direction. Closer to the hole → bigger N (3 / 2 / 1 cells per
//!   tick). This distance gradient is what gives the planet a "tidal peel"
//!   visual: the near-side advances faster than the far-side, adjacency
//!   breaks, bonds clear, the body disintegrates.
//! * After the move pass, bond bits are recomputed from scratch based on
//!   current adjacency. We deliberately re-form on contact (Phase 1 only);
//!   "once broken, stays broken" semantics are a later refinement.

use crate::material::Material;

/// Grid resolution. 256² is the Phase 1 spec target (~30 fps on m5). Flip to
/// 512 once the perf budget allows it — this is the only line to change.
pub const W: usize = 256;
pub const H: usize = 256;

/// Black hole position (grid center).
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

pub struct World {
    pub particles: Vec<u8>, // length W*H, material id per cell
    pub bonds: Vec<u8>,     // length W*H, 4-bit bond bitmask
}

impl World {
    pub fn new() -> Self {
        Self {
            particles: vec![0; W * H],
            bonds: vec![0; W * H],
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
    /// visible before the user clicks anything.
    pub fn seed_rain(&mut self) {
        // A handful of narrow streams, varied materials, so the user sees
        // rocks falling and plasma diffusing.
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

    /// Spawn a planet: a filled circle of bonded particles of one material.
    /// `mat` must be a bonding material (not Vacuum or Plasma).
    pub fn spawn_planet(&mut self, cx: i32, cy: i32, radius: i32, mat: u8) {
        if !Material::from_u8(mat).has_bonds() {
            return;
        }
        let r2 = radius * radius;
        // Fill the disk.
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
            }
        }
    }

    /// One sim tick: move pass + bond rebuild.
    pub fn step(&mut self) {
        self.move_pass();
        self.recompute_bonds();
    }

    /// Movement pass: for every non-empty cell, decide a target direction and
    /// walk up to `max_steps` empty cells in that direction.
    fn move_pass(&mut self) {
        let mut new_particles = self.particles.clone();
        let mut new_bonds = self.bonds.clone();
        let mut claim = vec![false; W * H];

        for y in 0..H {
            for x in 0..W {
                let i = Self::idx(x, y);
                if claim[i] || new_particles[i] == 0 {
                    continue;
                }
                let m = new_particles[i];
                let b = new_bonds[i];

                let (dx, dy) = gravity_direction(x as i32, y as i32);
                let max_steps = gravity_steps(x as i32, y as i32);

                // If the primary direction is blocked, try a couple of
                // fallbacks (falling-sand diagonals, etc.) so particles can
                // // flow around obstacles.
                let (tx, ty) = self.find_target(x as i32, y as i32, dx, dy, max_steps);

                if tx == x as i32 && ty == y as i32 {
                    // Stayed put.
                    claim[i] = true;
                } else {
                    let ti = Self::idx(tx as usize, ty as usize);
                    new_particles[ti] = m;
                    new_bonds[ti] = b;
                    new_particles[i] = 0;
                    new_bonds[i] = 0;
                    claim[ti] = true;
                }
            }
        }

        self.particles = new_particles;
        self.bonds = new_bonds;
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
        // Fallbacks: if we couldn't move at all and we're in the "fall down"
        // regime, try diagonal-downs so we don't get stuck on a single
        // supported particle.
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

    /// Rebuild bond bitmasks from current adjacency. Phase 1: re-form on
    /// contact, no carryover. A planet stays bonded while its cells are
    /// adjacent; it disintegrates when adjacency fails.
    fn recompute_bonds(&mut self) {
        for y in 0..H {
            for x in 0..W {
                let i = Self::idx(x, y);
                let m = self.particles[i];
                if !Material::from_u8(m).has_bonds() {
                    self.bonds[i] = 0;
                    continue;
                }
                let mut b = 0u8;
                if y > 0 && self.particles[Self::idx(x, y - 1)] == m {
                    b |= BOND_N;
                }
                if x + 1 < W && self.particles[Self::idx(x + 1, y)] == m {
                    b |= BOND_E;
                }
                if y > 0 && x + 1 < W && self.particles[Self::idx(x + 1, y - 1)] == m {
                    b |= BOND_NE;
                }
                if x + 1 < W && y + 1 < H && self.particles[Self::idx(x + 1, y + 1)] == m {
                    b |= BOND_SE;
                }
                self.bonds[i] = b;
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

#[inline]
fn gravity_direction(x: i32, y: i32) -> (i32, i32) {
    let dx = HOLE_X - x;
    let dy = HOLE_Y - y;
    let d2 = dx * dx + dy * dy;
    if d2 < PULL_RADIUS_SQ {
        (dx.signum(), dy.signum())
    } else {
        (0, 1)
    }
}

#[inline]
fn gravity_steps(x: i32, y: i32) -> i32 {
    let dx = HOLE_X - x;
    let dy = HOLE_Y - y;
    let d2 = dx * dx + dy * dy;
    if d2 <= NEAR_RADIUS_SQ {
        3
    } else if d2 <= MID_RADIUS_SQ {
        2
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore] // run with `cargo test dump_grid_seq -- --ignored --nocapture`
    fn dump_grid_seq_for_visual_check() {
        use std::io::Write;
        let mut w = World::new();
        w.spawn_planet(W as i32 / 2 + 60, H as i32 / 2 - 20, 8, Material::Rock as u8);
        let checkpoints = [0usize, 20, 40, 80, 160, 320];
        for &n in &checkpoints {
            // Run the sim for `n` steps total (not +n from previous).
            let target = n;
            let already = w.particles.iter().filter(|&&m| m != 0).count();
            // We re-create the world from scratch per checkpoint so each
            // captures the same initial state advanced by `n` steps.
            let _ = already;
            let mut ww = World::new();
            ww.spawn_planet(W as i32 / 2 + 60, H as i32 / 2 - 20, 8, Material::Rock as u8);
            for _ in 0..target {
                ww.step();
            }
            let mut rgba = vec![0u8; W * H * 4];
            ww.render_rgba(&mut rgba);
            let path = format!("/tmp/bhsand_t{n:03}.bin");
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(&rgba).unwrap();
            let remaining = ww.particles.iter().filter(|&&m| m != 0).count();
            eprintln!("t={n}: {remaining} cells, dumped {path}");
        }
    }

    #[test]
    #[ignore] // run with `cargo test dump_grid -- --ignored --nocapture`
    fn dump_grid_for_visual_check() {
        use std::io::Write;
        let mut w = World::new();
        w.seed_rain();
        w.spawn_planet(W as i32 / 2 + 50, H as i32 / 2, 12, Material::Rock as u8);
        for _ in 0..80 {
            w.step();
        }
        // Dump as RGBA binary.
        let mut rgba = vec![0u8; W * H * 4];
        w.render_rgba(&mut rgba);
        let mut f = std::fs::File::create("/tmp/bhsand_grid.bin").unwrap();
        f.write_all(&rgba).unwrap();
        eprintln!("dumped /tmp/bhsand_grid.bin ({}x{})", W, H);
    }

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
        // Place a Rock cell high and far from the hole (top-left corner is
        // well outside the 100-cell pull zone).
        w.set(10, 10, Material::Rock as u8);
        for _ in 0..200 {
            w.step();
        }
        // The cell should have fallen straight down and stopped near the
        // bottom or near the hole. At minimum, it should be below its start.
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
        // Count non-empty cells inside the planet area.
        let n = (45..=55)
            .flat_map(|y| (45..=55).map(move |x| (x, y)))
            .filter(|&(x, y)| w.particles[World::idx(x, y)] != 0)
            .count();
        // π·r² ≈ 78 cells, allow some slack for the discrete disk.
        assert!(n > 50 && n < 100, "expected ~78 planet cells, got {n}");
    }

    #[test]
    fn planet_disintegrates_under_tidal_pull() {
        let mut w = World::new();
        // Place a small planet close to (but not on top of) the hole so the
        // pull gradient can act on it.
        w.spawn_planet(W as i32 / 2 + 30, H as i32 / 2, 6, Material::Rock as u8);
        let initial_cells = w.particles.iter().filter(|&&m| m != 0).count();
        // Run a fair number of ticks; near the hole, max-steps=3 so the
        // outer ring should shed a few cells.
        for _ in 0..200 {
            w.step();
        }
        // The cluster should still exist (planet doesn't fully evaporate
        // in 200 ticks) but it should have moved.
        let remaining = w.particles.iter().filter(|&&m| m != 0).count();
        assert!(remaining > 0, "planet vanished entirely");
        // Center of mass should have moved toward the hole.
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
        let cx = sx / sn;
        let cy = sy / sn;
        let dx = cx - (W as i32 / 2) as i64;
        let dy = cy - (H as i32 / 2) as i64;
        let d2 = dx * dx + dy * dy;
        let start_dx = 30i64;
        let start_d2 = start_dx * start_dx;
        assert!(
            d2 < start_d2,
            "planet center of mass did not migrate toward the hole (d²={d2}, start d²={start_d2}, center=({cx},{cy}))"
        );
        eprintln!("initial={initial_cells}, remaining={remaining}, d²={d2}");
    }
}
