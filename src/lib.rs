pub mod assets;
pub mod audio;
pub mod camera;
pub mod config;
pub mod ecs;
pub mod events;
pub mod input;
pub mod renderer;
pub mod scripts;
pub mod time;

use crate::assets::AssetManager;
use crate::audio::AudioManager;
use crate::camera::Camera2D;
use crate::config::AppConfig;
use crate::ecs::EcsWorld;
use crate::events::GameEvent;
use crate::input::{Input, InputEvent};
use crate::renderer::Renderer;
use crate::scripts::{ScriptCommand, ScriptHost};
use crate::time::Time;

use bevy_ecs::prelude::Entity;
use glam::{Vec2, Vec4};

use anyhow::{Context, Result};
use std::collections::VecDeque;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};

// egui
use egui::Context as EguiCtx;
use egui_plot as eplot;
use egui_wgpu::{Renderer as EguiRenderer, RendererOptions, ScreenDescriptor};
use egui_winit::State as EguiWinit;

const CAMERA_BASE_HALF_HEIGHT: f32 = 1.2;

pub async fn run() -> Result<()> {
    let config = AppConfig::load_or_default("config/app.json");
    let event_loop = EventLoop::new().context("Failed to create winit event loop")?;
    let mut app = App::new(config).await;
    event_loop.run_app(&mut app).context("Event loop execution failed")?;
    Ok(())
}

pub struct App {
    renderer: Renderer,
    ecs: EcsWorld,
    time: Time,
    input: Input,
    assets: AssetManager,
    should_close: bool,
    accumulator: f32,
    fixed_dt: f32,

    // egui
    egui_ctx: EguiCtx,
    egui_winit: Option<EguiWinit>,
    egui_renderer: Option<EguiRenderer>,
    egui_screen: Option<ScreenDescriptor>,

    // UI State
    ui_spawn_per_press: i32,
    ui_auto_spawn_rate: f32, // per second
    ui_cell_size: f32,
    ui_hist: Vec<f32>,
    ui_root_spin: f32,
    ui_emitter_rate: f32,
    ui_emitter_spread: f32,
    ui_emitter_speed: f32,
    ui_emitter_lifetime: f32,
    ui_emitter_start_size: f32,
    ui_emitter_end_size: f32,
    ui_emitter_start_color: [f32; 4],
    ui_emitter_end_color: [f32; 4],

    // Audio
    audio: AudioManager,

    // Events
    recent_events: VecDeque<GameEvent>,
    event_log_limit: usize,

    // Camera / selection
    camera: Camera2D,
    selected_entity: Option<Entity>,

    // Configuration
    config: AppConfig,

    // Particles
    emitter_entity: Option<Entity>,

    // Scripting
    scripts: ScriptHost,
}

impl App {
    pub async fn new(config: AppConfig) -> Self {
        let renderer = Renderer::new(&config.window).await;
        let mut ecs = EcsWorld::new();
        let emitter = ecs.spawn_demo_scene();
        let mut audio = AudioManager::new(16);
        let event_log_limit = 32;
        let mut recent_events = VecDeque::with_capacity(event_log_limit);
        for event in ecs.drain_events() {
            if recent_events.len() == event_log_limit {
                recent_events.pop_front();
            }
            audio.handle_event(&event);
            recent_events.push_back(event);
        }
        let time = Time::new();
        let input = Input::new();
        let assets = AssetManager::new();

        // egui context and state
        let egui_ctx = EguiCtx::default();
        let egui_winit = None;
        let scripts = ScriptHost::new("assets/scripts/main.rhai");

        Self {
            renderer,
            ecs,
            time,
            input,
            assets,
            should_close: false,
            accumulator: 0.0,
            fixed_dt: 1.0 / 60.0,
            egui_ctx,
            egui_winit,
            egui_renderer: None,
            egui_screen: None,
            ui_spawn_per_press: 200,
            ui_auto_spawn_rate: 0.0,
            ui_cell_size: 0.25,
            ui_hist: Vec::with_capacity(240),
            ui_root_spin: 1.2,
            ui_emitter_rate: 35.0,
            ui_emitter_spread: std::f32::consts::PI / 3.0,
            ui_emitter_speed: 0.8,
            ui_emitter_lifetime: 1.2,
            ui_emitter_start_size: 0.18,
            ui_emitter_end_size: 0.05,
            ui_emitter_start_color: [1.0, 0.8, 0.2, 0.8],
            ui_emitter_end_color: [1.0, 0.2, 0.2, 0.0],
            audio,
            recent_events,
            event_log_limit,
            camera: Camera2D::new(CAMERA_BASE_HALF_HEIGHT),
            selected_entity: None,
            config,
            emitter_entity: Some(emitter),
            scripts,
        }
    }

