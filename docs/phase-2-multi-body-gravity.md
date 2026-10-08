# Phase 2 — Multi-Body Gravity

> **Status:** spec draft, not yet implemented. Phase 1 must land first (single black hole + planet disintegration working). This document is what T3 Code reads when Phase 1 ships.

---

## What Phase 2 Adds

Phase 1 had **one** gravitational source — a single black hole at grid center. Phase 2 introduces **N bodies**, each with mass and velocity, mutually attracting each other AND every particle in the grid.

This unlocks:
- Real Kepler orbits around a black hole (not just "everything falls toward center")
- Stable Lagrange points between two massive bodies
- Slingshot maneuvers (a body can *gain* energy by passing a massive one)
- Three-body chaos when you have two roughly equal-mass bodies
- Most importantly: **multi-body tidal forces**. A particle near body A feels A strongly and B weakly; the *difference* is what rips the planet apart. This is where the "rip apart particle by particle" effect gets *cinematic* — a planet caught between two black holes disintegrates differently than one caught in a single hole.

---

## Body Storage

Bodies are first-class objects. Not emergent clusters of particles — explicit, named, with mass and velocity.

```rust
struct Body {
    id: u32,
    position: Vec2,        // grid coords (sub-cell: f32)
    velocity: Vec2,        // grid cells per second
    mass: f32,             // mass units (1 mass unit = N particles? TBD)
    radius: f32,           // for collision; body is treated as a circle at this radius
    kind: BodyKind,        // BlackHole | Star | Planet
    particle_budget: u32,  // how many particles this body currently owns (mutates)
}

enum BodyKind {
    BlackHole,  // destroys particles that cross event horizon
    Star,       // emits heat? (Phase 4+)
    Planet,     // cluster of bonded particles; this IS the body
}
```

`Vec2` is `f32` for body positions, but the *particle grid* stays integer. Bodies exist at sub-cell resolution so orbits look smooth; particles snap to grid cells. We do the conversion: every frame, for each body, the body's `position` is rounded to a grid cell, and any particles within `radius` are "owned" by that body for physics purposes (and rendered as that body's material/color).

**Key invariant:** a Planet body's mass equals the number of particles it currently owns × per-particle-mass. As bonds break and particles fly off, **mass decreases in real time**. This is the visual signature: a planet's orbit decays as it loses mass to the black hole. Eventually it's just a debris stream.

---

## Gravity Math (N² direct sum, Phase 2)

For each particle in the grid, sum the gravitational pull from every body:

```rust
for particle in particles.iter() {
    let mut accel = Vec2::zero();
    for body in &bodies {
        let r = body.position - particle.position;
        let dist_sq = r.length_squared() + EPSILON;  // softening to avoid singularity
        let dist = dist_sq.sqrt();
        accel += G * body.mass * r / (dist_sq * dist);  // F = G*m1*m2/r², divided by m1
    }
    particle.velocity += accel * dt;
}
```

**Softening (`EPSILON`):** without it, a particle that gets arbitrarily close to a body gets infinite acceleration, which is numerically catastrophic. We use a Plummer-style softening: replace `r²` with `r² + ε²`. This gives stable orbits inside `ε` of the body. Pick `ε = 0.5` (half a cell) initially, tune later.

**Big-O:** O(particles × bodies). For Phase 2 with ≤10 bodies and 50k particles, that's 500k ops/frame — trivial on CPU. Body-body gravity is a separate small loop (10×10 = 100 ops/frame).

**The expensive case:** body-particle with hundreds of bodies or 100k+ particles. That's Phase 3 (octree).

---

## Event Horizon & Particle Destruction

When a particle crosses a BlackHole body's event horizon (distance < `event_horizon_radius`), the particle is **destroyed**:
1. The cell becomes empty (`particles[i] = Vacuum`).
2. All bonds to neighbors are cleared.
3. The body that owns the particle (if any) loses a particle from its count, so its mass decreases.

**Visual signature:** a body falling into a black hole doesn't just collide — it gets *stripped*. The near side gets pulled in first, the far side stretches, bonds break under the tidal stress, and you watch a planet dissolve into a stream of particles, each one disappearing as it crosses the horizon. The horizon itself is just a circle of "vacuum" — we don't need fancy rendering for it to read.

**Why this is a Phase 2 design decision, not Phase 4:** the moment we have N bodies with mass, the question of "what happens when a particle is consumed" comes up. We have to answer it now or the engine is incomplete.

---

## Body-Body Gravity (separate from body-particle)

Bodies attract each other. This is what makes orbits work.

```rust
for (i, a) in bodies.iter().enumerate() {
    for b in &bodies[i+1..] {
        let r = b.position - a.position;
        let dist_sq = r.length_squared() + EPSILON;
        let dist = dist_sq.sqrt();
        let force = G * a.mass * b.mass / dist_sq;
        a.velocity += force * r / dist * dt / a.mass;  // a = F/m
        b.velocity -= force * r / dist * dt / b.mass;
    }
}
```

Two-body problem (Star + Planet, no black hole) is stable. Three-body (Star + Planet + BlackHole) is chaotic and *interesting* — that's the gameplay.

---

## Verlet Integration (for bodies)

For body motion, use **velocity Verlet**, not Euler. Verlet is symplectic, which means energy is approximately conserved over long simulations. Euler accumulates energy error and orbits spiral.

```rust
fn verlet_step(body: &mut Body, dt: f32, accel: Vec2) {
    let new_pos = body.position + body.velocity * dt + accel * 0.5 * dt * dt;
    let new_accel = compute_accel_at(new_pos);
    let new_vel = body.velocity + (accel + new_accel) * 0.5 * dt;
    body.position = new_pos;
    body.velocity = new_vel;
    // accel is recomputed next frame
}
```

For particles, Euler is fine — they don't need long-term orbital stability, they're on grid cells and snap.

---

## Phase 2 Acceptance Criteria

When Phase 2 lands, the user can:

1. **Place 2+ bodies in the world** with arbitrary mass, position, and velocity.
2. **Place a planet near a black hole** and watch it orbit (not just fall in) if given tangential velocity.
3. **Slingshot a planet past a black hole** and watch it gain energy and fly off.
4. **Place a planet between two black holes** and watch it get *stretched and torn apart* between them — the visual signature of multi-body tidal forces.
5. **See a planet's orbit decay** as it loses particles to the black hole, eventually becoming a debris stream.
6. **Performance:** 10 bodies + 50k particles at 30+ fps on the gaming rig, 15+ fps on m5.

If any of those doesn't work, Phase 2 isn't done.

---

## What's NOT in Phase 2 (deferred)

- **Barnes-Hut octree** — Phase 3, when body/particle count gets high enough that N² is the bottleneck.
- **Body-body collision** (two planets crashing into each other) — Phase 4. Phase 2 treats bodies as point masses.
- **Particle emission from bodies** (stars emitting plasma) — Phase 4.
- **First-class body types beyond BlackHole and Planet** — Phase 4. Phase 2 only has those two.
- **Save/load** — not in any phase yet.

---

## Open Questions

- **Mass unit:** what does `mass = 1.0` mean? Most natural is "1 mass unit = 1 particle of Rock." But that makes planets heavier than stars unless we scale. Probably need a `mass_to_particles` ratio per body type.
- **How do users place bodies?** Phase 1: world starts with one black hole, hardcoded. Phase 2 needs at least a way to spawn bodies at runtime. CLI args? Keypress? Dev console? Defer to Phase 4 if not blocking.
- **Do bodies have a "max radius" for the particle cluster, or can a planet grow indefinitely as it accretes mass?** Not in scope for Phase 2 (no accretion yet).
