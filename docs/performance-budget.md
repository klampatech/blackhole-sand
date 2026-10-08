# Performance Budget

> **Audience:** anyone tuning the engine, choosing algorithms, or asking "should we optimize this?" — read this first. T3 Code should also read this before making performance trade-offs.

---

## The Numbers

| Grid size | Particles (full) | m5 (integrated Radeon) | Gaming rig (TBD — placeholder) |
|---|---|---|---|
| 128×128 | 16k | 60 fps | 60+ fps |
| 256×256 | 65k | 30 fps | 60+ fps |
| 512×512 | 262k | 10 fps (dev only) | 60 fps |
| 1024×1024 | 1M | not viable | 30 fps |
| 2048×2048 | 4M | not viable | 10 fps |

**m5 is for development**, not target. T3 Code will iterate at small grid sizes there, then we move to the gaming rig for serious testing.

**Gaming rig hardware TBD.** The user mentioned they have one; specs not yet captured. Once we have a target machine, we fill in the column. Until then, "60+ fps at 512×512" is the assumption and we'll measure when we get there.

---

## Where the Time Goes

Per frame, ordered by cost on the CPU side:

1. **Gravity computation** — O(particles × bodies). Dominates when both are large. Phase 2: N². Phase 3: Barnes-Hut (O(N log N)).
2. **Bond stress** — O(particles), but with a constant factor of ~10 (you check 4 neighbors × check strain × maybe break). Cheap relative to gravity.
3. **Particle update / collision** — O(particles), constant factor ~5 (fall patterns, swap with empty neighbors). Cheap.
4. **Texture upload + render** — O(grid pixels), but on the GPU. ~1 ms at any reasonable size.

For Phase 1 with one body and no N² gravity, the bottleneck is the **render** or **bond stress**, not gravity.

---

## Specific Bottleneck Triggers

| Bottleneck | Threshold | Symptom | Fix |
|---|---|---|---|
| Gravity N² | 10k+ particles + 10+ bodies | fps drops linearly with product | Phase 3 Barnes-Hut |
| Bond update | 500k+ particles | per-frame time > 16 ms | SoA layout (already in spec); later, vectorize the inner loop |
| Texture upload | 1024×1024 grid | bandwidth-bound on PCIe | compress to palette-indexed upload + run-length encoded? Overkill for now |
| Render | never, at any sane size | — | GPU has trillions of FLOPS; a full-screen quad sample is nothing |

**Phase 1 won't hit any of these.** Phase 2 might hit gravity N². Phase 3 is when Barnes-Hut lands.

---

## What "Slow Path" Looks Like

If fps is below target, the first thing to do is **profile, don't optimize**. Concretely:

1. Add a frame-time log: `info!("frame time: {} ms", instant.elapsed().as_secs_f32() * 1000.0);`
2. Instrument each pass separately: gravity, bonds, collision, render.
3. Find the bottleneck. Optimize *that*.
4. Don't speculatively optimize parts that aren't slow.

T3 Code should not "improve performance" without a measurement showing what's actually slow.

---

## Profiling Tools

| Tool | Use |
|---|---|
| `cargo build --release` then run | Release builds are 5-10x faster than debug. Always measure in release. |
| `cargo flamegraph` | Per-function CPU time. See which sim pass is hot. |
| `perf record` on Linux | Same idea, lower-level. |
| wgpu's built-in timing | GPU pass timing via `CommandEncoder::begin_timestamp_pass`. Phase 4+ when we care about GPU. |

For Phase 1, we don't need any of this. Console logs are fine.

---

## Memory Budget

| Resource | At 512×512 | At 1024×1024 |
|---|---|---|
| `particles[]` (u8) | 256 KB | 1 MB |
| `bonds_n[]`, `bonds_e[]`, `bonds_ne[]`, `bonds_se[]` (u8 each) | 1 MB | 4 MB |
| GPU sim texture | 256 KB | 1 MB |
| GPU palette texture | 1 KB | 1 KB |

**Total sim state at 1024×1024: ~5 MB.** Trivial. We will never be memory-bound on the sim.

---

## The "One Optimization That Always Pays"

**Compile in release mode.** `cargo run --release` is 5-10x faster than `cargo run`. If fps looks bad, try this first. Don't go optimizing code that's actually fine in release.

---

## When to Profile vs When to Spec

- **Spec:** if we're about to write a new pass or change an algorithm (e.g., add Barnes-Hut). Performance budget is part of the spec.
- **Profile:** if we're tuning an existing pass that's slower than the spec says it should be. The spec is the contract; profiling finds the gap.

T3 Code should default to spec-first. If a perf issue shows up, profile, then update the spec with the actual numbers (replace estimates with measurements).