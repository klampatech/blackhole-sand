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
