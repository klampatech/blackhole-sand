# Blackhole Sand

A 2D particle-physics engine for space, where celestial bodies are made of
particles and a black hole's tidal forces can rip them apart particle by
particle.

> **Status:** Phase 0 — design locked, repo bootstrap. No engine code yet.
> See [`docs/SPEC.md`](docs/SPEC.md) for the phased plan.

## Building

Requires Rust 1.74+ and a Vulkan/Metal/DX12-capable GPU (wgpu handles the
backend selection).

```bash
cargo run
```

## Repo layout

- `docs/SPEC.md` — canonical design spec. Edit on a branch + PR.
- `SPEC.md` — stub pointer to `docs/SPEC.md`.
- `HANDOFF.md` — session-to-session continuity log.
- `src/` — engine code (Phase 1+).
- `.github/workflows/ci.yml` — CI guard for spec layout.

## License

TBD.
