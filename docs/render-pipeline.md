# Render Pipeline

> **Audience:** anyone writing or reviewing the GPU side of the engine. T3 Code reads this when implementing Phase 1's render pass and Phase 2+ body rendering.

---

## What Gets Sent to the GPU

**Everything in one texture, every frame.** That's the whole design.

The sim lives in CPU memory as a SoA layout (see `docs/SPEC.md` § State). At the start of each render pass, we copy the `particles[]` array (one byte per cell: the material enum) into an `R8Unorm` texture. The shader reads that texture as a color palette lookup, looks up the material's color, and writes to the framebuffer.

No scene graph. No draw calls per particle. One draw call: full-screen quad sampling the texture.

---

## Texture Format Choices

| Data | Format | Why |
|---|---|---|
| Material per cell | `R8Unorm` (1 byte) | Smallest possible. 256×256 grid = 64 KB. 1024×1024 = 1 MB. Fits in any GPU's memory. |
| Render target | `Bgra8Unorm` (default swapchain) | Standard. No reason to deviate. |
| Body positions (Phase 2+) | small `Storage` buffer | ≤100 bodies, updated per frame. |

We're not using `Rgba8` for the sim texture because that wastes 75% of the bandwidth and the material fits in one byte.

---

## The Render Pass (per frame)

```
┌─────────────────────────────────────────────────────────────┐
│ Frame N                                                     │
│                                                             │
│ 1. CPU sim (gravity, bond, collision) — updates particles[] │
│ 2. queue.write_texture(particle_texture, &particles, ...)   │
│ 3. Begin render pass to swapchain                           │
│ 4. Set pipeline + bind particle_texture + sampler            │
│ 5. Draw 3 vertices (full-screen triangle)                   │
│ 6. Submit                                                   │
└─────────────────────────────────────────────────────────────┘
```

Total GPU work: one texture upload, one triangle draw. The whole point of this design is to keep the GPU so underused that we never have to think about it again.

**Full-screen triangle, not quad:** one vertex shader, no overdraw on the diagonal, slightly faster. Trivial to write.

---

## Material → Color Palette

The material enum has, say, 5 values: Vacuum, Rock, Ice, Plasma, Gas. We bake a 256-entry color lookup into a small `Rgba8Unorm` texture at startup. The fragment shader does:

```wgsl
let material_byte = textureSample(sim_tex, samp, uv).r;
let color = textureLoad(palette_tex, vec2<i32>(i32(material_byte * 255.0), 0)).rgba;
```

255 entry slots is overkill but it's free. Future materials (Phase 4: lava, metal, exotic matter) just add entries to the palette.

**Why not just `if material == Rock { red } else if material == Ice { blue }` in the shader:** branches are bad on GPUs, especially in a fragment shader running once per pixel. A texture lookup is one instruction.

---

## Zoom and Pan (Phase 2+)

The grid is a fixed resolution (e.g. 512×512), but the *window* can be any size. The fragment shader's `uv` is mapped to the grid cell by `uv * (grid_size / window_size)` or similar — when the window is bigger, you see more cells; when smaller, you see fewer.

For zoom (Phase 2+): a uniform `zoom: f32` and `pan: Vec2` in the shader. `uv = (screen_uv - 0.5) / zoom + pan;` — values >1.0 zoom out (more cells visible), <1.0 zoom in (fewer cells, larger). Pan shifts the center.

This means the grid is the world. There's no camera; you just choose which part of the grid to look at.

---

## Phase 2+ Additions

When bodies arrive, we have a choice:

**Option A: bodies are part of the grid.** The body is rendered as a circle of cells with the body's color. No separate rendering pass. This is the cheap path.

**Option B: bodies are a separate render layer.** Draw a small `<= 100` quads/circles on top of the grid, blended. This costs a second draw call but allows bodies to have outlines, glow, event-horizon rings, etc. — visual polish.

**Pick for Phase 2:** Option A. Bodies are part of the grid, just a different material/color. We can graduate to Option B in Phase 4 when we have visual polish needs.

For the **black hole event horizon**: it's a circle of `Vacuum` cells around the body's position. The body itself can be a few `Vacuum` cells too (a black hole "is" the absence of matter at its location). No special rendering needed.

---

## Performance Notes

- The texture upload is `grid_w * grid_h` bytes per frame. At 60 fps and 1024×1024 grid, that's 64 MB/s. Trivial bandwidth.
- The fragment shader runs once per window pixel. At 1080p (2M pixels) and 60 fps, that's 120M shader invocations/sec. Each does one texture sample + one palette lookup. Modern GPUs do billions of these.
- **CPU bottleneck, not GPU.** The sim is 100% CPU. The render is "free" by comparison.

This is the whole point of the architecture: keep the expensive work on the CPU where we have full control over the data layout, and let the GPU do the one thing it's good at (texturing a quad).

---

## Common Pitfalls

- **Don't try to render every particle as a separate sprite.** A grid of 1024×1024 = 1M particles would be 1M draw calls. Even with instancing, you're hammering the GPU for nothing.
- **Don't use a separate `Rgba8` texture for the sim data.** We only have 5–10 materials; one byte is enough.
- **Don't sample the sim texture with `Nearest` filtering for a sub-cell-accurate look.** It will look pixelated. `Linear` filtering does a fast bilinear blend, gives a smoother look at no cost. (Caveat: Linear can blur the material boundaries in a way that looks weird. We can revisit in Phase 2 when we have more materials.)
- **Don't recreate the pipeline every frame.** Build it once at startup, reuse it. wgpu's pipeline creation is expensive (shader compilation).
- **Don't forget to handle window resize.** The swapchain is tied to the window size; on resize, recreate the swapchain. winit's `WindowEvent::Resized` is the trigger.

---

## Reference: Phase 1 Shader Skeleton

```wgsl
struct VertexOutput {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    // Full-screen triangle
    let uv = vec2<f32>(f32((idx << 1) & 2), f32(idx & 2));
    var out: VertexOutput;
    out.pos = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

@group(0) @binding(0) var sim_tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var palette_tex: texture_2d<f32>;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let m = textureSample(sim_tex, samp, in.uv).r;
    let color = textureLoad(palette_tex, vec2<i32>(i32(m * 255.0), 0));
    return color;
}
```

This is the entire render pass. About 30 lines of WGSL. T3 Code fills in the Rust host code that creates the textures, palette, pipeline, and bind group.
