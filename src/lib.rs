
pub mod time;
pub mod input;
pub mod renderer;
pub mod ecs;
pub mod assets;

use crate::ecs::EcsWorld;
use crate::renderer::Renderer;
use crate::time::Time;
use crate::input::{Input, InputEvent};
use crate::assets::AssetManager;

use bevy_ecs::prelude::Entity;
use glam::{Mat4, Vec2, Vec3, Vec4};

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};

// egui
use egui::{Context as EguiCtx};
use egui_winit::State as EguiWinit;
use egui_wgpu::{Renderer as EguiRenderer, RendererOptions, ScreenDescriptor};
use egui_plot as eplot;

const CAMERA_BASE_HALF_HEIGHT: f32 = 1.2;

pub async fn run() {
    let event_loop = EventLoop::new().unwrap();
    let mut app = App::new().await;
    event_loop.run_app(&mut app).unwrap();
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

    // Camera / selection
    camera_pos: Vec2,
    camera_zoom: f32,
    selected_entity: Option<Entity>,
}

impl App {
    pub async fn new() -> Self {
        let renderer = Renderer::new().await;
        let mut ecs = EcsWorld::new();
        ecs.spawn_demo_scene();
        let time = Time::new();
        let input = Input::new();
        let assets = AssetManager::new();

        // egui context and state
        let egui_ctx = EguiCtx::default();
        let egui_winit = None;

        Self {
            renderer, ecs, time, input, assets,
            should_close: false,
            accumulator: 0.0,
            fixed_dt: 1.0/60.0,
            egui_ctx, egui_winit,
            egui_renderer: None,
            egui_screen: None,
            ui_spawn_per_press: 200,
            ui_auto_spawn_rate: 0.0,
            ui_cell_size: 0.25,
            ui_hist: Vec::with_capacity(240),
            ui_root_spin: 1.2,
            camera_pos: Vec2::ZERO,
            camera_zoom: 1.0,
            selected_entity: None,
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.renderer.ensure_window(event_loop);
        let (device, queue) = self.renderer.device_and_queue();
        self.assets.set_device(device, queue);
        self.assets.load_atlas("main", "assets/images/atlas.json").expect("atlas");
        let atlas_view = self.assets.atlas_texture_view("main").expect("atlas tex");
        let sampler = self.assets.default_sampler().clone();
        self.renderer.init_sprite_pipeline_with_atlas(atlas_view, sampler);

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
        let egui_renderer = EguiRenderer::new(self.renderer.device(), self.renderer.surface_format(), RendererOptions::default());
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

        if consumed { return; }

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
                    if *state == ElementState::Pressed { self.should_close = true; }
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _e: &ActiveEventLoop, _dev: winit::event::DeviceId, ev: DeviceEvent) {
        self.input.push(InputEvent::from_device_event(&ev));
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.should_close { event_loop.exit(); return; }
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

        if let Some(delta) = self.input.consume_wheel_delta() {
            let zoom_multiplier = (-delta * 0.1).exp();
            self.camera_zoom = (self.camera_zoom * zoom_multiplier).clamp(0.25, 5.0);
        }

        if self.input.right_held() {
            let (dx, dy) = self.input.mouse_delta;
            if dx.abs() > f32::EPSILON || dy.abs() > f32::EPSILON {
                let pan = self.screen_delta_to_world(dx, dy);
                self.camera_pos -= pan;
            }
        }

        let view_proj = self.view_projection_matrix();
        let inv_view_proj = view_proj.inverse();

        if self.input.take_left_click() {
            if let Some((sx, sy)) = self.input.cursor_position() {
                if let Some(world) = self.screen_to_world(Vec2::new(sx, sy), inv_view_proj) {
                    self.selected_entity = self.ecs.pick_entity(world);
                } else {
                    self.selected_entity = None;
                }
            }
        }

        self.ecs.set_spatial_cell(self.ui_cell_size.max(0.05));

        while self.accumulator >= self.fixed_dt {
            self.ecs.fixed_step(self.fixed_dt);
            self.accumulator -= self.fixed_dt;
        }
        self.ecs.update(dt);

        let (instances, _atlas) = self.ecs.collect_sprite_instances(&self.assets);
        let _ = self.renderer.render_batch(&instances, view_proj);

        if self.egui_winit.is_none() { return; }
        let window_size = self.renderer.size();
        let pixels_per_point = self.renderer.pixels_per_point();

        let raw_input = {
            let Some(window) = self.renderer.window() else { return; };
            self.egui_winit.as_mut().unwrap().take_egui_input(window)
        };
        let dt_ms = dt * 1000.0;
        self.ui_hist.push(dt_ms);
        if self.ui_hist.len() > 240 { self.ui_hist.remove(0); }

        let hist_points: Vec<[f64;2]> = self.ui_hist.iter().enumerate().map(|(i,v)| [i as f64, *v as f64]).collect();
        let entity_count = self.ecs.entity_count();
        let instances_drawn = instances.len();
        let mut ui_cell_size = self.ui_cell_size;
        let mut ui_spawn_per_press = self.ui_spawn_per_press;
        let mut ui_auto_spawn_rate = self.ui_auto_spawn_rate;
        let mut ui_root_spin = self.ui_root_spin;
        let mut selected_entity = self.selected_entity;
        let mut selection_details = selected_entity.and_then(|entity| self.ecs.entity_info(entity));
        let mut highlight_rect = selected_entity
            .and_then(|entity| self.ecs.entity_bounds(entity))
            .and_then(|(min, max)| self.world_rect_to_screen_rect(min, max, view_proj, window_size, pixels_per_point));

        #[derive(Default)]
        struct UiActions {
            spawn_now: bool,
            delete_entity: Option<Entity>,
        }
        let mut actions = UiActions::default();

        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            egui::Window::new("Kestrel Debug").resizable(true).show(ctx, |ui| {
                ui.label(format!("Entities: {}", entity_count));
                ui.label(format!("Instances drawn: {}", instances_drawn));
                ui.separator();
                ui.add(egui::Slider::new(&mut ui_cell_size, 0.05..=0.8).text("Spatial cell size"));
                ui.add(egui::Slider::new(&mut ui_spawn_per_press, 1..=5000).text("Spawn per press"));
                ui.add(egui::Slider::new(&mut ui_auto_spawn_rate, 0.0..=5000.0).text("Auto-spawn per second"));
                ui.add(egui::Slider::new(&mut ui_root_spin, -5.0..=5.0).text("Root spin speed"));
                if ui.button("Spawn now").clicked() {
                    actions.spawn_now = true;
                }
                ui.separator();
                let hist = eplot::Plot::new("fps_plot").height(120.0).include_y(0.0).include_y(40.0);
                hist.show(ui, |plot_ui| {
                    plot_ui.line(eplot::Line::new("ms/frame", eplot::PlotPoints::from(hist_points.clone())));
                });
                ui.label("Target: 16.7ms for 60 FPS");
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
                    }
                } else {
                    ui.label("No entity selected");
                }
            });

