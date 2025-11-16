use egui::Context as EguiCtx;
use egui_wgpu::{Renderer as EguiRenderer, ScreenDescriptor};
use egui_winit::State as EguiWinit;

pub(crate) struct EditorShell {
    pub egui_ctx: EguiCtx,
    pub egui_winit: Option<EguiWinit>,
    pub egui_renderer: Option<EguiRenderer>,
    pub egui_screen: Option<ScreenDescriptor>,
}

impl EditorShell {
    pub fn new() -> Self {
        Self { egui_ctx: EguiCtx::default(), egui_winit: None, egui_renderer: None, egui_screen: None }
    }
}
