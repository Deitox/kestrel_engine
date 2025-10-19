# Decision Log

| Area | Choice | Rationale |
|------|--------|-----------|
| Windowing | `winit` 0.30 | Modern event loop with async-friendly `ApplicationHandler` API and cross-platform support. |
| GPU backend | `wgpu` 27 | Cross-platform Vulkan/Metal/DX12 abstraction with WebGPU compatibility and solid Rust ecosystem. |
| ECS | `bevy_ecs` 0.15 | Provides performant ECS core without requiring the full Bevy engine; aligns with data-driven design. |
| Math | `glam` 0.27 | SIMD-accelerated vector/matrix types with straightforward `bytemuck` integration for GPU uploads. |
| UI | `egui` 0.33 + `egui-winit/wgpu` | Immediate-mode UI suits internal tooling and integrates cleanly with WGPU for debug overlays. |
| Assets | `image` crate | Supports PNG decoding without heavy dependencies; works with atlas JSON pipeline. |
| Serialization | `serde` + `serde_json` | Flexible data formats for config/atlas files; broad ecosystem support. |
| Error handling | `anyhow` | Simplified error propagation with contextual messages during initialization and asset loading. |
| Randomness | `rand` 0.8 | Lightweight RNG for spawning demo entities and gameplay experimentation. |
| Scripting | `rhai` 1.23 | Lightweight, hot-reloadable scripting with an ergonomic Rust API surface and no VM build step. |

### Guiding Principles
- Prefer deterministic, data-driven flows with explicit resource ownership.
- Keep subsystems modular so renderer, ECS, and tooling evolve independently.
- Fail gracefully: surface configuration or asset errors to the console without panicking.
- Optimize hot loops (rendering, physics) while keeping the API ergonomic.
- Allow live iteration via scripting and data reloading without restarting the app.