            if let Some(rect) = highlight_rect {
                ctx.debug_painter().rect_stroke(rect, 0.0, egui::Stroke::new(2.0, egui::Color32::YELLOW), egui::StrokeKind::Inside);
            }
        });

        self.ui_cell_size = ui_cell_size;
        self.ui_spawn_per_press = ui_spawn_per_press;
        self.ui_auto_spawn_rate = ui_auto_spawn_rate;
        self.ui_root_spin = ui_root_spin;
        self.selected_entity = selected_entity;

        let egui::FullOutput {
            platform_output,
            textures_delta,
            shapes,
            ..
        } = full_output;
        if let Some(window) = self.renderer.window() {
            self.egui_winit.as_mut().unwrap().handle_platform_output(window, platform_output);
        } else {
            return;
        }

        if actions.spawn_now {
            self.ecs.spawn_burst(&self.assets, self.ui_spawn_per_press as usize);
        }
        if let Some(entity) = actions.delete_entity {
            self.ecs.despawn_entity(entity);
            self.selected_entity = None;
        }

        if let (Some(ren), Some(screen)) = (self.egui_renderer.as_mut(), self.egui_screen.as_ref()) {
            for (id, delta) in &textures_delta.set {
                ren.update_texture(self.renderer.device(), self.renderer.queue(), *id, delta);
            }
            let meshes = self.egui_ctx.tessellate(shapes, screen.pixels_per_point);
            let _ = self.renderer.render_egui(ren, &meshes, screen);
            for id in &textures_delta.free { ren.free_texture(id); }
        }

        self.ecs.set_root_spin(self.ui_root_spin);

        if let Some(w) = self.renderer.window() { w.request_redraw(); }
        self.input.clear_frame();
    }
}

