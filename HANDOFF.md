# Handoff Log

Session-to-session continuity. New entries on top. Keep entries short (4-5 bullets max).

---

## Template

```markdown
### YYYY-MM-DD — Session N: <one-line summary>

- <what shipped>
- <what's still broken / next up>
- <decisions made>
- <playtest status — what did the user actually run?>
```

---

## Log


### 2026-10-09 — Session 7: Phase 3 — Barnes-Hut body-particle gravity

- New module `src/barnes_hut.rs` (flat `Vec<QuadNode>` quadtree,
  `QuadTree::new(bodies, theta)` build, `compute_accel(p)` walk
  with s/d<theta criterion). Integrated into `move_pass` via a
  new `gravity_step_for_cell_bh`; the path activates when
  `bodies.len() >= 8`. Body-body gravity stays N² pairwise.
- Decisions #23–#27 added (theta=0.5 default, threshold=8, rebuild
  every tick, body-body N² stays, Plummer softening in tree).
- 36 tests pass (was 29, +7 — 5 in barnes_hut, 2 in sim);
  3 ignored. Perf: 10 BHs + 65k particles at 7.4 ms/tick; 100
  BHs + 63k particles at 15.2 ms/tick — both well under the
  33 ms/tick budget for 30+ fps.
- **Playtest status:** default-scenario dump (the visual smoke
  test) is byte-for-byte identical to the Phase 2 baseline
  (the default scenario has 2 bodies, so Barnes-Hut is not
  exercised). Barnes-Hut-vs-N² equivalence verified at 5%
  relative error for 8 bodies; the 8+ body regime is the
  Phase 3 trigger from the spec.
- **Next up:** open PR for review; visual playtest on real
  hardware to confirm the multi-body "tidal peel" reads
  correctly; Phase 4 (bodies as first-class objects with
  shape templates) is now the next major chunk of work.

### 2026-10-08 — Session 4: Phase 2 hardening (BH-zoom playtest fix)

- Three real bugs were behind the MacBook "BH zooms around" report
  from the default-scenario playtest: (1) `move_pass` silently lost
  particles when two converged on the same cell (up to 84 of 113 in
  the default scenario by t=200) because `find_target` only checked
  the *original* grid, not the in-progress `claim` array; (2) a
  Frankenstein integrator (Forward Euler on velocity, Verlet on
  position) was gaining energy and spiraling orbits outward; (3) the
  default scenario's near-collision orbit was throwing the planet
  off the grid where its pinned mass dragged the BH around. All
  three fixed: pass `claim` into `find_target`, switch to a proper
  KDK leapfrog, and replace the default with a circular orbit at
  r=50 with COM-stationary initial velocities.
- New tests: `move_pass_does_not_lose_particles`,
  `leapfrog_does_not_gain_energy_in_pure_orbit`,
  `body_body_gravity_conserves_momentum`. 27 pass, 2 ignored.
- **Playtest status:** T3 Code verified the BH stays near grid
  center and the planet completes a circular orbit; the user
  confirmed the playtest on the m5. Perf sanity (10 BHs + 65k
  particles) is 3.3 ms/tick — within budget.
- **Next up:** open PR #4 for review; pick up the remaining
  Newton's-3rd-law event-horizon hole in a follow-up.

### 2026-10-09 — Session 5: Phase 2 — Newton's 3rd law across the event horizon

- Closed the last Phase 2 hole flagged in Session 4: particles
  consumed at the BH's event horizon used to vanish with their
  momentum, leaving the BH to retain its pre-consumption velocity
  and breaking conservation of total system momentum. The COM-
  stationary default init hid the symptom; head-on / off-COM
  configs would have shown the BH lurching.
- `apply_event_horizons` now applies `Δv_BH = v_particle / M_BH`
  per consumed particle (v_particle ≈ owning planet body velocity).
  Recoil is accumulated per BH in a small `Vec` and applied at the
  end of the destruction loop to avoid borrow conflicts with the
  per-planet `particle_budget` decrement. Decision #22 added.
