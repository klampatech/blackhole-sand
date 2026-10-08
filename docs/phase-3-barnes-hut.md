# Phase 3 — Barnes-Hut Octree

> **Status:** spec draft. Triggered when Phase 2's N² gravity becomes the bottleneck (around 10k+ particles, or 50+ bodies). Document what T3 Code reads when it's time to optimize.

---

## When We Need This

N² direct sum is `O(particles × bodies)` per frame. Concrete cost:

| Particles | Bodies | Ops/frame | 60 fps target | Notes |
|---|---|---|---|---|
| 5,000 | 5 | 25,000 | trivial | Phase 1/2 scale |
| 50,000 | 10 | 500,000 | fine on CPU | Phase 2 max |
| 100,000 | 50 | 5,000,000 | borderline on m5, fine on dGPU | Phase 3 trigger |
| 500,000 | 100 | 50,000,000 | impossible on CPU | Phase 3 trigger, definitely needs GPU |

**The Barnes-Hut approximation** reduces this to `O(N log N)` by treating distant bodies as a single point mass at their center of mass. A body's gravitational influence on a particle is computed by walking an octree of bodies: at each node, check if the node is "far enough" from the particle to approximate; if yes, use the node's total mass at its center of mass; if no, recurse into children.

The classic criterion: `s/d < θ` where `s` is the node's spatial extent, `d` is the distance from the particle to the node's center, and `θ` is a tunable accuracy parameter. `θ = 0.5` is standard; smaller is more accurate, larger is faster.

---

## Octree Structure

A 2D variant (technically a **quadtree**, but we keep the name "Barnes-Hut" for the algorithm). Each node:

```rust
struct QuadNode {
    // Either a leaf with one body's data, OR an internal node with 4 children
    center_of_mass: Vec2,
    total_mass: f32,
    bounds: Rect,             // x, y, w, h — for the θ test
    body_id: Option<u32>,     // Some(id) iff this is a leaf node
    children: Option<[Box<QuadNode>; 4]>,  // NW, NE, SW, SE; None iff leaf
}
```

Built once per frame from the current body list. Tree construction is `O(N log N)`. We rebuild every frame because bodies move — incremental updates are complex and not worth it at this scale.

**Memory:** for N bodies, max tree size is `4N` nodes (each internal node has 4 children, leaves are 1 body). For 1000 bodies, that's 4000 nodes × ~64 bytes = 256 KB. Fits in L2 cache on most CPUs.

**Layout:** use a flat array of nodes (`Vec<QuadNode>`) instead of `Box<QuadNode>` to keep memory contiguous and cache-friendly. Parent stores child indices, not pointers.

---

## The Algorithm (per particle, per frame)

```rust
fn compute_accel(particle_pos: Vec2, node: &QuadNode, theta: f32) -> Vec2 {
    let d = (node.center_of_mass - particle_pos).length();
    let s = node.bounds.width();  // assume square cells; or use max(w, h)

    // Far enough to approximate?
    if node.body_id.is_some() || s / d < theta {
        // Treat as point mass
        let r = node.center_of_mass - particle_pos;
        let dist_sq = r.length_squared() + EPSILON;
        let dist = dist_sq.sqrt();
        return G * node.total_mass * r / (dist_sq * dist);
    }

    // Recurse
    let mut accel = Vec2::zero();
    if let Some(children) = &node.children {
        for child in children {
            accel += compute_accel(particle_pos, child, theta);
        }
    }
    accel
}
```

For each particle, we walk the tree starting at the root. The traversal visits `O(log N)` nodes on average — total cost `O(N log N)`.

---

## What Stays N² (and Why)

- **Particle-particle bonds and collision** — these are inherently local, between bonded neighbors. Barnes-Hut doesn't help; the falling-sand swap is already O(neighbors) per cell.
- **Body-body gravity** — if you have ≤100 bodies, `100² = 10,000` ops/frame is nothing. Don't bother octree'ing this. Only body-PARTICLE goes through the tree.

---

## GPU Compute Question (Phase 3.5 or Phase 4)

Barnes-Hut is embarrassingly parallel: each particle's gravity is independent. The natural GPU work pattern:

1. CPU builds the quadtree from the current body list (small N, fast on CPU).
2. Upload tree to GPU as a buffer.
3. **Compute shader**: one thread per particle, walks the tree, writes acceleration to a buffer.
4. CPU reads back accelerations? No — keep the rest of the sim (velocity update, bond break, collision) on CPU and *don't* read back. Or push the whole sim to GPU. The latter is a much bigger refactor.

**Recommendation:** Phase 3 ships CPU Barnes-Hut. The GPU version is a Phase 4+ question, after we have body-class work on GPU and a clearer picture of the data flow.

**The trap to avoid:** moving the tree build to GPU. The tree is small (≤4000 nodes) and only changes when bodies move. CPU is fine. GPU would mean synchronizing with the renderer every frame, which wgpu's design tries to avoid.

---

## Tuning Knobs

| Parameter | Default | Trade-off |
|---|---|---|
| `θ` (theta) | 0.5 | Lower = more accurate, slower. Higher = faster, less accurate. Below 0.3 the orbits visibly drift; above 1.0 the simulation gets weird. |
| Tree rebuild | every frame | Bodies move, tree goes stale. If perf demands, rebuild only when a body has moved >1 cell. |
| Body softening | `ε = 0.5` (grid cells) | Same as Phase 2. The tree inherits this — internal node CoM calculations use the same softened inverse-square. |

---

## Phase 3 Acceptance Criteria

When Phase 3 lands, the user can:

1. **Run 100,000 particles + 50 bodies at 30+ fps on the gaming rig**, 15+ fps on m5.
2. **See the same orbits as Phase 2 N²** — visually indistinguishable for stable two-body and three-body systems. A unit test asserts orbital period is within 5% of the N² reference.
3. **Tune θ and see the speedup** — a CLI flag `--theta 0.3` (more accurate) vs `--theta 1.0` (faster), with a perf log.
4. **Stress test:** place 1000 random bodies and 100k random particles, simulation runs at 30+ fps on dGPU without visible artifacts.

If any of those doesn't work, Phase 3 isn't done.

---

## What's NOT in Phase 3

- **GPU compute** — Phase 4+. See "GPU Compute Question" above.
- **Multipole expansion** — the standard Barnes-Hut extension uses quadrupole moments for higher accuracy. We don't need it. Single-pole (center of mass) is good enough for θ=0.5.
- **Hierarchical O(N) (HOT)** — a more advanced algorithm. Barnes-Hut gets us 10x; HOT would get us 100x. Not needed.

---

## Open Questions

- **θ=0.5 default is right for stable orbits but may visibly distort close encounters.** Phase 2 testing will tell us. If the user notices, we add an adaptive θ (smaller when bodies are close, larger when far).
- **Barnes-Hut + bond stress interaction:** if a particle is part of a planet, the planet is *also* a body. Does the planet exert gravity on its own particles? Probably yes (and that's what makes "the planet's own gravity holds itself together"), but we need to test that the math doesn't blow up.