    fn record_events(&mut self) {
        let events = self.ecs.drain_events();
        if events.is_empty() {
            return;
        }
        for event in events {
            self.audio.handle_event(&event);
            if self.recent_events.len() == self.event_log_limit {
                self.recent_events.pop_front();
            }
            self.recent_events.push_back(event);
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Err(err) = self.renderer.ensure_window(event_loop) {
            eprintln!("Renderer initialization error: {err:?}");
            self.should_close = true;
            return;
        }
        let (device, queue) = match self.renderer.device_and_queue() {
            Ok(pair) => pair,
            Err(err) => {
                eprintln!("Renderer missing device/queue: {err:?}");
                self.should_close = true;
                return;
            }
        };
        self.assets.set_device(device, queue);
        if let Err(err) = self.assets.load_atlas("main", "assets/images/atlas.json") {
            eprintln!("Failed to load atlas: {err:?}");
            self.should_close = true;
            return;
        }
        let atlas_view = match self.assets.atlas_texture_view("main") {
            Ok(view) => view,
            Err(err) => {
                eprintln!("Failed to create atlas texture view: {err:?}");
                self.should_close = true;
                return;
            }
        };
        let sampler = self.assets.default_sampler().clone();
        if let Err(err) = self.renderer.init_sprite_pipeline_with_atlas(atlas_view, sampler) {
            eprintln!("Failed to initialize sprite pipeline: {err:?}");
            self.should_close = true;
            return;
        }

        if self.egui_winit.is_none() {
            if let Some(window) = self.renderer.window() {
                let state = EguiWinit::new(
                    self.egui_ctx.clone(),
                    egui::ViewportId::ROOT,
                    window,
                    Some(self.renderer.pixels_per_point()),
                    window.theme(),
                    None,
                );
                self.egui_winit = Some(state);
            }
        }

        // egui painter
        let egui_renderer = match (self.renderer.device(), self.renderer.surface_format()) {
            (Ok(device), Ok(format)) => EguiRenderer::new(device, format, RendererOptions::default()),
            (Err(err), _) | (_, Err(err)) => {
                eprintln!("Unable to initialize egui renderer: {err:?}");
                self.should_close = true;
                return;
            }
        };
        self.egui_renderer = Some(egui_renderer);
        let size = self.renderer.size();
        self.egui_screen = Some(ScreenDescriptor {
            size_in_pixels: [size.width, size.height],
            pixels_per_point: self.renderer.pixels_per_point(),
        });
    }

    fn window_event(&mut self, _el: &ActiveEventLoop, id: winit::window::WindowId, event: WindowEvent) {
        // egui wants the events too
        let mut consumed = false;
        if let (Some(window), Some(state)) = (self.renderer.window(), self.egui_winit.as_mut()) {
            if id == window.id() {
                let resp = state.on_window_event(window, &event);
                if resp.consumed {
                    consumed = true;
                }
            }
        }
        let input_event = InputEvent::from_window_event(&event);
        self.input.push(input_event);

        if consumed {
            return;
        }

        match &event {
            WindowEvent::CloseRequested => self.should_close = true,
            WindowEvent::Resized(size) => {
                self.renderer.resize(*size);
                if let Some(sd) = &mut self.egui_screen {
                    sd.size_in_pixels = [size.width, size.height];
                    sd.pixels_per_point = self.renderer.pixels_per_point();
                }
            }
            WindowEvent::KeyboardInput { event: KeyEvent { logical_key, state, .. }, .. } => {
                if let Key::Named(NamedKey::Escape) = logical_key {
                    if *state == ElementState::Pressed {
                        self.should_close = true;
                    }
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _e: &ActiveEventLoop, _dev: winit::event::DeviceId, ev: DeviceEvent) {
        self.input.push(InputEvent::from_device_event(&ev));
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.should_close {
            event_loop.exit();
            return;
        }
        self.time.tick();
        let dt = self.time.delta_seconds();
        self.accumulator += dt;

        if let Some(entity) = self.selected_entity {
            if !self.ecs.entity_exists(entity) {
                self.selected_entity = None;
            }
        }

        if self.ui_auto_spawn_rate > 0.0 {
            let to_spawn = (self.ui_auto_spawn_rate * dt) as i32;
            if to_spawn > 0 {
                self.ecs.spawn_burst(&self.assets, to_spawn as usize);
            }
        }

        if self.input.take_space_pressed() {
            self.ecs.spawn_burst(&self.assets, self.ui_spawn_per_press as usize);
        }
        if self.input.take_b_pressed() {
            self.ecs.spawn_burst(&self.assets, (self.ui_spawn_per_press * 5).max(1000) as usize);
        }

        let window_size = self.renderer.size();

        if let Some(delta) = self.input.consume_wheel_delta() {
            self.camera.apply_scroll_zoom(delta);
        }

        if self.input.right_held() {
            let (dx, dy) = self.input.mouse_delta;
            if dx.abs() > f32::EPSILON || dy.abs() > f32::EPSILON {
                self.camera.pan_screen_delta(Vec2::new(dx, dy), window_size);
            }
        }

        let view_proj = self.camera.view_projection(window_size);

        if self.input.take_left_click() {
            if let Some((sx, sy)) = self.input.cursor_position() {
                if let Some(world) = self.camera.screen_to_world(Vec2::new(sx, sy), window_size) {
                    self.selected_entity = self.ecs.pick_entity(world);
                } else {
                    self.selected_entity = None;
                }
            }
        }

        self.ecs.set_spatial_cell(self.ui_cell_size.max(0.05));
        if let Some(emitter) = self.emitter_entity {
            self.ecs.set_emitter_rate(emitter, self.ui_emitter_rate);
            self.ecs.set_emitter_spread(emitter, self.ui_emitter_spread);
            self.ecs.set_emitter_speed(emitter, self.ui_emitter_speed);
            self.ecs.set_emitter_lifetime(emitter, self.ui_emitter_lifetime);
            self.ecs.set_emitter_colors(
                emitter,
                Vec4::from_array(self.ui_emitter_start_color),
                Vec4::from_array(self.ui_emitter_end_color),
            );
            self.ecs.set_emitter_sizes(emitter, self.ui_emitter_start_size, self.ui_emitter_end_size);
        }
        self.scripts.update(dt);
        let commands = self.scripts.drain_commands();
        self.apply_script_commands(commands);
        for message in self.scripts.drain_logs() {
            self.ecs.push_event(GameEvent::ScriptMessage { message });
        }

        while self.accumulator >= self.fixed_dt {
            self.ecs.fixed_step(self.fixed_dt);
            self.accumulator -= self.fixed_dt;
        }
        self.ecs.update(dt);
        self.record_events();

        let (instances, _atlas) = match self.ecs.collect_sprite_instances(&self.assets) {
            Ok(data) => data,
            Err(err) => {
                eprintln!("Instance collection error: {err:?}");
                self.input.clear_frame();
                return;
            }
        };
        if let Err(err) = self.renderer.render_batch(&instances, view_proj) {
            eprintln!("Render error: {err:?}");
        }

        if self.egui_winit.is_none() {
            return;
        }
        let pixels_per_point = self.renderer.pixels_per_point();

        let raw_input = {
            let Some(window) = self.renderer.window() else {
                return;
            };
            self.egui_winit.as_mut().unwrap().take_egui_input(window)
        };
        let dt_ms = dt * 1000.0;
        self.ui_hist.push(dt_ms);
        if self.ui_hist.len() > 240 {
            self.ui_hist.remove(0);
        }

        let hist_points: Vec<[f64; 2]> =
            self.ui_hist.iter().enumerate().map(|(i, v)| [i as f64, *v as f64]).collect();
        let entity_count = self.ecs.entity_count();
        let instances_drawn = instances.len();
        let mut ui_cell_size = self.ui_cell_size;
        let mut ui_spawn_per_press = self.ui_spawn_per_press;
        let mut ui_auto_spawn_rate = self.ui_auto_spawn_rate;
        let mut ui_root_spin = self.ui_root_spin;
        let mut ui_emitter_rate = self.ui_emitter_rate;
        let mut ui_emitter_spread = self.ui_emitter_spread;
        let mut ui_emitter_speed = self.ui_emitter_speed;
        let mut ui_emitter_lifetime = self.ui_emitter_lifetime;
        let mut ui_emitter_start_size = self.ui_emitter_start_size;
        let mut ui_emitter_end_size = self.ui_emitter_end_size;
        let mut ui_emitter_start_color = self.ui_emitter_start_color;
        let mut ui_emitter_end_color = self.ui_emitter_end_color;
        let mut selected_entity = self.selected_entity;
        let mut selection_details = selected_entity.and_then(|entity| self.ecs.entity_info(entity));
        let cursor_world = self
            .input
            .cursor_position()
            .and_then(|(sx, sy)| self.camera.screen_to_world(Vec2::new(sx, sy), window_size));
        let mut highlight_rect = None;
        let mut gizmo_center_px = None;
        let camera_position = self.camera.position;
        let camera_zoom = self.camera.zoom;
        let recent_events: Vec<GameEvent> = self.recent_events.iter().cloned().collect();
        let audio_triggers: Vec<String> = self.audio.recent_triggers().cloned().collect();
        let mut audio_enabled = self.audio.enabled();

        if let Some(entity) = selected_entity {
            if let Some((min, max)) = self.ecs.entity_bounds(entity) {
                if let Some((min_px, max_px)) = self.camera.world_rect_to_screen_bounds(min, max, window_size)
                {
                    highlight_rect = Some(egui::Rect::from_two_pos(
                        egui::pos2(min_px.x / pixels_per_point, min_px.y / pixels_per_point),
                        egui::pos2(max_px.x / pixels_per_point, max_px.y / pixels_per_point),
                    ));
                    gizmo_center_px = Some((min_px + max_px) * 0.5);
                }
            }
        }

        #[derive(Default)]
        struct UiActions {
            spawn_now: bool,
            delete_entity: Option<Entity>,
            clear_particles: bool,
            reset_world: bool,
        }
        let mut actions = UiActions::default();

        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            egui::Window::new("Kestrel Debug").resizable(true).show(ctx, |ui| {
                ui.label(format!("Entities: {}", entity_count));
                ui.label(format!("Instances drawn: {}", instances_drawn));
                ui.separator();
                ui.add(egui::Slider::new(&mut ui_cell_size, 0.05..=0.8).text("Spatial cell size"));
                ui.add(egui::Slider::new(&mut ui_spawn_per_press, 1..=5000).text("Spawn per press"));
                ui.add(
                    egui::Slider::new(&mut ui_auto_spawn_rate, 0.0..=5000.0).text("Auto-spawn per second"),
                );
                ui.add(
                    egui::Slider::new(&mut ui_emitter_rate, 0.0..=200.0).text("Emitter rate (particles/s)"),
                );
                ui.add(
                    egui::Slider::new(&mut ui_emitter_spread, 0.0..=std::f32::consts::PI)
                        .text("Emitter spread (rad)"),
                );
                ui.add(egui::Slider::new(&mut ui_emitter_speed, 0.0..=3.0).text("Emitter speed"));
                ui.add(egui::Slider::new(&mut ui_emitter_lifetime, 0.1..=5.0).text("Particle lifetime (s)"));
                ui.add(egui::Slider::new(&mut ui_emitter_start_size, 0.01..=0.5).text("Particle start size"));
                ui.add(egui::Slider::new(&mut ui_emitter_end_size, 0.01..=0.5).text("Particle end size"));
                ui.horizontal(|ui| {
                    ui.label("Start color");
                    ui.color_edit_button_rgba_unmultiplied(&mut ui_emitter_start_color);
                });
                ui.horizontal(|ui| {
                    ui.label("End color");
                    ui.color_edit_button_rgba_unmultiplied(&mut ui_emitter_end_color);
                });
                ui.add(egui::Slider::new(&mut ui_root_spin, -5.0..=5.0).text("Root spin speed"));
                if ui.button("Spawn now").clicked() {
                    actions.spawn_now = true;
                }
                if ui.button("Clear particles").clicked() {
                    actions.clear_particles = true;
                }
                if ui.button("Reset world").clicked() {
                    actions.reset_world = true;
                }
                ui.separator();
                let hist = eplot::Plot::new("fps_plot").height(120.0).include_y(0.0).include_y(40.0);
                hist.show(ui, |plot_ui| {
                    plot_ui.line(eplot::Line::new("ms/frame", eplot::PlotPoints::from(hist_points.clone())));
                });
                ui.label("Target: 16.7ms for 60 FPS");
                ui.separator();
                ui.label(format!(
                    "Camera: pos({:.2}, {:.2}) zoom {:.2}",
                    camera_position.x, camera_position.y, camera_zoom
                ));
                let display_mode = if self.config.window.fullscreen { "Fullscreen" } else { "Windowed" };
                ui.label(format!(
                    "Display: {}x{} {}",
                    self.config.window.width, self.config.window.height, display_mode
                ));
                ui.label(format!("VSync: {}", if self.config.window.vsync { "On" } else { "Off" }));
                if let Some(cursor) = cursor_world {
                    ui.label(format!("Cursor world: ({:.2}, {:.2})", cursor.x, cursor.y));
                } else {
                    ui.label("Cursor world: n/a");
                }
                ui.separator();
                if let Some(entity) = selected_entity {
                    ui.label(format!("Selected: {:?}", entity));
                    if let Some(info) = selection_details.as_ref() {
                        ui.label(format!("Position: ({:.2}, {:.2})", info.translation.x, info.translation.y));
                        if let Some(vel) = info.velocity {
                            ui.label(format!("Velocity: ({:.2}, {:.2})", vel.x, vel.y));
                        } else {
                            ui.label("Velocity: n/a");
                        }
                        if let Some(region) = &info.sprite_region {
                            ui.label(format!("Sprite: {}", region));
                        }
                    } else {
                        ui.label("Selection data unavailable");
                    }
                    if ui.button("Delete selected").clicked() {
                        actions.delete_entity = Some(entity);
                        selected_entity = None;
                        selection_details = None;
                        highlight_rect = None;
                        gizmo_center_px = None;
                    }
                } else {
                    ui.label("No entity selected");
                }
                ui.separator();
                ui.heading("Scripts");
                ui.label(format!("Path: {}", self.scripts.script_path().display()));
                let mut scripts_enabled = self.scripts.enabled();
                if ui.checkbox(&mut scripts_enabled, "Enable scripts").changed() {
                    self.scripts.set_enabled(scripts_enabled);
                }
                if ui.button("Reload script").clicked() {
                    if let Err(err) = self.scripts.force_reload() {
                        self.scripts.set_error_message(err.to_string());
                    }
                }
                if let Some(err) = self.scripts.last_error() {
                    ui.colored_label(egui::Color32::RED, format!("Error: {err}"));
                } else if self.scripts.enabled() {
                    ui.label("Script running");
                } else {
                    ui.label("Scripts disabled");
                }
                ui.separator();
                ui.heading("Recent Events");
                if recent_events.is_empty() {
                    ui.label("No events recorded");
                } else {
                    for event in recent_events.iter().rev().take(10) {
                        ui.label(event.to_string());
                    }
                }
                ui.separator();
                ui.heading("Audio Debug");
                if ui.checkbox(&mut audio_enabled, "Enable audio triggers").changed() {
                    self.audio.set_enabled(audio_enabled);
                }
                if ui.button("Clear audio log").clicked() {
                    self.audio.clear();
                }
                if audio_triggers.is_empty() {
                    ui.label("No audio triggers");
                } else {
                    for trigger in audio_triggers.iter().rev() {
                        ui.label(trigger);
                    }
                }
            });

            let painter = ctx.debug_painter();
            if let Some(rect) = highlight_rect {
                painter.rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(2.0, egui::Color32::YELLOW),
                    egui::StrokeKind::Inside,
                );
            }
            if let Some(center_px) = gizmo_center_px {
                let center = egui::pos2(center_px.x / pixels_per_point, center_px.y / pixels_per_point);
                let extent = 8.0 / pixels_per_point;
                painter.line_segment(
                    [egui::pos2(center.x - extent, center.y), egui::pos2(center.x + extent, center.y)],
                    egui::Stroke::new(2.0, egui::Color32::YELLOW),
                );
                painter.line_segment(
                    [egui::pos2(center.x, center.y - extent), egui::pos2(center.x, center.y + extent)],
                    egui::Stroke::new(2.0, egui::Color32::YELLOW),
                );
                painter.circle_stroke(
                    center,
                    3.0 / pixels_per_point,
                    egui::Stroke::new(2.0, egui::Color32::YELLOW),
                );
            }
        });

        self.ui_cell_size = ui_cell_size;
        self.ui_spawn_per_press = ui_spawn_per_press;
        self.ui_auto_spawn_rate = ui_auto_spawn_rate;
        self.ui_root_spin = ui_root_spin;
        self.ui_emitter_rate = ui_emitter_rate;
        self.ui_emitter_spread = ui_emitter_spread;
        self.ui_emitter_speed = ui_emitter_speed;
        self.ui_emitter_lifetime = ui_emitter_lifetime;
        self.ui_emitter_start_size = ui_emitter_start_size;
        self.ui_emitter_end_size = ui_emitter_end_size;
        self.ui_emitter_start_color = ui_emitter_start_color;
        self.ui_emitter_end_color = ui_emitter_end_color;
        self.selected_entity = selected_entity;

        let egui::FullOutput { platform_output, textures_delta, shapes, .. } = full_output;
        if let Some(window) = self.renderer.window() {
            self.egui_winit.as_mut().unwrap().handle_platform_output(window, platform_output);
        } else {
            return;
        }

        if actions.spawn_now {
            self.ecs.spawn_burst(&self.assets, self.ui_spawn_per_press as usize);
        }
        if let Some(entity) = actions.delete_entity {
            if self.ecs.despawn_entity(entity) {
                self.scripts.forget_entity(entity);
            }
            self.selected_entity = None;
        }
        if actions.clear_particles {
            self.ecs.clear_particles();
            self.ui_emitter_rate = 0.0;
            self.ui_emitter_spread = std::f32::consts::PI / 3.0;
            self.ui_emitter_speed = 0.8;
            self.ui_emitter_lifetime = 1.2;
            self.ui_emitter_start_size = 0.05;
            self.ui_emitter_end_size = 0.05;
            self.ui_emitter_start_color = [1.0, 1.0, 1.0, 1.0];
            self.ui_emitter_end_color = [1.0, 1.0, 1.0, 0.0];
            self.scripts.clear_handles();
            if let Some(emitter) = self.emitter_entity {
                self.ecs.set_emitter_rate(emitter, self.ui_emitter_rate);
                self.ecs.set_emitter_spread(emitter, self.ui_emitter_spread);
                self.ecs.set_emitter_speed(emitter, self.ui_emitter_speed);
                self.ecs.set_emitter_lifetime(emitter, self.ui_emitter_lifetime);
                self.ecs.set_emitter_colors(
                    emitter,
                    Vec4::from_array(self.ui_emitter_start_color),
                    Vec4::from_array(self.ui_emitter_end_color),
                );
                self.ecs.set_emitter_sizes(emitter, self.ui_emitter_start_size, self.ui_emitter_end_size);
            }
        }
        if actions.reset_world {
            self.ecs.clear_world();
            self.emitter_entity = None;
            self.selected_entity = None;
            self.scripts.clear_handles();
        }

        if let (Some(ren), Some(screen)) = (self.egui_renderer.as_mut(), self.egui_screen.as_ref()) {
            if let (Ok(device), Ok(queue)) = (self.renderer.device(), self.renderer.queue()) {
                for (id, delta) in &textures_delta.set {
                    ren.update_texture(device, queue, *id, delta);
                }
            }
            let meshes = self.egui_ctx.tessellate(shapes, screen.pixels_per_point);
            if let Err(err) = self.renderer.render_egui(ren, &meshes, screen) {
                eprintln!("Egui render error: {err:?}");
            }
            for id in &textures_delta.free {
                ren.free_texture(id);
            }
        }

        self.ecs.set_root_spin(self.ui_root_spin);

        if let Some(w) = self.renderer.window() {
            w.request_redraw();
        }
        self.input.clear_frame();
    }
}

impl App {
    fn apply_script_commands(&mut self, commands: Vec<ScriptCommand>) {
        for cmd in commands {
            match cmd {
                ScriptCommand::Spawn { handle, atlas, region, position, scale, velocity } => {
                    match self.ecs.spawn_scripted_sprite(
                        &self.assets,
                        &atlas,
                        &region,
                        position,
                        scale,
                        velocity,
                    ) {
                        Ok(entity) => {
                            self.scripts.register_spawn_result(handle, entity);
                        }
                        Err(err) => {
                            eprintln!("[script] spawn error for {atlas}:{region}: {err}");
                            self.scripts.forget_handle(handle);
                        }
                    }
                }
                ScriptCommand::SetVelocity { handle, velocity } => {
                    if let Some(entity) = self.scripts.resolve_handle(handle) {
                        if !self.ecs.set_velocity(entity, velocity) {
                            eprintln!("[script] set_velocity failed for handle {handle}");
                        }
                    } else {
                        eprintln!("[script] set_velocity unknown handle {handle}");
                    }
                }
                ScriptCommand::SetPosition { handle, position } => {
                    if let Some(entity) = self.scripts.resolve_handle(handle) {
                        if !self.ecs.set_translation(entity, position) {
                            eprintln!("[script] set_position failed for handle {handle}");
                        }
                    } else {
                        eprintln!("[script] set_position unknown handle {handle}");
                    }
                }
                ScriptCommand::Despawn { handle } => {
                    if let Some(entity) = self.scripts.resolve_handle(handle) {
                        if self.ecs.despawn_entity(entity) {
                            self.scripts.forget_handle(handle);
                        } else {
                            eprintln!("[script] despawn failed for handle {handle}");
                        }
                    } else {
                        eprintln!("[script] despawn unknown handle {handle}");
                    }
                }
                ScriptCommand::SetAutoSpawnRate { rate } => {
                    self.ui_auto_spawn_rate = rate.max(0.0);
                }
                ScriptCommand::SetSpawnPerPress { count } => {
                    self.ui_spawn_per_press = count.max(0);
                }
                ScriptCommand::SetEmitterRate { rate } => {
                    self.ui_emitter_rate = rate.max(0.0);
                    if let Some(emitter) = self.emitter_entity {
                        self.ecs.set_emitter_rate(emitter, self.ui_emitter_rate);
                    }
                }
                ScriptCommand::SetEmitterSpread { spread } => {
                    self.ui_emitter_spread = spread.clamp(0.0, std::f32::consts::PI);
                    if let Some(emitter) = self.emitter_entity {
                        self.ecs.set_emitter_spread(emitter, self.ui_emitter_spread);
                    }
                }
                ScriptCommand::SetEmitterSpeed { speed } => {
                    self.ui_emitter_speed = speed.max(0.0);
                    if let Some(emitter) = self.emitter_entity {
                        self.ecs.set_emitter_speed(emitter, self.ui_emitter_speed);
                    }
                }
                ScriptCommand::SetEmitterLifetime { lifetime } => {
                    self.ui_emitter_lifetime = lifetime.max(0.05);
                    if let Some(emitter) = self.emitter_entity {
                        self.ecs.set_emitter_lifetime(emitter, self.ui_emitter_lifetime);
                    }
                }
                ScriptCommand::SetEmitterStartColor { color } => {
                    self.ui_emitter_start_color = color.to_array();
                    if let Some(emitter) = self.emitter_entity {
                        self.ecs.set_emitter_colors(
                            emitter,
                            color,
                            Vec4::from_array(self.ui_emitter_end_color),
                        );
                    }
                }
                ScriptCommand::SetEmitterEndColor { color } => {
                    self.ui_emitter_end_color = color.to_array();
                    if let Some(emitter) = self.emitter_entity {
                        self.ecs.set_emitter_colors(
                            emitter,
                            Vec4::from_array(self.ui_emitter_start_color),
                            color,
                        );
                    }
                }
                ScriptCommand::SetEmitterStartSize { size } => {
                    self.ui_emitter_start_size = size.max(0.01);
                    if let Some(emitter) = self.emitter_entity {
                        self.ecs.set_emitter_sizes(
                            emitter,
                            self.ui_emitter_start_size,
                            self.ui_emitter_end_size,
                        );
                    }
                }
                ScriptCommand::SetEmitterEndSize { size } => {
                    self.ui_emitter_end_size = size.max(0.01);
                    if let Some(emitter) = self.emitter_entity {
                        self.ecs.set_emitter_sizes(
                            emitter,
                            self.ui_emitter_start_size,
                            self.ui_emitter_end_size,
                        );
                    }
                }
            }
        }
    }
}