- New test `event_horizon_conserves_total_momentum` runs the
  COM-stationary default to consumption: total system momentum
  stays ≤ 5 mass-units*cells/tick end-to-end, and the BH ends up
  with |v| ≤ 0.1 cells/tick (its initial recoil has been cancelled
  by the absorbed planet momentum). 28 pass, 2 ignored.
- **Playtest status:** dump at t=400 shows BH velocity dropped
  from initial (0, -0.045) to (0.005, 0.017) — nearly stationary
  at the COM, as the math predicts. Perf sanity 3.41 ms/tick.

### 2026-10-08 — Session 3: Phase 2 ships

- Phase 2 build complete on branch `feat/phase-2-multi-body-gravity`
  (12 commits: docs → Body types → sticky bonds → body-particle gravity
  + event horizon → CLI scenario + default → visual ring + orbital
  tests → 2-BH tearing → perf sanity → docs).
- New modules: `src/body.rs` (Body, BodyKind, G, Plummer ε, event
  horizon), `src/scenario.rs` (`--bodies "..."` parser + default).
- Engine: 24 tests passing (8 sim, 7 phase-2 sim, 3 body, 9 scenario).
  Engine still compiles, binary boots cleanly in Xvfb.
- **Playtest status:** visual smoke test on the default scenario (1 BH
  at center + 1 planet on near-collision orbit) shows the planet
  orbiting and being stripped of 11/113 particles over 400 ticks
  (~6.7s @ 60fps). Perf measurement: 10 BHs + 65k particles runs at
  ~3.2 ms/tick in release → ~310 fps. Well above the perf budget
  from `docs/performance-budget.md`. Real m5 / gaming-rig playtest
  pending — this sandbox has no working Vulkan and no display.
- **Next up:** open the PR (`feat/phase-2-multi-body-gravity →
  main`), get user review, do the real-window playtest on m5 (or the
  user's gaming rig), and start Phase 3 (Barnes-Hut octree for
  larger-N scenarios) or Phase 4 (bodies as first-class objects).

### 2026-10-08 — Session 2: Phase 1 ships

- Shipped the Phase 1 sim on branch `phase-1`: wgpu window + falling-sand grid + single black hole at center + click-to-spawn planet + per-particle bond recompute.
- New modules: `src/material.rs` (Rock/Ice/Plasma/Vacuum), `src/sim.rs` (SoA grid, gravity pull with 1/2/3-step gradient for tidal effect, falling-sand fallbacks, bond recompute), `src/state.rs` (wgpu surface/pipeline + per-frame RGBA8 upload), `src/app.rs` (winit ApplicationHandler with left-click spawn / right-click clear / resize).
- Grid is `256×256` behind `pub const W: usize = 256;` in `sim.rs` — flip to 512 in one place when the m5 perf budget allows.
- 4 sim tests pass (empty world, falling sand, planet spawn, planet migrates toward hole); 2 `#[ignore]`'d visual-dump tests for screenshot debugging.
- **Playtest status:** visual sequence dumped from the sim (t=0/20/40) shows the planet being stretched into a long thin stream of isolated particles as the near-side gets pulled faster than the far-side — exactly the "evaporate particle by particle" signature from the spec. Real-window playtest pending on the user's m5 (this sandbox has no working DRI3 Vulkan and no display); the binary boots cleanly in Xvfb + lavapipe.
- **Next up:** Phase 2 (multi-body gravity), or polish on Phase 1 first if the real-window playtest surfaces issues (render-pipeline-bound? the spec calls out `docs/render-pipeline.md` as a possible early PR).
### 2026-10-08 — Session 1: bootstrap & spec

- Created repo at `~/Development/blackhole-sand/`, GitHub remote `klampatech/blackhole-sand`, init commit pushed.
- Wrote `docs/SPEC.md` (canonical), `SPEC.md` (stub pointer), this file, CI guard, README, .gitignore.
- Locked design: 2D, hand-rolled wgpu + Rust sim, per-particle bonds for structural integrity.
- **Playtest status:** nothing playable yet. Repo is bootstrapped, CI is green, next session starts Phase 1 (window + single black hole + falling sand + first bond break).