impl App {
    fn view_projection_matrix(&self) -> Mat4 {
        let aspect = self.renderer.aspect_ratio();
        let half_height = CAMERA_BASE_HALF_HEIGHT / self.camera_zoom;
        let half_width = half_height * aspect;
        let proj = Mat4::orthographic_rh_gl(-half_width, half_width, -half_height, half_height, -1.0, 1.0);
        let view = Mat4::from_translation(Vec3::new(-self.camera_pos.x, -self.camera_pos.y, 0.0));
        proj * view
    }

    fn screen_delta_to_world(&self, dx: f32, dy: f32) -> Vec2 {
        let size = self.renderer.size();
        if size.width == 0 || size.height == 0 { return Vec2::ZERO; }
        let half_height = CAMERA_BASE_HALF_HEIGHT / self.camera_zoom;
        let half_width = half_height * self.renderer.aspect_ratio();
        let world_width = half_width * 2.0;
        let world_height = half_height * 2.0;
        Vec2::new(
            dx / size.width as f32 * world_width,
            -dy / size.height as f32 * world_height,
        )
    }

    fn screen_to_world(&self, screen: Vec2, inv_view_proj: Mat4) -> Option<Vec2> {
        let size = self.renderer.size();
        if size.width == 0 || size.height == 0 { return None; }
        let ndc_x = (screen.x / size.width as f32) * 2.0 - 1.0;
        let ndc_y = 1.0 - (screen.y / size.height as f32) * 2.0;
        let clip = Vec4::new(ndc_x, ndc_y, 0.0, 1.0);
        let world = inv_view_proj * clip;
        if world.w.abs() <= f32::EPSILON { return None; }
        let world = world / world.w;
        Some(Vec2::new(world.x, world.y))
    }

    fn world_to_screen_pixels(&self, world: Vec2, view_proj: Mat4, window_size: PhysicalSize<u32>) -> Option<Vec2> {
        if window_size.width == 0 || window_size.height == 0 { return None; }
        let clip = view_proj * Vec4::new(world.x, world.y, 0.0, 1.0);
        if clip.w.abs() <= f32::EPSILON { return None; }
        let ndc = clip.truncate() / clip.w;
        let x = (ndc.x + 1.0) * 0.5 * window_size.width as f32;
        let y = (1.0 - ndc.y) * 0.5 * window_size.height as f32;
        Some(Vec2::new(x, y))
    }

    fn world_rect_to_screen_rect(
        &self,
        min: Vec2,
        max: Vec2,
        view_proj: Mat4,
        window_size: PhysicalSize<u32>,
        pixels_per_point: f32,
    ) -> Option<egui::Rect> {
        let corners = [
            self.world_to_screen_pixels(Vec2::new(min.x, min.y), view_proj, window_size)?,
            self.world_to_screen_pixels(Vec2::new(min.x, max.y), view_proj, window_size)?,
            self.world_to_screen_pixels(Vec2::new(max.x, min.y), view_proj, window_size)?,
            self.world_to_screen_pixels(Vec2::new(max.x, max.y), view_proj, window_size)?,
        ];
        let mut min_screen = Vec2::splat(f32::INFINITY);
        let mut max_screen = Vec2::splat(f32::NEG_INFINITY);
        for p in corners {
            min_screen = min_screen.min(p);
            max_screen = max_screen.max(p);
        }
        let top_left = egui::pos2(min_screen.x / pixels_per_point, min_screen.y / pixels_per_point);
        let bottom_right = egui::pos2(max_screen.x / pixels_per_point, max_screen.y / pixels_per_point);
        Some(egui::Rect::from_two_pos(top_left, bottom_right))
    }
}
