#[cfg(feature = "alloc_profiler")]
pub mod alloc_profiler;
pub mod analytics;
pub mod animation_validation;
pub mod app;
pub mod assets;
pub mod audio;
pub mod camera;
pub mod camera3d;
pub mod cli;
pub mod config;
pub mod ecs;
pub mod environment;
pub mod events;
pub(crate) mod gizmo;
pub mod gpu_baseline;
pub mod input;
pub mod material_registry;
pub mod mesh;
pub(crate) mod mesh_preview;
pub mod mesh_registry;
pub mod plugin_rpc;
pub mod plugins;
pub mod prefab;
pub mod renderer;
pub mod scene;
pub mod scene_capture;
pub mod scripts;
pub mod time;

#[cfg(feature = "alloc_profiler")]
#[global_allocator]
static GLOBAL_ALLOCATOR: alloc_profiler::TrackingAllocator = alloc_profiler::TrackingAllocator;

pub use app::{run, run_with_overrides, App};

pub(crate) fn wrap_angle(mut radians: f32) -> f32 {
    let two_pi = 2.0 * std::f32::consts::PI;
    while radians > std::f32::consts::PI {
        radians -= two_pi;
    }
    while radians < -std::f32::consts::PI {
        radians += two_pi;
    }
    radians
}
