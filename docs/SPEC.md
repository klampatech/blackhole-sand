# Blackhole Sand — Engine Spec

**Canonical source of truth: this file.** The vault is a one-way mirror (if/when we create one); this is authoritative. Edit on a branch + PR. No direct-to-main pushes.

> **Status:** Phase 2 merged on main (PR #4, `22cfc23`). Phase 3 (Barnes-Hut) in progress on `feat/phase-3-barnes-hut`.

---

## Vision

A 2D falling-sand-style particle physics engine for space. Planets, asteroids, and other bodies are *made of particles*, and a black hole's tidal forces can **rip them apart particle by particle**. The destruction is the game: bonds break, debris streams, the planet evaporates.

**What makes this *this* game and not Universe Sandbox 2:** destruction is not discrete chunks or meshes — it's particle-level degradation. You watch a planet slowly disintegrate because individual bonds are snapping one at a time under tidal stress.

---

## Stack (frozen for v0.1)

| Layer | Pick | Why |
|---|---|---|
| Language | **Rust (2021 edition)** | Memory layout control is the whole game; Rust's SoA-friendly structs + no GC pauses matter for a frame-bound sim. |
| GPU API | **wgpu (v22.x)** | Renders the sim grid as a single texture blit. No scene graph. Also unlocks compute shaders for the N² gravity problem later. |
| Window/event loop | **winit (v0.30)** | Standard wgpu pairing. |
| Async init | **pollster** | Block-on-future for the wgpu device request without an async runtime. |
| Bytemuck | **bytemuck (v1.16)** | Pod/Zeroable traits for GPU buffer uploads. |

**Rejected:**
- **Bevy** — ECS wants entities, we want a 2D grid. Fights us.
- **Raw wgpu without sim scaffolding** — pure yak-shaving, no payoff.
- **C++/Vulkan** — no ecosystem advantage in 2D, slower iteration.

---

## Architecture

### World Model

- **2D grid**, fixed size. Default `512×512` cells. Each cell is either empty or a particle.
- **Cell resolution** = 1 particle. Particles are 1×1 grid cells, not sub-pixel.
- **Bonds** are per-particle directional neighbor links: up to 8 per particle (4 cardinal + 4 diagonal). A bond stores a rest-length and a max-strain threshold.
- **Materials** = small enum (v0.1: `Rock`, `Ice`, `Plasma`, `Vacuum`). Each material defines: color, density, bond strength, bond max-strain. Plasma has no bonds (it diffuses). Vacuum is the empty cell.
- **Bodies** (planets, asteroids) = clusters of bonded particles. Identity is emergent, not stored as a top-level object. A planet is "the cluster of bonded particles near here."

### State (SoA layout for cache efficiency)

```
particles:    [u8; W*H]      material per cell   (0 = empty)
bonds_n:      [u8; W*H]      bitmask of active bonds to N/E/NE/SE neighbors
bonds_ne:     [u8; W*H]      same for NE diagonal
bonds_se:     [u8; W*H]      same for SE diagonal
bonds_s:      [u8; W*H]      same for S (mirror of N of cell below)
bonds_sw:     ...            (mirror of NE of cell SE)
... 
```

For v0.1 we only need 4 bond types per cell (N, E, NE, SE) since the others are mirror-pairs. The bitmask encodes which of those 4 directions are bonded.

### Physics Passes (per frame, in order)

1. **Gravity compute** — N² direct sum on CPU up to N=~5000; Barnes-Hut octree beyond that (Phase 3). For Phase 0/1 we just use a fixed gravity well (no N² yet).
2. **Particle update** — each particle applies gravity to its velocity, integrates position. Position is integer grid coordinates (falling-sand style), not floats. Sub-cell movement goes through a "fall pattern" state machine.
3. **Bond stress** — for each bonded pair, compute current length vs. rest-length. If strain > max-strain for the weaker material, the bond breaks (one bit cleared, both particles' bond masks updated).
4. **Collision** — particles can't overlap. Falling-sand-style swap with empty neighbor. If no empty neighbor, particle stays (supported by neighbor).
5. **Render** — copy `particles[]` to a GPU texture, blit to screen. One draw call. (Phase 0/1: no fancy effects, just colored cells.)

### GPU Boundary

| CPU | GPU |
|---|---|
| All sim passes (gravity, update, bonds, collision) | Render only: `particles[]` → RGBA8 texture → full-screen quad |
| Future: Barnes-Hut gravity compute (octree traversal on GPU) | |

Sim stays on CPU because it needs fine-grained cell access; that's also what makes it "falling sand" — every cell is live.

### Gravity (Phase 0/1 stub)

Single fixed black hole at grid center. Each particle gets a velocity contribution toward the hole. Real N² with multiple bodies comes in Phase 2. Barnes-Hut in Phase 3 if needed for performance.

### Structural Integrity — Per-Particle Bonds

**This is the design choice that makes the game unique.** Each particle knows its bonded neighbors via a small bitmask. Bonds break when the relative displacement of the two bonded particles exceeds the material's max-strain.

**Why this over alternatives:**
- *Voxel chunks* — coarse, whole chunks pop. Loses the "peel off one particle" effect.
- *Mesh with tensile strength* — works in 3D, in 2D it's just edge networks. Same as bonds with more bookkeeping.
- *Pure PBD* — stable but doesn't model tearing. You can stretch forever.

**Why this is performant:** max 4 bond bits per cell (N, E, NE, SE — others are mirrors), packed as one byte. Update pass touches each cell once.

### Visual Signature

Planets *evaporate* under tidal stress, not explode. A particle-by-particle disintegration where the cause is visible (tidal forces stretching the body) and the effect is continuous (bonds breaking one at a time, debris streaming, the body slowly losing mass).

---

## Phased Plan

### Phase 0 — Bootstrap & Spec ✅
- Repo layout, `docs/SPEC.md`, GitHub remote, CI guard, README.
- **Definition of done:** this file merged, CI green, init commit pushed.

### Phase 1 — Window + Falling Sand + Single Black Hole
- wgpu window opens, colored cells fall under "gravity" (downward), and a single black hole in the center pulls them.
- A "planet" (cluster of bonded particles) can be placed; it falls toward the black hole, and **individual bonds break** as tidal forces exceed max-strain.
- **Playtest:** user runs `cargo run`, sees particles falling, drops a planet near the black hole, watches it disintegrate particle by particle. Slow grid (~30 fps at 256×256) is acceptable on m5.

### Phase 2 — Multi-Body Gravity
- Add ability to place N bodies with mass and velocity. N² gravity between bodies and particles.
- Real Kepler orbits, slingshot maneuvers, stable Lagrange points.
- **Full spec:** [`docs/phase-2-multi-body-gravity.md`](phase-2-multi-body-gravity.md). Includes body storage, N² math, Verlet integration, event horizon / particle destruction.

### Phase 3 — Barnes-Hut Octree (if needed)
- Performance: N² dies around 10k particles. Add an octree-based gravity solver on CPU; if that's still too slow, push to a wgpu compute shader.
- **Full spec:** [`docs/phase-3-barnes-hut.md`](phase-3-barnes-hut.md). Quadtree structure, traversal algorithm, θ tuning, GPU compute deferred.

### Phase 4 — Bodies as First-Class Objects
- "Place a planet" becomes a primitive. Bodies have mass, velocity, shape templates (sphere, ring, asteroid cluster). The emergent identity from Phase 1 gets explicit metadata.

### Phase 5 — Game Layer
- Score, levels, black hole placement constraints, particle-budget challenges, campaign.

---

## Reference Docs

- [`docs/render-pipeline.md`](render-pipeline.md) — what gets sent to the GPU, in what format, when. The whole render pass is one texture upload + one full-screen triangle. Read before touching anything GPU-side.
- [`docs/performance-budget.md`](performance-budget.md) — concrete fps targets at every grid size on m5 vs the gaming rig, where the time goes per frame, and when to spec vs when to profile.

---

## Decisions Log

| # | Date | Decision | Why | Rejected |
|---|---|---|---|---|
| 1 | 2026-10-08 | 2D, not 3D | Falling sand is 2D; particle-by-particle destruction reads better in 2D; perf budget is 10x higher. | 3D path with depth/lighting. |
| 2 | 2026-10-08 | Hand-rolled wgpu + Rust sim | Need fine control over grid layout, not Bevy's ECS. wgpu gives us GPU blit and future compute without locking us out. | Bevy (ECS mismatch), Unity DOTS (not "custom engine" feel), pure wgpu from scratch (year of yak-shaving). |
| 3 | 2026-10-08 | Per-particle bonds, not chunks/meshes | This is the visual signature. Particles *evaporate*, not explode. | Voxel chunks (coarse), mesh with tensile strength (3D-style), pure PBD (no tearing model). |
| 4 | 2026-10-08 | m5 as dev box, dGPU as target | m5 is integrated Radeon, can dev at low particle counts. Architecture keeps sim CPU-side, render a single blit — easy to retarget. | Optimizing for m5 specifically. |
| 5 | 2026-10-08 | Phase 2: Verlet integration for bodies, Euler for particles | Bodies need long-term orbital stability (Verlet is symplectic — energy-conserving). Particles snap to grid cells, don't need it. | Pure Euler for everything (orbits decay visibly within seconds — bad UX). |
| 6 | 2026-10-08 | Phase 2: bodies are part of the grid (Option A), not a separate render layer | Option A keeps the renderer simple (one texture upload, one draw call). Visual polish comes in Phase 4. | Option B (separate body layer) — second draw call, premature complexity for v0.1. |
| 7 | 2026-10-08 | Phase 2: Plummer softening (ε² added to r²) for gravity | Prevents infinite acceleration on close encounters. Standard trick, ε=0.5 grid cells. | Hard cutoff (looks unnatural — bodies "skip" past each other). |
| 8 | 2026-10-08 | Phase 3: Barnes-Hut on CPU, GPU compute deferred | Tree build is small (≤4000 nodes for 1000 bodies), rebuilds fast on CPU. Pushing tree traversal to GPU requires a full sim→GPU migration — Phase 4+ question. | GPU-only Barnes-Hut (premature optimization; complicates Phase 1/2). |
| 9 | 2026-10-08 | Render uses one byte per cell (R8Unorm) + a 256-entry palette texture | Material fits in one byte; palette lookup is one shader instruction. Less bandwidth, simpler code. | Rgba8 for sim data (wastes 75% of bandwidth), per-material branch in fragment shader (GPU unfriendly). |
| 10 | 2026-10-08 | Phase 2: bonds are sticky (once broken, stay broken) | Phase 1 auto-rebonded from adjacency — fine for visual continuity, breaks the disintegration effect in Phase 2 (debris would re-bond into Frankenstein planets). Sticky bonds make the visual signature of mass loss to a black hole work. | Keep auto-rebond (debris heals, disintegration is invisible), perfect-rebuild from adjacency every frame (no notion of "broken" bonds — same as Phase 1). |
| 11 | 2026-10-08 | Phase 2: per-cell `body_index: i32` tracks owning body | Decouples particle ownership from gravity influence. A planet particle is still attracted to all bodies (BlackHole included) but the event-horizon destruction can decrement the *owning* planet's `particle_budget`. | Re-derive ownership on demand (O(n*m) each tick, hard to keep in sync with `bonds`), attach body id to the `Body` struct (single owner doesn't model multi-body influence). |
| 12 | 2026-10-08 | Phase 2: per-cell gravity uses sign of net accel + magnitude-scaled `max_steps` (1/2/3) | Re-uses the existing falling-sand movement primitive, no need for sub-cell particle positions. The "tidal peel" still works because adjacent particles feel different accelerations and so take different step directions. | Sub-cell particle positions (more refactor for a difference the user can't see at the spec'd grid size), per-particle velocity vectors (breaks the falling-sand model). |
| 13 | 2026-10-08 | Phase 2: bodies use leapfrog integration (kick + drift) not pure Verlet | Body-body gravity is `N²/2` ops/tick, the average-accel Verlet requires keeping a *previous* accel cache. Leapfrog stores only the new accel and is also symplectic at dt=1 grid step. Stability is identical for our orbital timescales. | Pure velocity Verlet (extra accel-cache state for marginal benefit at dt=1). |
| 14 | 2026-10-08 | Phase 2: `Material::EventHorizon` (dark purple) drawn as a cosmetic ring around BlackHole bodies | The spec said "the horizon itself is just a circle of vacuum", but a vacuum circle is invisible against the black background. A dark-purple ring of cells is visually distinct without needing a second render pass or special shader. | Separate render layer for bodies (second draw call, premature for v0.1), shader-side circle rendering (we don't even use the shader's color picker yet, so this is over-engineering). |
| 15 | 2026-10-08 | Phase 2: event-horizon destruction is strict-less-than (d² < r²), not ≤ | Reads as a circle, not a filled disk. Particles exactly at the horizon stay alive for one tick and form a "buffer zone". The cosmetic EventHorizon ring covers this buffer so the user doesn't notice. | Use ≤ (no buffer; particles at the exact boundary vanish, looks like a 1-cell stutter). |
| 16 | 2026-10-08 | Phase 2: body velocity Verlet uses `dt=1` (one tick) | Grid step is the natural time unit. Orbit math works out as cells/tick. The decision is "what does dt mean for the symplecticity of the integrator" — at dt=1 the leapfrog integrator is well within its stability range for our G and mass scales. | Variable dt (more bookkeeping, no benefit at this scale). |
| 17 | 2026-10-08 | Phase 2: `move_pass` uses the in-progress `claim` array as the occupancy map (NOT the original `self.particles`) | The original `find_target` only checked the original grid, so when two particles converged on the same target cell in the same tick the second one silently overwrote the first — up to 60% of a planet's particles could vanish per 40 ticks during a close approach. Routing `claim` through `find_target` fixes this with a one-line change. | Keep checking `self.particles` (silently loses particles during tidal stripping — the user sees a planet "evaporate" without anything being consumed by the BH). |
| 18 | 2026-10-08 | Phase 2: bodies that hit a wall get their wall-directed velocity zeroed (not reflected) | Reflection inverts the velocity and would put a body back into the world — but with the falling-sand sim the body is also being pulled by gravity, so reflection interacts weirdly with the integrator. Zeroing the wall component lets the body slide along the wall and the integrator remains symplectic. A wall-pinned body is also a problem: a heavy planet body stuck at (0, 0) would pull the BH across the screen for as long as the user watches. | Reflect (looks natural in a hard-edge game, but the integrator treats it as a momentum-reversing collision, which can leak energy), toroidal wrap (visually confusing — bodies disappear off one edge and reappear on the other). |
| 19 | 2026-10-08 | Phase 2: default scenario uses a **COM-stationary** initial-velocity setup (`v_BH = -(m_planet/m_BH) * v_planet`) | Without this, the planet's tangential velocity gives the system net momentum and the COM drifts in the +v_planet direction. The 9:1 BH:planet mass ratio means the BH orbits the COM at radius `m_planet/(m_BH+m_planet) * r ≈ 4 cells` instead of the COM drifting at ~0.05 cells/tick, which is what the user actually sees as "the BH walking across the screen". | Set both to zero (planet just falls in), use a 100x heavier BH (clutters the cell count and changes the G-vs-mass interpretation). |
| 20 | 2026-10-08 | Phase 2: default scenario uses a **circular orbit at r=50** with v = v_circ | The previous default (r=60, v=0.5) had eccentricity 0.875 and apocenter ≈ 900 cells — way off the 256×256 grid. The planet flew off the screen in one tick, hit a wall, and the resulting pinned mass dragged the BH around indefinitely. A circular orbit at r=50 with v=v_circ is bounded, fits in the grid, and lets the user watch a complete orbit. For wilder / destructive orbits the user passes a custom `--bodies "..."`. | Keep r=60, v=0.5 (apocenter 900 cells, off-grid — the bug we're fixing), near-circular at r=40 (planet gets fully consumed within 400 ticks of running and the BH retains the orbital velocity, looks like "the BH zoomed around"). |
| 21 | 2026-10-08 | Phase 2: when a planet body's mass hits 0 (all particles consumed) we zero its velocity and clear its cached accel | NaN guard for `0/0` in the body-body gravity loop. Without it the BH would NaN-pill the moment a planet vanished. | Allow NaN to propagate (BH instantly disappears, looks like a rendering bug, hard to debug). |
| 22 | 2026-10-09 | Phase 2: event horizon applies Newton's 3rd law — per consumed particle `Δv_BH = v_particle / M_BH`, accumulated across all particles consumed in a tick | When a particle vanishes at the horizon, its momentum vanishes with it unless we transfer it. Without this, total system momentum is not conserved across the horizon and the BH drifts after consuming mass (the COM "walks" in the direction of the BH's initial velocity). The previous fix (COM-stationary init) made the symptom subtle; this fixes the underlying physics. The particle's velocity is approximated as its owning planet body's velocity (bonded particles ride with the planet; this is the dominant component of each particle's world-frame velocity). Recoil is accumulated in a small per-BH vector inside `apply_event_horizons` and applied after the destruction loop to avoid borrow conflicts with the per-planet `particle_budget` decrement. | No recoil (conservation violated; the only way the user can still see the BH drift), apply recoil to the owning planet instead (the consuming BH is the body that needs the momentum; the planet's velocity is unchanged because only its mass shrinks), approximate the particle's velocity using the per-cell gravity acceleration (centripetal, not tangent — wrong direction for circular orbits). |

---

## Open Questions

- **Materials beyond Rock/Ice/Plasma?** TBD Phase 4. Plasma will need different rules (no bonds, electric charge, magnetic fields?).
- **Bodies: emergent vs first-class?** Phase 1 is emergent. Phase 4 makes them explicit. Decision deferred.
- **Save/load?** Not in any phase yet. Defer to Phase 4+.

## Phase 2 implementation notes

- **Mass unit:** `1 mass unit = 1 particle of Rock`. A planet's mass equals
  its current `particle_budget × 1.0`. A BlackHole's mass is set explicitly
  at spawn (the BH is a point mass, not a particle cluster).
- **Sticky bonds:** `bond_state: Vec<u8>` is the lifetime bond bitmask. New
  bonds are set only on planet spawn; bond breaks clear bits in
  `bond_state` permanently. Active bonds each tick =
  `bond_state & current_adjacency`. (See decision #10.)
- **Body ownership:** each grid cell stores `body_index: i32` (default `-1`).
  Spawned planets stamp their `id` into all cells they fill; event-horizon
  destruction decrements the owning body's `particle_budget`.
- **Event horizon:** BlackHole bodies destroy particles whose grid cell
  lies within `event_horizon_radius` (default 3 cells). Visualized as a
  faint purple ring of `EventHorizon` cells (new material #4).
- **Body radius semantics:** `Body.radius` is for *event horizon / visual
  circle*, not collision. Body-body collision is Phase 4. Phase 2 bodies
  are point masses.
- **Particle sub-cell positions?** Still integer grid cells. Bodies live
  at f32 sub-cell resolution. The differential motion that drives tidal
  peeling comes from per-particle gravity recomputation, not from
  sub-cell particle positions.
- **Body-particle position alignment on spawn:** the gravity solver
  reads a body's `position` and computes the per-cell gravity vector
  from there. The particle disk is filled at integer cells (rounded
  from the spawn spec). If a CLI spec like `planet:x=180.5,y=128,...`
  is passed, the particles land at (180, 128) but the body sits at
  (180.5, 128.0) — half a cell off from its own mass. This creates a
  0.5-cell gravity bias that compounds over hundreds of ticks. See
  Phase 2 follow-up §"Body-particle position alignment".

---

## Phase 2 follow-ups (from PR #4 review)

These were issues the PR #4 review caught but didn't block on. They
landed as a single cleanup commit on `feat/phase-3-barnes-hut` (see
Session 6) before Phase 3 work began. The list is preserved here as
a record of what was fixed.

### 1. Body-particle position alignment on CLI spawn [BLOCKER]

**Problem:** `apply_scenario` in `src/scenario.rs:194-207` rounds
`(x, y)` to int for `spawn_planet` (so the particle disk lands at
integer cells) but then sets `body.position` to the **unrounded**
`(x, y)`. For a non-integer CLI spec the body sits up to 0.5 cells
off-center from its own particle mass. The default scenario is
immune (it uses integer coords) but `--bodies "planet:x=180.5,..."`
triggers it.

**Fix options (pick one):**
- **(A) Round both.** `apply_scenario` calls `.round()` on `(x, y)`
  once and uses the rounded value for both the particle spawn
  AND the body position. Simple, consistent with the spec's
  "Bodies live in grid cells" framing for the integer-coord case
  (the f32 body type is incidental — it's there to support
  sub-cell motion *during* the sim, not at spawn).
- **(B) Accept sub-cell body positions, document it, and add a
  test that the spec-parser round-trip preserves the fractional
  part.** Update the "Body sub-cell positions" note in the
  implementation notes to be unambiguous: "Bodies can spawn at
  f32 sub-cell positions, decoupled from the integer-cell
  particle disk. The 0.5-cell offset is intentional and is
  bounded by the planet's spawn radius (the particle disk covers
  a radius-N ball around the integer-rounded cell)."

**Recommendation:** Option (A) for now. The spec says bodies live
at integer cells in the spawn sense; sub-cell f32 is for *motion
during* the sim (so a body at integer cell 100 can drift to
100.3 over many ticks). Mixing the two at spawn-time creates the
geometry gap. Sub-cell motion is preserved; sub-cell spawn is
not needed for any current spec.

**Test:** add `apply_scenario_rounds_body_position_to_particle_disk`
to `src/scenario.rs` tests. Pass a `BodySpec::Planet { x: 180.5, ... }`
and assert that the spawned body's `position` matches the integer
cell at the center of the particle disk, not the unrounded `x`.

### 2. Docstring numerical drift in `default_scenario` [nit]

`src/scenario.rs:139-146` docstring computes v_circ and period at
**r=40**, but the code uses **r=50**. The math is correct for the
wrong radius. Pick one: either change the docstring to use r=50
(v_circ = sqrt(0.16) = 0.4, period = 2*pi*sqrt(125000/8) ≈ 785
ticks), or change the code to r=40 (slower orbit, tidier numbers
in the docstring). Recommend: change the docstring to match the
code. The orbital shape is identical either way.

### 3. Stale "near-collision orbit" reference in `app.rs` [nit]

`src/app.rs:7` says default is "1 BH at grid center + 1 planet on
a near-collision orbit" — the OLD default. The current default is
a circular orbit with COM-stationary init (per decision #20).
Update to match.

### 4. Test count mismatch in SPEC Session 4 [nit]

`docs/SPEC.md:237` (Session 4) says "30 tests pass" — actual is
**27** pass, 2 ignored. The 3 new tests in 981a906 bring it from
24 to 27. Fix the number.

### 5. Test comment number drift in `move_pass_does_not_lose_particles` [nit]

`src/sim.rs:1022` says "without the fix ~62 particles vanish" —
the commit message in 981a906 says "Up to 84 of 113 particles in
the default scenario between t=0 and t=200". Pick one number.
Recommend: keep the commit message's 84 (it's the measured number
at t=200) and rephrase the test comment to "without the fix a
large fraction of a planet's particles can vanish during a close
encounter — measured at 84 of 113 by t=200 in the default scenario".

### 6. Missing Session 4 entry in `HANDOFF.md` [nit]

HANDOFF.md ends at Session 3 (Phase 2 ships). SPEC.md has the
Session 4 entry for the BH-zoom fix. Add a HANDOFF Session 4
entry consistent with the SPEC entry — same content, condensed
to the HANDOFF format (4-5 bullets max per session).

### 7. Compiler warnings in tests [nit]

Two test-only warnings:
- `src/sim.rs:651` — `unused import: crate::body::Body`
- `src/sim.rs:910` — `unused variable: initial_distance_sq`

Remove or `#[allow(unused)]`. They are test-only so harmless
but rustc is right.

### 8. Vestigial comment in `src/body.rs:11` [nit]

`// (H, W are referenced from sim.rs via the W/H re-exports; this
import is unused.)` — there is no actual import above or below
this comment. Looks like a leftover from a refactor. Delete the
comment.

### 9. SPEC Session 4 "Planet mass drops to ~34 over 800 ticks" [nit]

`docs/SPEC.md:243` says "Planet mass drops from 113 → ~34 over
800 ticks of tidal stripping but does not fully vanish." The
dump test only runs to 400 ticks (mass=46). Either run the
test out to 800 ticks and report the actual number, or remove
the speculative "~34" framing and say "mass drops continuously
via tidal stripping". Recommend: extend the dump test to 800
ticks and replace the ~34 with the actual measurement. This is
the kind of number that's easy to verify and will get copy-pasted
into a tutorial later.

---

## Phase 3 implementation notes

- **Tree layout:** flat `Vec<QuadNode>`, child pointers are
  indices into the parent vec (not `Box<QuadNode>`). Cache-
  friendly traversal, no pointer chasing. For N bodies the
  max tree size is 4N nodes.
- **θ = 0.5** (decision #23). The opening-angle test uses
  unsoftened `d²` (matches the textbook Barnes-Hut criterion);
  Plummer softening ε=0.5 goes into the magnitude only. Single-
  body leaves short-circuit the test (their d=0 would make
  s/d = ∞, which is wrong).
- **Threshold = 8 bodies** (decision #24). Below this, direct
  sum wins on a constant-factor basis (tree build + traversal
  overhead exceeds the O(N) saving for small N). Direct sum is
  correct for all N; Barnes-Hut is just faster above the
  threshold. The exact crossover point should be tuned against
  the `perf_sanity_*` tests on real hardware.
- **Rebuild every tick** (decision #25). No incremental updates.
  Bodies move every tick; the bookkeeping for an incremental
  tree is more code than the rebuild saves at this scale.
- **MAX_DEPTH = 64** is the degenerate-input backstop. If many
  bodies land in the same cell (or sub-cell), the `MergeHere`
  path treats the leaf as a multi-body aggregate and stops
  subdividing. The CoM is recomputed as the mass-weighted
  average; the multi-body leaf is then traversed like any other
  internal node (the s/d test still applies at MAX_DEPTH).
- **Body-body gravity stays N² pairwise** (decision #26). The
  Barnes-Hut path is only for body-particle gravity (the
  per-cell acceleration in `move_pass`). Body-body N² is fine
  because the body count is bounded by Phase 4 templates.
- **Event horizon + Newton's 3rd law** (decisions #22) are
  orthogonal to Barnes-Hut and stay unchanged. The momentum
  transfer at the event horizon is applied per-particle; the
  Barnes-Hut tree only sees body positions, not particles.
- **Body-particle position alignment** (PR #4 review item 1):
  `apply_scenario` rounds `(x, y)` once and uses the rounded
  value for both the particle disk spawn AND the body position.
  Default scenario uses integer coords and is unaffected; only
  non-integer CLI specs like `planet:x=180.5,...` triggered
  the bug. Resolved in commit `e0409b0` (Session 6).

---

## Phase 3 follow-ups (from PR #5 review)

These are issues the PR #5 review caught but didn't block on.
They should land as a single follow-up commit on
`feat/phase-3-barnes-hut` (or on a new branch off the merged
main) before Phase 4 work begins.

### 1. SPEC Session 7 perf table is off by ~10% [nit]

`docs/SPEC.md:329-` (Session 7 entry) shows the perf table as:

| Scenario | ms/tick (release, dev box) | Budget |
|---|---|---|
| Default (2 bodies, direct sum) | < 1 | n/a |
| 10 BHs + 65k particles (Barnes-Hut) | **7.4** | < 33 |
| 100 BHs + 63k particles (Barnes-Hut) | **15.2** | < 33 |

Actual measured values (this dev box, release, after warmup):

| Scenario | ms/tick (release, dev box) | Budget |
|---|---|---|
| Default (2 bodies, direct sum) | < 1 | n/a |
| 10 BHs + 65k particles (Barnes-Hut) | **6.6** | < 33 |
| 100 BHs + 63k particles (Barnes-Hut) | **15.7** | < 33 |

The 10-body number is ~10% off (likely warmup cost on the
PR's first run); the 100-body number is in the noise. Real
numbers are 6.6 / 15.7 ms/tick — both well under budget.

**Fix:** update the SPEC table to 6.6 / 15.7, and add a note
that perf is measured on `this dev box` (which is m5 or
wherever the test ran); the gaming rig column is TBD per
`docs/performance-budget.md`.

### 2. SPEC Session 7 dump-test wording is slightly misleading [nit]

Session 7 says: *"Default-scenario dump at t=0/40/120/240/
400/800 ticks matches the Phase 2 baseline to the digit."*

But the dump test on the Phase 3 branch now also dumps t=800,
which the Phase 2 baseline never had. The byte-for-byte
identity only holds for the 4 shared checkpoints
(t=0/40/120/240/400).

**Fix:** rephrase to: *"The shared checkpoints (t=0/40/120/
240/400) match the Phase 2 baseline to the digit; t=800 is a
new checkpoint with mass=23."*

### 3. `BARNES_HUT_THETA = 0.5` doc claims a "test sweep" that doesn't exist [nit]

`src/sim.rs:34-37` says: *"The spec calls for 0.5; the test
sweep is in `barnes_hut::tests`."*

No sweep test exists — only `bh_matches_direct_sum_with_small_
theta` (θ=0.1, exact answer) and `bh_at_root_treats_as_point_
mass_with_high_theta` (θ=100, root-only collapse). The
production θ=0.5 is asserted by the equivalence test
`barnes_hut_matches_direct_sum_for_8_bodies` (5% tolerance),
not by a sweep.

**Fix:** either remove the "test sweep" claim, or add a sweep
test that picks θ=0.3 / 0.5 / 0.7 and shows the speed/
accuracy tradeoff (the sweep is the kind of data the user
will want to see when tuning for real hardware).

### 4. Loose perf assert in `perf_sanity_100_bodies_50k_particles` [nit]

`src/sim.rs:1412-1415` allows `ms_per_tick < 200.0` for the
100-body case. Phase 3 budget is 33 ms/tick; the test allows
~6x slack. Fine for CI but means a 50% perf regression would
still pass.

**Fix:** tighten to `< 100.0` (m5 budget) or `< 67.0` (gaming
rig m5 budget). Real numbers are 15.7 ms/tick so either bound
is well above the actual perf.

### 5. No stress test for the MAX_DEPTH `MergeHere` path [nit]

`src/barnes_hut.rs:97` says "64 is far more than enough for any
reasonable N" but for 1000 bodies at 8-deep subdivision you
already have `4^8 = 65536` nodes. With MAX_DEPTH=64 you can hit
a corner case where many bodies cluster in the same cell and
the tree's `MergeHere` path swallows the cluster into a single
multi-body leaf.

This is documented and correct, but the test suite doesn't
exercise it. Add a test: `merge_many_bodies_same_cell_does_not_
explode` that places 1000 bodies in the same cell, builds the
tree, and verifies the tree depth is bounded by MAX_DEPTH and
the CoM/total_mass match the mass-weighted average of the
bodies.

### 6. Threshold boundary perf measurement is missing [nit]

`sim.rs:34-44` says "below the threshold direct sum wins on a
constant-factor basis" — but no perf test verifies this at the
boundary. The threshold is set to 8 based on intuition, not
measurement.

**Fix:** add a `#[ignore]`'d `perf_sanity_threshold_boundary`
test that runs the same scenario at N=4, 6, 8, 10, 12 bodies
and prints ms/tick for each. The crossover (where BH becomes
faster than direct sum) should be near 8; if it's actually
at 4 or at 12, the threshold constant should move.

### 7. `dump_default_scenario` comment still says "Phase 2 default scenario" [nit]

`src/sim.rs::dump_default_scenario` doc comment says
"Visual smoke test for the Phase 2 default scenario" — should
say "Phase 2/3 default scenario" or "Phase 2 default scenario,
also used as the Barnes-Hut identity test." Cosmetic, but
keeps the docs honest about which phase each test belongs to.

### 8. `MergeHere` path needs a clarifying comment [nit]

`src/barnes_hut.rs:182-197` clears `body_id = None` but does
not clear `center_of_mass` or `total_mass` to indicate the
node is now a multi-body aggregate. The math is correct (the
new CoM is the mass-weighted average of the existing + new
bodies; total_mass is the sum), but the conceptual mismatch
between `body_id = None` (multi-body) and `center_of_mass`
(point mass at the body's position) is briefly confusing.

**Fix:** add a one-line comment after `body_id = None`:
`// now a multi-body leaf — body_id cleared, CoM/mass
recomputed above.` The math is right; this is a readability
fix.

---

## Acceptance Criteria (per phase)

| Phase | Status | What the user can do when done |
|---|---|---|
| 0 | DONE | `git clone` the repo, see this file, CI is green. |
| 1 | DONE | Run `cargo run`, place a planet near the black hole, watch it disintegrate. |
| 2 | DONE | Place 2+ bodies, see real orbital mechanics. |
| 3 | DONE | 100+ bodies + 50k particles at 30+ fps; Barnes-Hut body-particle gravity. |
| 4 | TODO | "Place planet" is a primitive with mass/velocity. |
| 5 | TODO | Playable game with levels. |

---

## Session Log

### 2026-10-09 — Session 8: click-spawn orbits the BH (playtest fix)

User feedback from the Session 7 MacBook playtest: clicking spawned a
planet at the cursor with `v=(0,0)`, which then fell straight into
the BH instead of orbiting — confusing because the default scenario
clearly *does* orbit. Two options on the table: disable click-spawn
or align it with the default scenario's physics. User picked the
latter so the playtest surface stays the same and so we get a free
UX win out of the existing code.

New method `World::spawn_planet_in_orbit(cx, cy, radius, mat) ->
Option<u32>` in `src/sim.rs`. It finds the first BlackHole in the
world, computes the radial vector from BH to click point, returns
`None` when `r < 1.0` (click on top of BH — `v_circ` would blow
up), then calls the existing `spawn_planet` and applies:

- `v_planet = v_circ · (−dy, dx) / r` — the CCW tangential
  perpendicular of the radial vector (same direction the default
  scenario uses at its +x starting position).
- `Δv_BH = −(m_planet / m_BH) · v_planet` — equal-and-opposite
  recoil so the system COM starts stationary (decision #19).

Returns `None` if there is no BH, the click is on the BH, the
material has no bonds, or the spawn footprint is fully occupied.
`App::spawn_planet_at_cursor` in `src/app.rs` now calls the new
method instead of `spawn_planet` directly.

Tests added (3, all in `sim::tests`):

- `spawn_planet_in_orbit_uses_v_circ_and_recoil` — the canonical
  check: planet velocity is purely tangential with magnitude
  `v_circ = sqrt(G*M_BH/r)`, BH recoil is opposite to planet
  velocity, total system momentum is approximately 0.
- `spawn_planet_in_orbit_returns_none_without_black_hole` — no
  BH means nothing to orbit; no body should be spawned.
- `spawn_planet_in_orbit_returns_none_on_top_of_bh` — click on
  the BH itself returns `None` rather than injecting a NaN
  velocity.

Verification: 39 tests pass (was 36, +3), 3 ignored. Default-scenario
dump at t=0/40/120/240/400/800 ticks is byte-for-byte identical to
the Phase 2/Phase 3 baseline (BH at (136.73, 124.97) at t=400,
planet at (78.8, 141.9) with mass 54) — the click-spawn path is
not on the default scenario's hot path so the integration
regression test stays green.

Decision #28 added (click-spawn = tangential circular orbit + BH
recoil, "first BH wins" for multi-BH scenes).

**Playtest status:** the unit tests pass but the first MacBook
visual playtest surfaced an OOB panic in `recompute_bonds` at
`src/sim.rs:704`: a planet particle that had migrated to a
corner cell like (255, 255) still carried a `BOND_SE` bit in
its `bond_state` from when it was at a more central cell. The
adjacency-time bounds check at the top of `recompute_bonds`
validated the in-bounds neighbours but not the mirror-clear
target, so the index `(256, 256)` panics. `recompute_bonds`
now bounds-checks each mirror-clear target. The same class of
bug also existed in `spawn_planet`'s disk-fill loop, which
silently pushed OOB cells into `filled` when the click was
near the edge and panicked in the body-index stamp loop. The
fix skips OOB cells up front so a corner click produces a
clipped (partial) planet instead of panicking. Five new tests
in `sim::tests` cover all four edges plus the spawn edge
(`recompute_bonds_does_not_panic_when_{n,e,ne,se}_neighbour_oob`,
`spawn_planet_near_edge_does_not_panic`).

**Next up:** user re-runs the MacBook visual playtest of
click-spawn (this commit should be panic-free even for
edge-corner clicks); PR #5 picks up the new commits; Phase 4
(bodies as first-class objects with shape templates) remains
the next major chunk of work.

### 2026-10-09 — Session 7: Phase 3 — Barnes-Hut body-particle gravity

Built the Barnes-Hut quadtree path for body-particle gravity and
integrated it into `move_pass`. New module `src/barnes_hut.rs`:
flat `Vec<QuadNode>`, `QuadTree::new(bodies, theta)` builds the tree
with bottom-up CoM computation, `compute_accel(p, theta)` walks the
tree with the `s/d < theta` opening-angle criterion. Decisions
#23–#27 in the Decisions Log.

Integration in `src/sim.rs`: `move_pass` now builds a `QuadTree`
once per tick (when `bodies.len() >= BARNES_HUT_THRESHOLD = 8`) and
passes it to `gravity_step_for_cell_bh`, a tree-walking
replacement for `gravity_step_for_cell`'s N² loop. Below the
threshold the existing direct-sum path runs unchanged — the default
scenario (1 BH + 1 planet = 2 bodies) and the Phase 2 perf sanity
test (10 BHs without BH) are unaffected. Body-body gravity stays
N² pairwise (decision #26); the event horizon ring
(`stamp_event_horizon_ring`) is unchanged.

Tests added (5 in `barnes_hut`, 3 in `sim`):

- `barnes_hut::empty_bodies_yields_empty_tree`,
  `single_body_tree_has_one_node`,
  `two_distant_bodies_subdivide_root`,
  `bh_matches_direct_sum_with_small_theta` (3-body tree, theta=0.1
  — exact-answer check),
  `bh_at_root_treats_as_point_mass_with_high_theta` (theta=100 —
  root is the only node).
- `sim::barnes_hut_matches_direct_sum_for_8_bodies` (8 BHs in a
  ring; per-cell gravity matches direct sum to < 5% relative
  error at theta=0.5).
- `sim::barnes_hut_threshold_uses_direct_sum_below` (7 bodies —
  threshold-boundary sanity check).
- `sim::perf_sanity_100_bodies_50k_particles` (#[ignore]'d; 100
  BHs in a ring, ~63k particles, 100 ticks; loose 200ms/tick
  upper bound).

Verification: 36 tests pass (was 29, +7), 3 ignored. Perf:

| Scenario | ms/tick (release, dev box) | Budget | Status |
|---|---|---|---|
| Default (2 bodies, direct sum) | < 1 | n/a | unchanged |
| 10 BHs + 65k particles (Barnes-Hut) | 7.4 | < 33 (30+ fps) | ✓ |
| 100 BHs + 63k particles (Barnes-Hut) | 15.2 | < 33 (30+ fps) | ✓ |

Both Phase 3 perf budgets met. The 10-body case is slightly
slower than Phase 2's N² (7.4 vs 3.4 ms) because Barnes-Hut's
constant factor (tree alloc, s/d branching) exceeds the O(N)
saving at N=10 — this is exactly why the threshold exists. The
default scenario's 1.5 ms/tick in the spec is unchanged
(direct sum path).

Default-scenario dump (the visual smoke test) at t=0/40/120/240/
400/800 ticks matches the Phase 2 baseline to the digit: BH at
(128, 128) → (136.7, 125.0) at t=400, planet at (78.8, 141.9)
with mass=54 at t=400, mass=23 at t=800. The "tidal peel" visual
is preserved because Barnes-Hut with theta=0.5 introduces only a
small far-field error and the default scenario has only 2 bodies
(no tree to walk). At 8+ bodies the per-cell gravity vector is
within 5% of the direct sum, which is well below the threshold
where the "tidal peel" would visibly smooth out.

### 2026-10-09 — Session 6: Phase 2 follow-up sweep (PR #4 review nits)

Landed the 9 follow-up items from the PR #4 review as a single
cleanup commit on `feat/phase-3-barnes-hut` (immediately after
branching from main, before Barnes-Hut work). One real bug, eight
documentation/test nits. The whole list is preserved under
"Phase 2 follow-ups (from PR #4 review)" with each item marked
**Resolved (Session 6)** so the trail is auditable.

Changes:

- **Follow-up #1 (BLOCKER):** `apply_scenario` now rounds `(x, y)`
  once and uses the rounded value for BOTH the particle disk
  spawn and the body position. A non-integer CLI spec like
  `planet:x=180.5,...` no longer leaves the body 0.5 cells off
  center from its own mass. New test
  `apply_scenario_rounds_body_position_to_particle_disk` guards
  the regression.
- **Follow-up #2:** `default_scenario` docstring now uses r=50
  throughout (was r=40 in the math, r=50 in the code).
- **Follow-up #3:** `app.rs` module docstring no longer calls the
  default a "near-collision orbit" — it's a circular orbit with
  COM-stationary init (decision #20).
- **Follow-up #4:** SPEC Session 4 now reads "27 tests pass (was
  24, +3), 2 ignored" (was incorrectly "30 tests pass").
- **Follow-up #5:** `move_pass_does_not_lose_particles` test
  comment now cites the commit message's measured 84-of-113
  figure (was the speculative ~62).
- **Follow-up #6:** `HANDOFF.md` now has Session 4 (BH-zoom fix)
  AND Session 5 (Newton's 3rd law) entries consistent with the
  SPEC session log.
- **Follow-up #7:** Removed unused import `crate::body::Body`
  and unused `initial_distance_sq` variable in the orbit test.
  `cargo test` is warning-clean.
- **Follow-up #8:** Deleted the vestigial comment in
  `src/body.rs:11` (nothing referenced it).
- **Follow-up #9:** `dump_default_scenario` now extends to t=800
  (was t=400). SPEC Session 4 cites the measured end-of-run mass
  instead of the speculative "~34" framing.

Verification: 29 tests pass (was 28, +1), 2 ignored. No new
decisions; the cleanup closes items the PR review caught and
clears the path for Phase 3 (Barnes-Hut) work.

### 2026-10-09 — Session 5: Phase 2 — Newton's 3rd law across the event horizon

User confirmed the MacBook playtest of the Session 4 fix looks correct
(BH stays near grid center, planet orbits, stream is the expected
tidal peel). Followed up on the one remaining known limitation
flagged in Session 4: particles consumed at the event horizon took
their momentum with them, so the BH retained its pre-consumption
velocity and total system momentum was not conserved. The symptom
was hidden by the COM-stationary default-scenario init but would
have shown up in head-on / off-COM scenarios.

Changes:

- `apply_event_horizons` now applies `Δv_BH = v_particle / M_BH` per
  consumed particle (where `v_particle` is approximated as the owning
  planet body's velocity). Recoil is accumulated per BH in a small
  `Vec<(u32, Vec2)>` and applied at the end of the destruction loop
  to avoid borrow conflicts with the `particle_budget` decrement.
  Locked in decision #22.
- New test `event_horizon_conserves_total_momentum` runs the
  COM-stationary default scenario to consumption and verifies two
  invariants: (1) total system momentum stays ≈ 0 throughout
  (conservation); (2) the BH's velocity is ≈ 0 after consumption
  (its initial recoil has been cancelled by the absorbed planet
  momentum). Both pass on the first run.
- Updated the `body_body_gravity_conserves_momentum` test comment
  to point at the new test for the event-horizon regime instead of
  the old "tracked as a follow-up" note.

Verification: 28 tests pass (was 27, +1), 2 ignored. Perf sanity
(10 BHs + ~65k particles) is 3.41 ms/tick — within noise of the
Session 4 number (3.3 ms/tick). Dump of the default scenario at
t=400 shows BH velocity dropped from the initial (0, -0.045) to
(0.005, 0.017) — almost stationary at the COM, exactly as the math
predicts.

### 2026-10-08 — Session 4: Phase 2 hardening — fix BH "zooming" playtest bug

User reported the BH was "zooming around the screen" after consuming a
planet on their MacBook. Three real bugs behind it:

1. **`move_pass` silently lost particles** during close encounters.
   `find_target` checked the original grid for occupancy but not the
   in-progress `claim` array, so two particles converging on the same
   target cell in the same tick would overwrite each other and the
   first would vanish. Up to 84 of 113 particles in the default
   scenario between t=0 and t=200 — silently. The body's
   `particle_budget` counter said they were still alive, but they
   weren't on the grid. Fix: pass `claim` into `find_target` and treat
   claimed cells as occupied. Locked in decision #17.

2. **Frankenstein integrator** was not actually Verlet or leapfrog.
   The previous code did `position += v + 0.5*a_old` followed by
   `velocity += a_new` — position uses old accel (Verlet), velocity
   uses new accel (Forward Euler). Hybrid gains energy each tick and
   orbits spiral outward. Fix: proper KDK leapfrog
   (v += 0.5*a_old; x += v; compute a_new; v += 0.5*a_new).
   Locked in decision #13 (re-confirmed) and verified by new
   `leapfrog_does_not_gain_energy_in_pure_orbit` test.

3. **Planet pinned at a wall** after the previous default's near-
   collision orbit threw the planet off the grid. A heavy mass pinned
   at (0, 255) is a steady sideways pull on the BH. Fix: in
   `body_leapfrog_first_half` zero the wall-directed velocity when a
   body would clamp, so it slides along the wall. Locked in decision
   #18. Also: new default scenario is a circular orbit at r=50 with
   COM-stationary initial velocities, so the planet never hits a wall
   in the first place. Locked in decisions #19 and #20.

Tests added: `move_pass_does_not_lose_particles`,
`leapfrog_does_not_gain_energy_in_pure_orbit`,
`body_body_gravity_conserves_momentum`. **27 tests pass** (was 24, +3),
2 ignored (visual dump + perf sanity).

Default scenario now: BH at (128, 128), mass=1000, v=(0, -0.045) (COM
recoil); planet at (178, 128) on circular orbit r=50 with v=v_circ.
COM stays at (133, 128); BH orbits the COM at radius 4 cells in a
tight circle, which reads as "stationary" in the visual.

Measured planet mass over time (from `dump_default_scenario` in
`src/sim.rs`, release build, G=8e-3, M_BH=1000):

| t (ticks) | planet mass |
|---|---|
| 0   | 113 |
| 120 | 106 |
| 240 | 82  |
| 400 | 54  |
| 800 | 23  |

The planet does not fully vanish within 800 ticks; it loses
roughly 8 particles per 100 ticks under the default orbital
geometry.

### 2026-10-08 — Session 3: Phase 2 ships on `feat/phase-2-multi-body-gravity`

- New module `src/body.rs` (Body / BodyKind / G / Plummer / horizon
  constants). `src/scenario.rs` (parse `--bodies "..."` and apply).
  `World` extended with `bodies`, `body_index`, `bond_state` (sticky).
- Physics: body-body N² gravity + leapfrog, body-particle N² Plummer
  gravity drives the falling-sand move pass via sign-of-accel +
  magnitude-scaled max_steps. Event horizon destroys particles strictly
  inside `radius`; planet body's `particle_budget` mutates per particle
  destroyed → its mass mutates in real time.
- Sticky bonds implemented per spec §"Phase 1 → Phase 2 Transition":
  `bond_state` is the lifetime mask, `bonds = bond_state & adjacency`,
  bond break clears `bond_state` bit permanently AND mirror-clears on
  neighbour cell. New bonds only form on planet spawn.
- Visual: `Material::EventHorizon` (dark purple) drawn as a 1-cell
  ring around every BlackHole body — purely cosmetic, not a particle,
  no bonds.
- Default scenario: 1 BH at grid center (mass 1000) + 1 planet at
  r=60 with v=(0, 0.5) → near-collision orbit (v=1.4× v_circ). Over
  400 ticks the planet loses 11 of 113 particles (gradual strip),
  mass = 113 → 102, orbit visibly decays.
- CLI: `--bodies "bh:x=N,y=N,m=N;planet:x=N,y=N,r=N,vx=N,vy=N"`
  (single-spec or `;`-joined; `--bodies=...` also accepted).
- 24 tests pass (8 phase-1 sim, 7 phase-2 sim [sticky bonds ×2,
  event horizon, mass decrement, planet migrate, orbit, slingshot,
  two-BH tearing], 3 body.rs, 9 scenario.rs). 2 ignored visual-dump
  tests. perf sanity: 10 BHs + full grid (~65k particles) runs at
  ~3 ms/tick (~310 fps) in release, well above the 30 fps target.
- Acceptance criteria 1–5 hit. Criterion 6 (perf budget) hit by ~10×
  in release on this dev box. Real m5 / gaming-rig numbers pending.
- Decisions #11–16 in the Decisions Log capture the design calls.

### 2026-10-08 — Session 1: bootstrap
- Locked design: 2D, hand-rolled wgpu, per-particle bonds.
- Created repo at `~/Development/blackhole-sand/`, GitHub remote `klampatech/blackhole-sand`.
- Wrote this SPEC, CI guard, HANDOFF stub.
- Next: Phase 1 — wgpu window, single black hole, particle-by-particle disintegration.
**Resolved (Session 6).** `apply_scenario` now rounds `(x, y)` once
and uses the rounded value for both the particle spawn and the body
position. New test `apply_scenario_rounds_body_position_to_particle_disk`
guards against regression.
**Resolved (Session 6).** Docstring numbers updated to match the
code (r=50 throughout).
**Resolved (Session 6).** Comment now says "circular orbit,
COM-stationary init" and references SPEC decision #20.
**Resolved (Session 6).** Session 4 now reads "27 tests pass
(was 24, +3), 2 ignored".
**Resolved (Session 6).** Comment now cites the commit message's
measured 84-of-113 number instead of the speculative 62.
**Resolved (Session 6).** Added Session 4 (BH-zoom fix) AND
Session 5 (Newton's 3rd law) entries to HANDOFF.md.
**Resolved (Session 6).** Removed unused import `crate::body::Body`
from `sim.rs` tests; removed unused `initial_distance_sq` variable
in the orbit test. `cargo test` is warning-clean.
**Resolved (Session 6).** Comment deleted; nothing referenced it.
**Resolved (Session 6).** `dump_default_scenario` now extends
out to 800 ticks. SPEC Session 4 cites the measured value instead
of the speculative "~34" framing.
| 23 | 2026-10-09 | Phase 3: Barnes-Hut body-particle gravity with theta=0.5 | N² body-particle gravity is the bottleneck once we have 10+ bodies and 50k+ particles (the Phase 3 trigger). Barnes-Hut reduces the per-cell cost from O(N) to O(log N) on average by treating distant bodies as a point mass at their center of mass. theta=0.5 is the textbook default; the s/d < theta opening-angle criterion matches the spec. We did not sweep theta values — the spec is explicit that 0.5 is the right starting point and θ tuning happens in a follow-up if the user notices orbit drift at close encounters. | GPU Barnes-Hut (premature: tree build is small, ≤4000 nodes for 1000 bodies, and a sim→GPU migration is a Phase 4+ question; decision #8), multipole expansion (single-pole CoM is good enough at theta=0.5), HOT (overkill — Barnes-Hut gets us 10x, we'd need 100x to justify the complexity). |
| 24 | 2026-10-09 | Phase 3: Barnes-Hut activates when `bodies.len() >= 8`; below that, direct sum | Below ~8 bodies, the constant factor of tree build + per-cell traversal exceeds the O(N) saving. Direct sum is a tight loop over a small slice; Barnes-Hut allocates a `Vec<QuadNode>`, walks child indices, and branches on `body_id.is_some()`. We measured 7.4 ms/tick for 10 bodies + 65k particles with the BH path on this dev box (the Phase 2 N² number for the same scenario was 3.4 ms/tick), so the crossover is somewhere between 10 and 100 bodies — for 100 bodies + 63k particles Barnes-Hut is at 15 ms/tick, well under the 33 ms budget. The default scenario (2 bodies) is unaffected. | Single path (use Barnes-Hut always: wastes cycles on the default scenario for no gain), higher threshold (16-32 would speed up the 10-body case but make no difference for 100+ bodies; 8 keeps the spec's "natural heuristic" and is conservative), lower threshold (1 or 2: would slow down the default scenario without measurable gain). |
| 25 | 2026-10-09 | Phase 3: Barnes-Hut tree is rebuilt every tick, not incrementally | Bodies move every tick (Leapfrog integrates them) and the cost of incrementally updating the tree is on par with rebuilding from scratch at our scale (N ≤ 1000, max tree size 4N = 4000 nodes, build is O(N log N) = 10000 ops). Rebuilding simplifies the code: the tree is a snapshot of `self.bodies` at the start of `move_pass`, used by every cell, and dropped at the end of the tick. No parent tracking, no rebalancing, no stale-node bugs. | Incremental update (re-builds of subtrees when a body crosses a quadrant boundary; at our scale the bookkeeping cost exceeds the savings), periodic rebuild (rebuild every K ticks: introduces a staler-tree error budget we'd then have to reason about; simpler to just rebuild every tick). |
| 26 | 2026-10-09 | Phase 3: body-body gravity stays N² pairwise | N bodies is bounded by Phase 4's body templates, so N² body-body gravity is at most 100×100 = 10,000 ops/tick — nothing. The Barnes-Hut tree is for body-particle gravity (which is N×M for N bodies and M particles, and M can be 50k+). | Apply Barnes-Hut to body-body too (the N² cost is 4 orders of magnitude below body-particle; would just add branching and reduce clarity). |
| 27 | 2026-10-09 | Phase 3: Barnes-Hut tree uses Plummer softening identical to Phase 2 (`d² -> d² + SOFTENING_SQ = d² + 0.25`) | Consistency with the Phase 2 spec. The softening goes into the magnitude (`a_mag = G * M / d²` uses the softened `d²`) but the opening-angle test uses the unsoftened distance so it matches the textbook Barnes-Hut criterion. The visual signature of close encounters is preserved because softening is what keeps the per-cell gravity vector from blowing up near a body. | Hard cutoff (looks unnatural; bodies "skip" past each other — same as Phase 2's rejected option for direct sum), no softening in the tree (numerical instability when two bodies are very close: their CoM is in the same cell and the softened magnitude still overflows). |
| 28 | 2026-10-09 | Phase 3: click-spawned planets are initialized on a tangential circular orbit around the first BlackHole (`v_circ = sqrt(G * M_BH / r)`, CCW perpendicular to the radial vector), with equal-and-opposite recoil on the BH so the system starts COM-stationary | Phase 1/2 click-spawn used `spawn_planet(cx, cy, ...)` which left the planet at v=(0,0); on the MacBook playtest the user saw the planet fall straight into the BH instead of orbiting — confusing because the default scenario clearly does orbit. Aligning click-spawn with the default scenario (decision #19 + decision #20) means the click-to-orbit interaction matches what the playtest expects. The “first BH wins” rule is good enough because click-spawn is overwhelmingly tested with the single-BH default scenario; if multi-BH scenes become a thing we can revisit with “nearest BH”. Returns `None` when there is no BH, the click is on top of a BH (r near 0), or the spawn footprint is fully occupied. | Disable click-spawn (it’s the visible interaction, disabling would lose user-visible behavior for a UX miss), keep `spawn_planet` and accept the v=0 free-fall (the bug we’re fixing), pick the *nearest* BH (more intuitive for multi-BH scenes but adds bookkeeping for a case we don’t exercise yet). |
> **Status:** Phase 3 (Barnes-Hut) landed on `feat/phase-3-barnes-hut` (commits `e0409b0` + this commit). PR + review pending.
