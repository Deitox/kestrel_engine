use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::{ecs::EcsWorld, renderer::Renderer};

/// Describes the current runtime execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    /// Editor-only state; the simulation is not running.
    Editing,
    /// Runtime play mode. `paused` distinguishes between live play and paused single-step.
    Playing { paused: bool },
}

/// Narrow interface exposed by the runtime so Studio can drive scenes without depending on App internals.
pub trait RuntimeHost {
    /// Current play/edit mode.
    fn play_state(&self) -> PlayState;

    /// Enter play mode (unpaused).
    fn enter_play_mode(&mut self);

    /// Exit play mode and return to editing.
    fn exit_play_mode(&mut self);

    /// Pause the active play session.
    fn pause_play_mode(&mut self);

    /// Resume play after a pause.
    fn resume_play_mode(&mut self);

    /// Step a single frame while paused.
    fn step_frame(&mut self) -> Result<()>;

    /// Update the active scene path tracked by the host.
    fn set_scene_path(&mut self, path: PathBuf);

    /// Current scene path, if one has been assigned.
    fn scene_path(&self) -> Option<PathBuf>;

    /// Load a scene from disk, wiring dependencies as needed.
    fn load_scene(&mut self, path: &Path) -> Result<()>;

    /// Save the current scene to disk.
    fn save_scene(&mut self, path: &Path) -> Result<()>;

    /// Mutable access to the renderer so the caller can drive the viewport.
    fn renderer(&mut self) -> &mut Renderer;

    /// Immutable ECS access for inspection.
    fn ecs(&self) -> &EcsWorld;

    /// Mutable ECS access for editor mutations.
    fn ecs_mut(&mut self) -> &mut EcsWorld;
}
