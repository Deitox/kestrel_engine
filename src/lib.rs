pub mod analytics;
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
pub mod input;
pub mod material_registry;
pub mod mesh;
pub(crate) mod mesh_preview;
pub mod mesh_registry;
pub mod plugins;
pub mod renderer;
pub mod scene;
pub mod scripts;
pub mod time;

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
