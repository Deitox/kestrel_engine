use crate::assets::ClipInterpolation;
use crate::ecs::AnimationTime;
use bevy_ecs::prelude::Entity;
use egui::{self, pos2, Color32, FontId, Id, Key, Modifiers, Rect, Sense, Stroke, Ui};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AnimationTrackId(u64);

impl AnimationTrackId {
    pub fn for_entity_slot(entity: Entity, slot_index: u32) -> Self {
        let entity_bits = entity.index() as u64;
        let slot_bits = slot_index as u64;
        Self((entity_bits << 32) | slot_bits)
    }

    pub fn raw(&self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyframeId {
    pub track: AnimationTrackId,
    pub index: u32,
}

impl KeyframeId {
    pub fn new(track: AnimationTrackId, index: usize) -> Self {
        Self { track, index: index as u32 }
    }

    pub fn egui_id(&self) -> Id {
        Id::new((self.track.raw(), self.index))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AnimationTrackKind {
    SpriteTimeline,
    Translation,
    Rotation,
    Scale,
    Tint,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyframeValue {
    None,
    Scalar(f32),
    Vec2([f32; 2]),
    Vec4([f32; 4]),
}

impl Default for KeyframeValue {
    fn default() -> Self {
        KeyframeValue::None
    }
}

impl KeyframeValue {
    pub fn as_scalar(self) -> Option<f32> {
        if let KeyframeValue::Scalar(value) = self {
            Some(value)
        } else {
            None
        }
    }

    pub fn as_vec2(self) -> Option<[f32; 2]> {
        if let KeyframeValue::Vec2(value) = self {
            Some(value)
        } else {
            None
        }
    }

    pub fn as_vec4(self) -> Option<[f32; 4]> {
        if let KeyframeValue::Vec4(value) = self {
            Some(value)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationTrackBinding {
    SpriteTimeline { entity: Entity },
    TransformChannel { entity: Entity, channel: AnimationTrackKind },
}

#[derive(Clone, Debug)]
pub enum AnimationPanelCommand {
    ScrubTrack {
        binding: AnimationTrackBinding,
        time: f32,
    },
    InsertKey {
        binding: AnimationTrackBinding,
        time: f32,
    },
    DeleteKeys {
        binding: AnimationTrackBinding,
        indices: Vec<usize>,
    },
    UpdateKey {
        binding: AnimationTrackBinding,
        index: usize,
        new_time: Option<f32>,
        new_value: Option<KeyframeValue>,
    },
    AdjustKeys {
        binding: AnimationTrackBinding,
        indices: Vec<usize>,
        time_delta: Option<f32>,
        value_delta: Option<KeyframeValue>,
    },
    Undo,
    Redo,
}

/// Lightweight summary for each animation track shown in the panel.
#[derive(Clone)]
pub struct AnimationTrackSummary {
    pub id: AnimationTrackId,
    pub label: String,
    pub kind: AnimationTrackKind,
    pub binding: AnimationTrackBinding,
    pub duration: f32,
    pub key_count: usize,
    pub interpolation: Option<ClipInterpolation>,
    pub playhead: Option<f32>,
    pub dirty: bool,
    pub key_details: Vec<KeyframeDetail>,
}

#[derive(Clone, Debug)]
pub struct KeyframeDetail {
    pub id: KeyframeId,
    pub index: usize,
    pub time: Option<f32>,
    pub value_preview: Option<String>,
    pub value: KeyframeValue,
}

/// Snapshot of editor state passed into the panel each frame.
pub struct AnimationKeyframePanelState<'a> {
    pub animation_time: &'a AnimationTime,
    pub selected_entity: Option<Entity>,
    pub track_summaries: Vec<AnimationTrackSummary>,
    pub can_undo: bool,
    pub can_redo: bool,
    pub status_message: Option<String>,
}

#[derive(Default)]
pub struct AnimationKeyframePanel {
    open: bool,
    track_filter: String,
    selected_tracks: BTreeSet<AnimationTrackId>,
    selected_keys: BTreeSet<KeyframeId>,
    scrub_time: f32,
    visible_duration: f32,
    pending_commands: Vec<AnimationPanelCommand>,
    multi_time_offset: f32,
    multi_scalar_offset: f32,
    multi_vec2_offset: [f32; 2],
    multi_vec4_offset: [f32; 4],
}

impl AnimationKeyframePanel {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    pub fn drain_commands(&mut self) -> Vec<AnimationPanelCommand> {
        std::mem::take(&mut self.pending_commands)
    }

    pub fn render_window(&mut self, ctx: &egui::Context, state: AnimationKeyframePanelState<'_>) {
        let mut open = self.open;
        egui::Window::new("Keyframe Editor (Milestone 5)")
            .open(&mut open)
            .default_width(480.0)
            .min_height(320.0)
            .show(ctx, |ui| {
                self.render_contents(ui, &state);
            });
        self.open = open;
    }

    fn render_contents(&mut self, ui: &mut Ui, state: &AnimationKeyframePanelState<'_>) {
        ui.heading("Keyframe Timeline");
        if let Some(status) = &state.status_message {
            ui.small(status);
        }
        match state.selected_entity {
            Some(entity) => {
                ui.label(format!("Entity ID {}", entity.index()));
            }
            None => {
                ui.label("Select an entity to inspect its animation clips.");
                return;
            }
        }
        ui.horizontal(|ui| {
            ui.label("Track filter:");
            let response = ui.text_edit_singleline(&mut self.track_filter);
            if response.changed() && self.track_filter.is_empty() {
                self.selected_tracks.clear();
                self.selected_keys.clear();
            }
            if ui.button("Clear").clicked() {
                self.track_filter.clear();
            }
            if ui.add_enabled(state.can_undo, egui::Button::new("Undo")).clicked() {
                self.pending_commands.push(AnimationPanelCommand::Undo);
            }
            if ui.add_enabled(state.can_redo, egui::Button::new("Redo")).clicked() {
                self.pending_commands.push(AnimationPanelCommand::Redo);
            }
        });
        ui.add_space(4.0);
        let filtered_tracks = self.filtered_tracks(state);
        self.reconcile_selection(&filtered_tracks);
        if filtered_tracks.is_empty() {
            ui.label("No animation tracks match the current filter.");
            return;
        }
        let max_duration =
            filtered_tracks.iter().fold(0.0_f32, |acc, summary| acc.max(summary.duration)).max(0.001);
        if self.visible_duration <= f32::EPSILON {
            self.visible_duration = max_duration;
        } else {
            self.visible_duration = self.visible_duration.max(max_duration);
        }
        self.scrub_time = self.scrub_time.clamp(0.0, self.visible_duration);
        ui.horizontal(|ui| {
            ui.label("Scrub");
            let scrub_label = format!("{:.2}s / {:.2}s", self.scrub_time, self.visible_duration);
            if ui
                .add(egui::Slider::new(&mut self.scrub_time, 0.0..=self.visible_duration).text(scrub_label))
                .changed()
            {
                self.queue_scrub_for_selection(&filtered_tracks);
            }
            if ui.button("Reset").clicked() {
                self.scrub_time = 0.0;
                self.queue_scrub_for_selection(&filtered_tracks);
            }
        });
        ui.separator();
        ui.horizontal(|ui| {
            let track_area_height = (filtered_tracks.len() as f32 * 40.0 + 80.0).clamp(240.0, 560.0);
            ui.set_height(track_area_height);
            ui.vertical(|ui| {
                ui.set_min_width(220.0);
                ui.strong("Tracks");
                self.render_track_list(ui, &filtered_tracks);
            });
            ui.separator();
            ui.vertical(|ui| {
                ui.strong("Timeline");
                self.render_timeline(ui, &filtered_tracks);
            });
        });
        ui.separator();
        self.render_selection_overview(ui, &filtered_tracks);
        ui.separator();
        ui.label(format!(
            "Animation clock: scale {:.2}, paused {}, fixed_step {:?}",
            state.animation_time.scale, state.animation_time.paused, state.animation_time.fixed_step
        ));
        if !state.animation_time.group_scales.is_empty() {
            egui::CollapsingHeader::new("Per-group Scale Overrides").show(ui, |ui| {
                for (group, scale) in state.animation_time.group_scales.iter() {
                    ui.label(format!("{group}: {scale:.2}x"));
                }
            });
        }
    }

    fn filtered_tracks<'a>(
        &self,
        state: &'a AnimationKeyframePanelState<'a>,
    ) -> Vec<&'a AnimationTrackSummary> {
        if self.track_filter.trim().is_empty() {
            state.track_summaries.iter().collect()
        } else {
            let filter = self.track_filter.to_lowercase();
            state
                .track_summaries
                .iter()
                .filter(|summary| summary.label.to_lowercase().contains(&filter))
                .collect()
        }
    }

    fn reconcile_selection(&mut self, tracks: &[&AnimationTrackSummary]) {
        if tracks.is_empty() {
            self.selected_tracks.clear();
            self.selected_keys.clear();
            return;
        }
        let mut valid_tracks: BTreeSet<AnimationTrackId> = BTreeSet::new();
        for summary in tracks {
            valid_tracks.insert(summary.id);
        }
        self.selected_tracks.retain(|track_id| valid_tracks.contains(track_id));
        self.selected_keys.retain(|key| valid_tracks.contains(&key.track));
        if self.selected_tracks.is_empty() {
            if let Some(first) = tracks.first() {
                self.selected_tracks.insert(first.id);
                if let Some(playhead) = first.playhead {
                    self.scrub_time = playhead;
                }
            }
        }
    }

    fn render_track_list(&mut self, ui: &mut Ui, tracks: &[&AnimationTrackSummary]) {
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            for summary in tracks {
                let selected = self.selected_tracks.contains(&summary.id);
                let dirty_suffix = if summary.dirty { " *" } else { "" };
                let text = format!(
                    "{} ({}){}\n{} keys | {:.2}s",
                    summary.label,
                    self.track_kind_label(summary.kind),
                    dirty_suffix,
                    summary.key_count,
                    summary.duration
                );
                let response = ui.add_sized(
                    egui::vec2(ui.available_width(), 44.0),
                    egui::Button::new(text).wrap().selected(selected),
                );
                if response.clicked() {
                    let modifiers = ui.input(|input| input.modifiers);
                    self.handle_track_click(summary, modifiers);
                }
            }
        });
    }

    fn queue_scrub_for_selection(&mut self, tracks: &[&AnimationTrackSummary]) {
        if tracks.is_empty() {
            return;
        }
        for summary in tracks {
            if self.selected_tracks.contains(&summary.id) {
                let clamped_time = self.scrub_time.min(summary.duration.max(0.0));
                self.pending_commands
                    .push(AnimationPanelCommand::ScrubTrack { binding: summary.binding, time: clamped_time });
            }
        }
    }

    fn render_timeline(&mut self, ui: &mut Ui, tracks: &[&AnimationTrackSummary]) {
        let axis_height = 26.0;
        let track_height = 36.0;
        let total_height = axis_height + tracks.len() as f32 * track_height + 12.0;
        let desired_size = egui::vec2(ui.available_width(), total_height.max(160.0));
        let (response, painter) = ui.allocate_painter(desired_size, Sense::hover());
        let rect = response.rect;
        let axis_rect = Rect::from_min_max(rect.left_top(), pos2(rect.right(), rect.top() + axis_height));
        let duration = self.visible_duration.max(0.001);
        self.draw_time_axis(&painter, axis_rect, duration, ui);
        let mut row_top = axis_rect.bottom();
        for summary in tracks {
            let row_rect =
                Rect::from_min_max(pos2(rect.left(), row_top), pos2(rect.right(), row_top + track_height));
            self.draw_track_row(ui, &painter, row_rect, summary, duration);
            row_top += track_height;
        }
        let scrub_x = self.time_to_screen(rect, duration, self.scrub_time);
        painter.line_segment(
            [pos2(scrub_x, axis_rect.bottom()), pos2(scrub_x, rect.bottom())],
            Stroke::new(2.0, Color32::from_rgb(255, 196, 94)),
        );
    }

    fn draw_time_axis(&self, painter: &egui::Painter, rect: Rect, duration: f32, ui: &Ui) {
        painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
        painter.line_segment(
            [pos2(rect.left(), rect.bottom()), pos2(rect.right(), rect.bottom())],
            Stroke::new(1.0, ui.visuals().widgets.noninteractive.fg_stroke.color),
        );
        let tick_step = self.tick_step(duration);
        let mut tick = 0.0;
        while tick <= duration + f32::EPSILON {
            let x = self.time_to_screen(rect, duration, tick);
            painter.line_segment(
                [pos2(x, rect.bottom() - 6.0), pos2(x, rect.bottom())],
                Stroke::new(1.0, Color32::from_gray(120)),
            );
            painter.text(
                pos2(x + 4.0, rect.top() + 2.0),
                egui::Align2::LEFT_TOP,
                format!("{tick:.2}s"),
                FontId::monospace(11.0),
                ui.visuals().text_color(),
            );
            tick += tick_step;
        }
    }

    fn draw_track_row(
        &mut self,
        ui: &mut Ui,
        painter: &egui::Painter,
        rect: Rect,
        summary: &AnimationTrackSummary,
        duration: f32,
    ) {
        let row_id = Id::new(("track_row_canvas", summary.id.raw()));
        let row_response = ui.interact(rect, row_id, Sense::click());
        if row_response.double_clicked() {
            if let Some(pos) = row_response.interact_pointer_pos() {
                let local_time = self.screen_to_time(rect, duration, pos.x);
                self.handle_insert_request(summary, local_time);
            }
        }
        let is_selected = self.selected_tracks.contains(&summary.id);
        let bg_color = if is_selected {
            ui.visuals().extreme_bg_color.linear_multiply(1.25)
        } else {
            ui.visuals().faint_bg_color
        };
        painter.rect_filled(rect, 4.0, bg_color);
        if let Some(playhead) = summary.playhead {
            let playhead_x = self.time_to_screen(rect, duration, playhead);
            painter.line_segment(
                [pos2(playhead_x, rect.top()), pos2(playhead_x, rect.bottom())],
                Stroke::new(1.0, Color32::from_rgb(96, 196, 255)),
            );
        }
        for detail in &summary.key_details {
            if let Some(time) = detail.time {
                self.draw_keyframe(ui, painter, rect, duration, summary, detail, time);
            }
        }
    }

    fn draw_keyframe(
        &mut self,
        ui: &mut Ui,
        painter: &egui::Painter,
        row_rect: Rect,
        duration: f32,
        summary: &AnimationTrackSummary,
        detail: &KeyframeDetail,
        time: f32,
    ) {
        let center_x = self.time_to_screen(row_rect, duration, time);
        let center = pos2(center_x, row_rect.center().y);
        let rect = Rect::from_center_size(center, egui::vec2(12.0, 12.0));
        let base_color = if self.selected_keys.contains(&detail.id) {
            Color32::from_rgb(250, 138, 64)
        } else {
            Color32::from_rgb(110, 170, 255)
        };
        painter.rect_filled(rect, 2.0, base_color);
        let response = ui.interact(rect, detail.id.egui_id(), Sense::click());
        if response.clicked() {
            let modifiers = ui.input(|input| input.modifiers);
            self.handle_key_click(detail.id, summary.id, modifiers);
        }
        let preview = detail.value_preview.as_deref().unwrap_or("value pending");
        response.on_hover_text(format!("{} #{} @ {:.3}s\n{}", summary.label, detail.index, time, preview));
    }

    fn render_selection_overview(&mut self, ui: &mut Ui, tracks: &[&AnimationTrackSummary]) {
        if let Some(track_id) = self.selected_tracks.iter().next().copied() {
            if let Some(summary) = tracks.iter().find(|summary| summary.id == track_id) {
                ui.label(format!(
                    "Selected track: {} ({}) • {} keys • {}",
                    summary.label,
                    self.track_kind_label(summary.kind),
                    summary.key_count,
                    self.interpolation_label(summary.interpolation)
                ));
            }
        } else {
            ui.label("Selected track: none");
        }
        let selection_info = self.selection_binding_and_indices(tracks);
        if self.selected_keys.is_empty() {
            ui.label("Selected keys: none");
        } else {
            ui.label(format!("Selected keys: {}", self.selected_keys.len()));
        }
        let delete_enabled = selection_info.is_some();
        let delete_button = ui.add_enabled(delete_enabled, egui::Button::new("Delete Selected Keys"));
        let delete_request =
            delete_button.clicked() || (delete_enabled && ui.input(|i| i.key_pressed(Key::Delete)));
        if delete_request {
            if let Some((binding, _, mut indices)) = selection_info.clone() {
                indices.sort();
                self.pending_commands.push(AnimationPanelCommand::DeleteKeys { binding, indices });
                self.selected_keys.clear();
            }
        }
        if self.selected_keys.len() > 1 {
            if let Some(selection) = selection_info.clone() {
                self.render_multi_edit_controls(ui, selection);
            }
        }
        if self.selected_keys.len() > 1 {
            if let Some(selection) = selection_info.clone() {
                self.render_multi_edit_controls(ui, selection);
            }
        }
        self.render_key_editor(ui, tracks);
    }

    fn render_key_editor(&mut self, ui: &mut Ui, tracks: &[&AnimationTrackSummary]) {
        if self.selected_keys.len() != 1 {
            return;
        }
        let Some((summary, detail)) = self.primary_selected_key(tracks) else {
            return;
        };
        if matches!(summary.kind, AnimationTrackKind::SpriteTimeline) {
            ui.label("Sprite keys are read-only.");
            return;
        }
        ui.separator();
        ui.strong("Key Editor");
        if let Some(mut time) = detail.time {
            let mut changed = false;
            let mut time_widget = egui::DragValue::new(&mut time).speed(0.01);
            time_widget = time_widget.prefix("Time ");
            if ui.add(time_widget).changed() {
                changed = true;
            }
            if changed {
                self.pending_commands.push(AnimationPanelCommand::UpdateKey {
                    binding: summary.binding,
                    index: detail.index,
                    new_time: Some(time.max(0.0)),
                    new_value: None,
                });
            }
        } else {
            ui.label("Key time unavailable.");
        }
        match summary.kind {
            AnimationTrackKind::Translation | AnimationTrackKind::Scale => {
                if let Some(mut value) = detail.value.as_vec2() {
                    let mut changed = false;
                    ui.horizontal(|ui| {
                        ui.label("Value (X,Y)");
                        if ui.add(egui::DragValue::new(&mut value[0]).speed(0.01)).changed() {
                            changed = true;
                        }
                        if ui.add(egui::DragValue::new(&mut value[1]).speed(0.01)).changed() {
                            changed = true;
                        }
                    });
                    if changed {
                        self.pending_commands.push(AnimationPanelCommand::UpdateKey {
                            binding: summary.binding,
                            index: detail.index,
                            new_time: None,
                            new_value: Some(KeyframeValue::Vec2(value)),
                        });
                    }
                } else {
                    ui.label("Value unavailable.");
                }
            }
            AnimationTrackKind::Rotation => {
                if let Some(mut value) = detail.value.as_scalar() {
                    if ui.add(egui::DragValue::new(&mut value).speed(0.01).prefix("Degrees ")).changed() {
                        self.pending_commands.push(AnimationPanelCommand::UpdateKey {
                            binding: summary.binding,
                            index: detail.index,
                            new_time: None,
                            new_value: Some(KeyframeValue::Scalar(value)),
                        });
                    }
                } else {
                    ui.label("Value unavailable.");
                }
            }
            AnimationTrackKind::Tint => {
                if let Some(mut value) = detail.value.as_vec4() {
                    let mut changed = false;
                    ui.horizontal(|ui| {
                        ui.label("RGBA");
                        for channel in value.iter_mut() {
                            if ui.add(egui::DragValue::new(channel).speed(0.01)).changed() {
                                changed = true;
                            }
                        }
                    });
                    if changed {
                        self.pending_commands.push(AnimationPanelCommand::UpdateKey {
                            binding: summary.binding,
                            index: detail.index,
                            new_time: None,
                            new_value: Some(KeyframeValue::Vec4(value)),
                        });
                    }
                } else {
                    ui.label("Value unavailable.");
                }
            }
            AnimationTrackKind::SpriteTimeline => {}
        }
    }

    fn handle_track_click(&mut self, summary: &AnimationTrackSummary, modifiers: Modifiers) {
        if modifiers.command || modifiers.ctrl {
            if self.selected_tracks.contains(&summary.id) {
                self.selected_tracks.remove(&summary.id);
            } else {
                self.selected_tracks.insert(summary.id);
            }
        } else {
            self.selected_tracks.clear();
            self.selected_tracks.insert(summary.id);
        }
        if let Some(playhead) = summary.playhead {
            self.scrub_time = playhead;
            self.pending_commands.push(AnimationPanelCommand::ScrubTrack {
                binding: summary.binding,
                time: playhead.min(summary.duration.max(0.0)),
            });
        }
        self.selected_keys.retain(|key| self.selected_tracks.contains(&key.track));
    }

    fn handle_key_click(&mut self, key_id: KeyframeId, track_id: AnimationTrackId, modifiers: Modifiers) {
        if modifiers.command || modifiers.ctrl {
            if !self.selected_keys.insert(key_id) {
                self.selected_keys.remove(&key_id);
            }
        } else {
            self.selected_keys.clear();
            self.selected_keys.insert(key_id);
        }
        if !self.selected_tracks.contains(&track_id) {
            self.selected_tracks.clear();
            self.selected_tracks.insert(track_id);
        }
    }

    fn handle_insert_request(&mut self, summary: &AnimationTrackSummary, time: f32) {
        if !self.can_edit_track(summary.kind) {
            return;
        }
        self.pending_commands.push(AnimationPanelCommand::InsertKey {
            binding: summary.binding,
            time: time.min(summary.duration.max(0.0)),
        });
    }

    fn can_edit_track(&self, kind: AnimationTrackKind) -> bool {
        !matches!(kind, AnimationTrackKind::SpriteTimeline)
    }

    fn time_to_screen(&self, rect: Rect, duration: f32, time: f32) -> f32 {
        if duration <= 0.0 {
            rect.left()
        } else {
            let normalized = (time / duration).clamp(0.0, 1.0);
            rect.left() + normalized * rect.width()
        }
    }

    fn screen_to_time(&self, rect: Rect, duration: f32, x: f32) -> f32 {
        if duration <= 0.0 {
            0.0
        } else {
            let normalized = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            normalized * duration
        }
    }

    fn tick_step(&self, duration: f32) -> f32 {
        if duration <= 0.0 {
            return 1.0;
        }
        let raw = (duration / 6.0).max(0.001);
        let power = raw.log10().floor();
        let base = 10_f32.powf(power);
        let mantissa = raw / base;
        let snapped = if mantissa < 1.5 {
            1.0
        } else if mantissa < 3.0 {
            2.0
        } else if mantissa < 7.0 {
            5.0
        } else {
            10.0
        };
        snapped * base
    }

    fn interpolation_label(&self, interpolation: Option<ClipInterpolation>) -> &'static str {
        match interpolation {
            Some(ClipInterpolation::Linear) => "Linear",
            Some(ClipInterpolation::Step) => "Step",
            None => "Unknown",
        }
    }

    fn track_kind_label(&self, kind: AnimationTrackKind) -> &'static str {
        match kind {
            AnimationTrackKind::SpriteTimeline => "Sprite",
            AnimationTrackKind::Translation => "Translation",
            AnimationTrackKind::Rotation => "Rotation",
            AnimationTrackKind::Scale => "Scale",
            AnimationTrackKind::Tint => "Tint",
        }
    }

    fn selection_binding_and_indices(
        &self,
        tracks: &[&AnimationTrackSummary],
    ) -> Option<(AnimationTrackBinding, AnimationTrackKind, Vec<usize>)> {
        if self.selected_keys.is_empty() {
            return None;
        }
        let mut binding: Option<AnimationTrackBinding> = None;
        let mut kind: Option<AnimationTrackKind> = None;
        let mut indices = Vec::new();
        for key in &self.selected_keys {
            let summary = tracks.iter().find(|summary| summary.id == key.track)?;
            if !self.can_edit_track(summary.kind) {
                return None;
            }
            match binding {
                Some(existing) if existing != summary.binding => return None,
                None => {
                    binding = Some(summary.binding);
                    kind = Some(summary.kind);
                }
                _ => {}
            }
            indices.push(key.index as usize);
        }
        binding.and_then(|binding| kind.map(|kind| (binding, kind, indices)))
    }

    fn primary_selected_key<'a>(
        &self,
        tracks: &[&'a AnimationTrackSummary],
    ) -> Option<(&'a AnimationTrackSummary, &'a KeyframeDetail)> {
        let key = self.selected_keys.iter().next()?;
        let summary = tracks.iter().find(|summary| summary.id == key.track)?;
        let detail = summary.key_details.iter().find(|detail| detail.id == *key)?;
        Some((summary, detail))
    }

    fn render_multi_edit_controls(
        &mut self,
        ui: &mut Ui,
        selection: (AnimationTrackBinding, AnimationTrackKind, Vec<usize>),
    ) {
        let (binding, kind, indices) = selection;
        if indices.len() < 2 || matches!(kind, AnimationTrackKind::SpriteTimeline) {
            return;
        }
        ui.separator();
        ui.strong("Multi-Key Adjustments");
        ui.horizontal(|ui| {
            ui.label("Time Offset (s)");
            ui.add(egui::DragValue::new(&mut self.multi_time_offset).speed(0.01));
            if ui.button("Apply Time Offset").clicked() && self.multi_time_offset != 0.0 {
                self.pending_commands.push(AnimationPanelCommand::AdjustKeys {
                    binding,
                    indices: indices.clone(),
                    time_delta: Some(self.multi_time_offset),
                    value_delta: None,
                });
                self.multi_time_offset = 0.0;
            }
            if ui.button("Reset").clicked() {
                self.multi_time_offset = 0.0;
            }
        });
        match kind {
            AnimationTrackKind::Translation | AnimationTrackKind::Scale => {
                ui.horizontal(|ui| {
                    ui.label("Value Offset (X,Y)");
                    ui.add(egui::DragValue::new(&mut self.multi_vec2_offset[0]).speed(0.01));
                    ui.add(egui::DragValue::new(&mut self.multi_vec2_offset[1]).speed(0.01));
                    if ui.button("Apply Value Offset").clicked()
                        && (self.multi_vec2_offset[0] != 0.0 || self.multi_vec2_offset[1] != 0.0)
                    {
                        self.pending_commands.push(AnimationPanelCommand::AdjustKeys {
                            binding,
                            indices: indices.clone(),
                            time_delta: None,
                            value_delta: Some(KeyframeValue::Vec2(self.multi_vec2_offset)),
                        });
                        self.multi_vec2_offset = [0.0; 2];
                    }
                    if ui.button("Reset Vec2").clicked() {
                        self.multi_vec2_offset = [0.0; 2];
                    }
                });
            }
            AnimationTrackKind::Rotation => {
                ui.horizontal(|ui| {
                    ui.label("Value Offset (degrees)");
                    ui.add(egui::DragValue::new(&mut self.multi_scalar_offset).speed(0.1));
                    if ui.button("Apply Rotation Offset").clicked() && self.multi_scalar_offset != 0.0 {
                        self.pending_commands.push(AnimationPanelCommand::AdjustKeys {
                            binding,
                            indices: indices.clone(),
                            time_delta: None,
                            value_delta: Some(KeyframeValue::Scalar(self.multi_scalar_offset)),
                        });
                        self.multi_scalar_offset = 0.0;
                    }
                    if ui.button("Reset Scalar").clicked() {
                        self.multi_scalar_offset = 0.0;
                    }
                });
            }
            AnimationTrackKind::Tint => {
                ui.horizontal(|ui| {
                    ui.label("Value Offset (RGBA)");
                    for component in self.multi_vec4_offset.iter_mut() {
                        ui.add(egui::DragValue::new(component).speed(0.01));
                    }
                    if ui.button("Apply Tint Offset").clicked()
                        && self.multi_vec4_offset.iter().any(|&v| v != 0.0)
                    {
                        self.pending_commands.push(AnimationPanelCommand::AdjustKeys {
                            binding,
                            indices: indices.clone(),
                            time_delta: None,
                            value_delta: Some(KeyframeValue::Vec4(self.multi_vec4_offset)),
                        });
                        self.multi_vec4_offset = [0.0; 4];
                    }
                    if ui.button("Reset Tint Offset").clicked() {
                        self.multi_vec4_offset = [0.0; 4];
                    }
                });
            }
            AnimationTrackKind::SpriteTimeline => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_count_matches_summary_vector() {
        let animation_time = AnimationTime::default();
        let state = AnimationKeyframePanelState {
            animation_time: &animation_time,
            selected_entity: None,
            track_summaries: vec![
                AnimationTrackSummary {
                    id: AnimationTrackId(1),
                    label: "Sprite/Translation".to_string(),
                    kind: AnimationTrackKind::Translation,
                    binding: AnimationTrackBinding::TransformChannel {
                        entity: Entity::from_raw(1),
                        channel: AnimationTrackKind::Translation,
                    },
                    duration: 1.0,
                    key_count: 12,
                    interpolation: Some(ClipInterpolation::Linear),
                    playhead: Some(0.25),
                    dirty: false,
                    key_details: Vec::new(),
                },
                AnimationTrackSummary {
                    id: AnimationTrackId(2),
                    label: "Sprite/Rotation".to_string(),
                    kind: AnimationTrackKind::Rotation,
                    binding: AnimationTrackBinding::TransformChannel {
                        entity: Entity::from_raw(2),
                        channel: AnimationTrackKind::Rotation,
                    },
                    duration: 1.0,
                    key_count: 4,
                    interpolation: Some(ClipInterpolation::Linear),
                    playhead: None,
                    dirty: false,
                    key_details: Vec::new(),
                },
            ],
            can_undo: false,
            can_redo: false,
            status_message: None,
        };
        assert_eq!(state.track_summaries.len(), 2);
    }

    #[test]
    fn summary_with_details_keeps_metadata() {
        let summary = AnimationTrackSummary {
            id: AnimationTrackId(42),
            label: "Transform Clip".to_string(),
            kind: AnimationTrackKind::Translation,
            binding: AnimationTrackBinding::TransformChannel {
                entity: Entity::from_raw(3),
                channel: AnimationTrackKind::Translation,
            },
            duration: 2.5,
            key_count: 2,
            interpolation: Some(ClipInterpolation::Linear),
            playhead: Some(0.5),
            dirty: true,
            key_details: vec![
                KeyframeDetail {
                    id: KeyframeId::new(AnimationTrackId(42), 0),
                    index: 0,
                    time: Some(0.0),
                    value_preview: Some("Translation (0,0)".to_string()),
                    value: KeyframeValue::Vec2([0.0, 0.0]),
                },
                KeyframeDetail {
                    id: KeyframeId::new(AnimationTrackId(42), 1),
                    index: 1,
                    time: Some(1.0),
                    value_preview: Some("Rotation 90deg".to_string()),
                    value: KeyframeValue::Scalar(90.0),
                },
            ],
        };
        assert_eq!(summary.key_details.len(), 2);
        assert_eq!(summary.key_details[1].time, Some(1.0));
    }
}
