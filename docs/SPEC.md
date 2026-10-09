# Blackhole Sand — Engine Spec

**Canonical source of truth: this file.** The vault is a one-way mirror (if/when we create one); this is authoritative. Edit on a branch + PR. No direct-to-main pushes.

> **Status:** Phase 0 — design locked, repo bootstrap. No engine code yet.

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

---

## Acceptance Criteria (per phase)

| Phase | Status | What the user can do when done |
|---|---|---|
| 0 | IN PROGRESS | `git clone` the repo, see this file, CI is green. |
| 1 | TODO | Run `cargo run`, place a planet near the black hole, watch it disintegrate. |
| 2 | IN PROGRESS | Place 2+ bodies, see real orbital mechanics. |
| 3 | TODO | 10k+ particles at 30+ fps. |
| 4 | TODO | "Place planet" is a primitive with mass/velocity. |
| 5 | TODO | Playable game with levels. |

---

## Session Log

### 2026-10-08 — Session 1: bootstrap
- Locked design: 2D, hand-rolled wgpu, per-particle bonds.
- Created repo at `~/Development/blackhole-sand/`, GitHub remote `klampatech/blackhole-sand`.
- Wrote this SPEC, CI guard, HANDOFF stub.
- Next: Phase 1 — wgpu window, single black hole, particle-by-particle disintegration.
