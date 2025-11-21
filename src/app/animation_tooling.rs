use super::*;

enum TrackEditOperation {
    Insert { time: f32, value: Option<KeyframeValue> },
    Delete { indices: Vec<usize> },
    Update { index: usize, new_time: Option<f32>, new_value: Option<KeyframeValue> },
    Adjust { indices: Vec<usize>, time_delta: Option<f32>, value_delta: Option<KeyframeValue> },
}

impl App {
    pub(super) fn show_animation_keyframe_panel(
        &mut self,
        ctx: &egui::Context,
        animation_time: &AnimationTime,
    ) {
        let panel_open = {
            let state = self.editor_ui_state();
            state.animation_keyframe_panel.is_open()
        };
        if !panel_open {
            return;
        }
        let panel_state = {
            let state = self.editor_ui_state();
            AnimationKeyframePanelState {
                animation_time,
                selected_entity: self.selected_entity(),
                track_summaries: self.collect_animation_track_summaries(),
                can_undo: !state.clip_edit_history.is_empty(),
                can_redo: !state.clip_edit_redo.is_empty(),
                status_message: state.animation_clip_status.clone(),
            }
        };
        self.with_editor_ui_state_mut(|state| {
            state.animation_keyframe_panel.render_window(ctx, panel_state);
        });
        self.process_animation_panel_commands();
    }

    fn collect_animation_track_summaries(&self) -> Vec<AnimationTrackSummary> {
        let mut summaries = Vec::new();
        if let Some(entity) = self.selected_entity() {
            if let Some(info) = self.ecs.entity_info(entity) {
                let mut slot_index = 0_u32;
                self.collect_sprite_track_summaries(entity, &info, &mut slot_index, &mut summaries);
                self.collect_transform_clip_summaries(entity, &info, &mut slot_index, &mut summaries);
            }
        }
        summaries
    }

    fn process_animation_panel_commands(&mut self) {
        let commands = self.with_editor_ui_state_mut(|state| state.animation_keyframe_panel.drain_commands());
        for command in commands {
            match command {
                AnimationPanelCommand::ScrubTrack { binding, time } => {
                    let track_kind = Self::analytics_track_kind(&binding);
                    self.handle_scrub_command(binding, time);
                    self.log_keyframe_editor_event(KeyframeEditorEventKind::Scrub { track: track_kind });
                }
                AnimationPanelCommand::InsertKey { binding, time, value } => {
                    let track_kind = Self::analytics_track_kind(&binding);
                    self.apply_track_edit(binding, TrackEditOperation::Insert { time, value });
                    self.log_keyframe_editor_event(KeyframeEditorEventKind::InsertKey { track: track_kind });
                }
                AnimationPanelCommand::DeleteKeys { binding, indices } => {
                    if !indices.is_empty() {
                        let track_kind = Self::analytics_track_kind(&binding);
                        let count = indices.len();
                        self.apply_track_edit(binding, TrackEditOperation::Delete { indices });
                        self.log_keyframe_editor_event(KeyframeEditorEventKind::DeleteKeys {
                            track: track_kind,
                            count,
                        });
                    }
                }
                AnimationPanelCommand::UpdateKey { binding, index, new_time, new_value } => {
                    let track_kind = Self::analytics_track_kind(&binding);
                    let changed_time = new_time.is_some();
                    let changed_value = new_value.is_some();
                    self.apply_track_edit(binding, TrackEditOperation::Update { index, new_time, new_value });
                    self.log_keyframe_editor_event(KeyframeEditorEventKind::UpdateKey {
                        track: track_kind,
                        changed_time,
                        changed_value,
                    });
                }
                AnimationPanelCommand::AdjustKeys { binding, indices, time_delta, value_delta } => {
                    if !indices.is_empty() {
                        let track_kind = Self::analytics_track_kind(&binding);
                        let count = indices.len();
                        let time_changed = time_delta.is_some();
                        let value_changed = value_delta.is_some();
                        self.apply_track_edit(
                            binding,
                            TrackEditOperation::Adjust { indices, time_delta, value_delta },
                        );
                        self.log_keyframe_editor_event(KeyframeEditorEventKind::AdjustKeys {
                            track: track_kind,
                            count,
                            time_delta: time_changed,
                            value_delta: value_changed,
                        });
                    }
                }
                AnimationPanelCommand::Undo => {
                    self.undo_clip_edit();
                    self.log_keyframe_editor_event(KeyframeEditorEventKind::Undo);
                }
                AnimationPanelCommand::Redo => {
                    self.redo_clip_edit();
                    self.log_keyframe_editor_event(KeyframeEditorEventKind::Redo);
                }
            }
        }
    }

    fn analytics_track_kind(binding: &AnimationTrackBinding) -> KeyframeEditorTrackKind {
        match binding {
            AnimationTrackBinding::SpriteTimeline { .. } => KeyframeEditorTrackKind::SpriteTimeline,
            AnimationTrackBinding::TransformChannel { channel, .. } => {
                Self::analytics_track_kind_from_channel(*channel)
            }
        }
    }

    fn analytics_track_kind_from_channel(channel: AnimationTrackKind) -> KeyframeEditorTrackKind {
        match channel {
            AnimationTrackKind::SpriteTimeline => KeyframeEditorTrackKind::SpriteTimeline,
            AnimationTrackKind::Translation => KeyframeEditorTrackKind::Translation,
            AnimationTrackKind::Rotation => KeyframeEditorTrackKind::Rotation,
            AnimationTrackKind::Scale => KeyframeEditorTrackKind::Scale,
            AnimationTrackKind::Tint => KeyframeEditorTrackKind::Tint,
        }
    }

    pub(super) fn log_keyframe_editor_event(&mut self, event: KeyframeEditorEventKind) {
        if let Some(analytics) = self.analytics_plugin_mut() {
            analytics.record_keyframe_editor_event(event);
        }
    }

    fn collect_sprite_track_summaries(
        &self,
        entity: Entity,
        info: &EntityInfo,
        slot_index: &mut u32,
        summaries: &mut Vec<AnimationTrackSummary>,
    ) {
        if let Some(sprite) = info.sprite.as_ref() {
            if let Some(animation) = sprite.animation.as_ref() {
                let track_id = AnimationTrackId::for_entity_slot(entity, *slot_index);
                *slot_index += 1;
                let timeline = self.assets.atlas_timeline(sprite.atlas.as_str(), animation.timeline.as_str());
                let duration = timeline
                    .map(|timeline| timeline.total_duration)
                    .unwrap_or(animation.frame_duration * animation.frame_count as f32);
                let key_count =
                    timeline.map(|timeline| timeline.frames.len()).unwrap_or(animation.frame_count);
                let playhead = timeline
                    .and_then(|timeline| {
                        if timeline.frames.is_empty() {
                            None
                        } else {
                            let clamped_index =
                                animation.frame_index.min(timeline.frames.len().saturating_sub(1));
                            let offset = timeline.frame_offsets.get(clamped_index).copied().unwrap_or(0.0);
                            Some((offset + animation.frame_elapsed).min(timeline.total_duration))
                        }
                    })
                    .or(Some(animation.frame_elapsed));
                summaries.push(AnimationTrackSummary {
                    id: track_id,
                    label: format!("Sprite Timeline ({})", animation.timeline),
                    kind: AnimationTrackKind::SpriteTimeline,
                    binding: AnimationTrackBinding::SpriteTimeline { entity },
                    duration,
                    key_count,
                    interpolation: None,
                    playhead,
                    dirty: false,
                    key_details: Self::sprite_key_details(track_id, animation, timeline),
                });
            }
        }
    }

    fn collect_transform_clip_summaries(
        &self,
        entity: Entity,
        info: &EntityInfo,
        slot_index: &mut u32,
        summaries: &mut Vec<AnimationTrackSummary>,
    ) {
        if let Some(clip) = info.transform_clip.as_ref() {
            let clip_asset = self.clip_resource(&clip.clip_key);
            let clip_dirty = {
                let state = self.editor_ui_state();
                state.clip_dirty.contains(&clip.clip_key)
            };
            if clip.has_translation {
                let track_id = AnimationTrackId::for_entity_slot(entity, *slot_index);
                *slot_index += 1;
                let (key_details, key_count, interpolation, duration) = if let Some(track_data) =
                    clip_asset.as_ref().and_then(|clip_asset| clip_asset.translation.as_ref())
                {
                    let details = Self::vec2_track_details(track_id, track_data);
                    (details, track_data.keyframes.len(), Some(track_data.interpolation), track_data.duration)
                } else {
                    let details = Self::transform_channel_details(
                        track_id,
                        clip.time,
                        clip.sample_translation
                            .map(|value| format!("Translation ({:.2}, {:.2})", value.x, value.y)),
                    );
                    let detail_count = details.len();
                    (details, detail_count, None, clip.duration)
                };
                summaries.push(AnimationTrackSummary {
                    id: track_id,
                    label: format!("Translation ({})", clip.clip_key),
                    kind: AnimationTrackKind::Translation,
                    binding: AnimationTrackBinding::TransformChannel {
                        entity,
                        channel: AnimationTrackKind::Translation,
                    },
                    duration,
                    key_count,
                    interpolation,
                    playhead: Some(clip.time),
                    dirty: clip_dirty,
                    key_details,
                });
            }
            if clip.has_rotation {
                let track_id = AnimationTrackId::for_entity_slot(entity, *slot_index);
                *slot_index += 1;
                let (key_details, key_count, interpolation, duration) = if let Some(track_data) =
                    clip_asset.as_ref().and_then(|clip_asset| clip_asset.rotation.as_ref())
                {
                    let details = Self::scalar_track_details(track_id, track_data);
                    (details, track_data.keyframes.len(), Some(track_data.interpolation), track_data.duration)
                } else {
                    let details = Self::transform_channel_details(
                        track_id,
                        clip.time,
                        clip.sample_rotation.map(|value| format!("Rotation {:.2}", value)),
                    );
                    let detail_count = details.len();
                    (details, detail_count, None, clip.duration)
                };
                summaries.push(AnimationTrackSummary {
                    id: track_id,
                    label: format!("Rotation ({})", clip.clip_key),
                    kind: AnimationTrackKind::Rotation,
                    binding: AnimationTrackBinding::TransformChannel {
                        entity,
                        channel: AnimationTrackKind::Rotation,
                    },
                    duration,
                    key_count,
                    interpolation,
                    playhead: Some(clip.time),
                    dirty: clip_dirty,
                    key_details,
                });
            }
            if clip.has_scale {
                let track_id = AnimationTrackId::for_entity_slot(entity, *slot_index);
                *slot_index += 1;
                let (key_details, key_count, interpolation, duration) = if let Some(track_data) =
                    clip_asset.as_ref().and_then(|clip_asset| clip_asset.scale.as_ref())
                {
                    let details = Self::vec2_track_details(track_id, track_data);
                    (details, track_data.keyframes.len(), Some(track_data.interpolation), track_data.duration)
                } else {
                    let details = Self::transform_channel_details(
                        track_id,
                        clip.time,
                        clip.sample_scale.map(|value| format!("Scale ({:.2}, {:.2})", value.x, value.y)),
                    );
                    let detail_count = details.len();
                    (details, detail_count, None, clip.duration)
                };
                summaries.push(AnimationTrackSummary {
                    id: track_id,
                    label: format!("Scale ({})", clip.clip_key),
                    kind: AnimationTrackKind::Scale,
                    binding: AnimationTrackBinding::TransformChannel {
                        entity,
                        channel: AnimationTrackKind::Scale,
                    },
                    duration,
                    key_count,
                    interpolation,
                    playhead: Some(clip.time),
                    dirty: clip_dirty,
                    key_details,
                });
            }
            if clip.has_tint {
                let track_id = AnimationTrackId::for_entity_slot(entity, *slot_index);
                *slot_index += 1;
                let (key_details, key_count, interpolation, duration) = if let Some(track_data) =
                    clip_asset.as_ref().and_then(|clip_asset| clip_asset.tint.as_ref())
                {
                    let details = Self::vec4_track_details(track_id, track_data);
                    (details, track_data.keyframes.len(), Some(track_data.interpolation), track_data.duration)
                } else {
                    let details = Self::transform_channel_details(
                        track_id,
                        clip.time,
                        clip.sample_tint.map(|value| {
                            format!("Tint ({:.2}, {:.2}, {:.2}, {:.2})", value.x, value.y, value.z, value.w)
                        }),
                    );
                    let detail_count = details.len();
                    (details, detail_count, None, clip.duration)
                };
                summaries.push(AnimationTrackSummary {
                    id: track_id,
                    label: format!("Tint ({})", clip.clip_key),
                    kind: AnimationTrackKind::Tint,
                    binding: AnimationTrackBinding::TransformChannel {
                        entity,
                        channel: AnimationTrackKind::Tint,
                    },
                    duration,
                    key_count,
                    interpolation,
                    playhead: Some(clip.time),
                    dirty: clip_dirty,
                    key_details,
                });
            }
        }
    }

    fn handle_scrub_command(&mut self, binding: AnimationTrackBinding, time: f32) {
        match binding {
            AnimationTrackBinding::SpriteTimeline { entity } => self.scrub_sprite_track(entity, time),
            AnimationTrackBinding::TransformChannel { entity, .. } => {
                self.scrub_transform_track(entity, time)
            }
        }
    }

    fn scrub_sprite_track(&mut self, entity: Entity, time: f32) {
        let Some(info) = self.ecs.entity_info(entity) else {
            return;
        };
        let Some(sprite) = info.sprite.as_ref() else {
            return;
        };
        let Some(animation) = sprite.animation.as_ref() else {
            return;
        };
        let Some(timeline) = self.assets.atlas_timeline(&sprite.atlas, animation.timeline.as_str()) else {
            return;
        };
        if timeline.frames.is_empty() {
            return;
        }
        let duration = timeline.total_duration.max(0.0);
        let wrapped = if timeline.looped && duration > 0.0 {
            let mut t = time % duration;
            if t < 0.0 {
                t += duration;
            }
            t
        } else {
            time.clamp(0.0, duration)
        };
        let mut target_index = timeline.frames.len() - 1;
        for (index, offset) in timeline.frame_offsets.iter().enumerate() {
            let span = timeline.durations.get(index).copied().unwrap_or(0.0).max(f32::EPSILON);
            if wrapped <= offset + span || index == timeline.frames.len() - 1 {
                target_index = index;
                break;
            }
        }
        let _ = self.ecs.set_sprite_animation_playing(entity, false);
        let _ = self.ecs.seek_sprite_animation_frame(entity, target_index);
    }

    fn scrub_transform_track(&mut self, entity: Entity, time: f32) {
        let Some(info) = self.ecs.entity_info(entity) else {
            return;
        };
        let Some(clip) = info.transform_clip.as_ref() else {
            return;
        };
        let clamped = time.clamp(0.0, clip.duration.max(0.0));
        let _ = self.ecs.set_transform_clip_playing(entity, false);
        let _ = self.ecs.set_transform_clip_time(entity, clamped);
    }

    fn apply_track_edit(&mut self, binding: AnimationTrackBinding, edit: TrackEditOperation) {
        match binding {
            AnimationTrackBinding::TransformChannel { entity, channel } => {
                self.edit_transform_channel(entity, channel, edit)
            }
            AnimationTrackBinding::SpriteTimeline { .. } => {}
        }
    }

    fn edit_transform_channel(
        &mut self,
        entity: Entity,
        channel: AnimationTrackKind,
        edit: TrackEditOperation,
    ) {
        let Some(info) = self.ecs.entity_info(entity) else {
            return;
        };
        let Some(clip_info) = info.transform_clip.as_ref() else {
            return;
        };
        let Some(source_clip) = self.clip_resource(&clip_info.clip_key) else {
            return;
        };
        let before_arc = Arc::clone(&source_clip);
        let mut clip = (*source_clip).clone();
        let mut dirty = false;
        match channel {
            AnimationTrackKind::Translation => {
                dirty = self.edit_vec2_track(
                    &mut clip.translation,
                    edit,
                    clip_info.sample_translation.or(Some(info.translation)),
                    Vec2::ZERO,
                );
            }
            AnimationTrackKind::Rotation => {
                dirty = self.edit_scalar_track(
                    &mut clip.rotation,
                    edit,
                    clip_info.sample_rotation.or(Some(info.rotation)),
                    0.0,
                );
            }
            AnimationTrackKind::Scale => {
                dirty = self.edit_vec2_track(
                    &mut clip.scale,
                    edit,
                    clip_info.sample_scale.or(Some(info.scale)),
                    Vec2::ONE,
                );
            }
            AnimationTrackKind::Tint => {
                dirty = self.edit_vec4_track(
                    &mut clip.tint,
                    edit,
                    clip_info.sample_tint.or(info.tint),
                    Vec4::ONE,
                );
            }
            AnimationTrackKind::SpriteTimeline => {}
        }
        if !dirty {
            return;
        }
        self.recompute_clip_duration(&mut clip);
        let clip_arc = Arc::new(clip);
        self.with_editor_ui_state_mut(|state| {
            state.clip_edit_overrides.insert(clip_info.clip_key.clone(), Arc::clone(&clip_arc));
        });
        self.apply_clip_override_to_instances(&clip_info.clip_key, Arc::clone(&clip_arc));
        self.record_clip_edit(&clip_info.clip_key, before_arc, Arc::clone(&clip_arc));
        self.persist_clip_edit(&clip_info.clip_key, clip_arc);
    }

    fn sprite_key_details(
        track_id: AnimationTrackId,
        animation: &SpriteAnimationInfo,
        timeline: Option<&SpriteTimeline>,
    ) -> Vec<KeyframeDetail> {
        if let Some(timeline) = timeline {
            timeline
                .frames
                .iter()
                .enumerate()
                .map(|(index, frame)| {
                    let time = timeline.frame_offsets.get(index).copied();
                    let duration = timeline.durations.get(index).copied().unwrap_or(0.0);
                    let mut preview = frame.name.as_ref().to_string();
                    if duration > 0.0 {
                        preview = format!("{preview} ({duration:.2}s)");
                    }
                    if !frame.events.is_empty() {
                        let events: Vec<String> =
                            frame.events.iter().map(|event| event.as_ref().to_string()).collect();
                        preview = format!("{preview} [{}]", events.join(", "));
                    }
                    KeyframeDetail {
                        id: KeyframeId::new(track_id, index),
                        index,
                        time,
                        value_preview: Some(preview),
                        value: KeyframeValue::None,
                    }
                })
                .collect()
        } else {
            (0..animation.frame_count)
                .map(|index| KeyframeDetail {
                    id: KeyframeId::new(track_id, index),
                    index,
                    time: if index == animation.frame_index { Some(animation.frame_elapsed) } else { None },
                    value_preview: animation.frame_region.clone(),
                    value: KeyframeValue::None,
                })
                .collect()
        }
    }

    fn vec2_track_details(track_id: AnimationTrackId, track: &ClipVec2Track) -> Vec<KeyframeDetail> {
        track
            .keyframes
            .iter()
            .enumerate()
            .map(|(index, keyframe)| KeyframeDetail {
                id: KeyframeId::new(track_id, index),
                index,
                time: Some(keyframe.time),
                value_preview: Some(format!("({:.2}, {:.2})", keyframe.value.x, keyframe.value.y)),
                value: KeyframeValue::Vec2([keyframe.value.x, keyframe.value.y]),
            })
            .collect()
    }

    fn scalar_track_details(track_id: AnimationTrackId, track: &ClipScalarTrack) -> Vec<KeyframeDetail> {
        track
            .keyframes
            .iter()
            .enumerate()
            .map(|(index, keyframe)| KeyframeDetail {
                id: KeyframeId::new(track_id, index),
                index,
                time: Some(keyframe.time),
                value_preview: Some(format!("{:.2}", keyframe.value)),
                value: KeyframeValue::Scalar(keyframe.value),
            })
            .collect()
    }

    fn vec4_track_details(track_id: AnimationTrackId, track: &ClipVec4Track) -> Vec<KeyframeDetail> {
        track
            .keyframes
            .iter()
            .enumerate()
            .map(|(index, keyframe)| {
                let value = keyframe.value;
                KeyframeDetail {
                    id: KeyframeId::new(track_id, index),
                    index,
                    time: Some(keyframe.time),
                    value_preview: Some(format!(
                        "({:.2}, {:.2}, {:.2}, {:.2})",
                        value.x, value.y, value.z, value.w
                    )),
                    value: KeyframeValue::Vec4([value.x, value.y, value.z, value.w]),
                }
            })
            .collect()
    }

    fn transform_channel_details(
        track_id: AnimationTrackId,
        time: f32,
        value: Option<String>,
    ) -> Vec<KeyframeDetail> {
        value
            .map(|preview| {
                vec![KeyframeDetail {
                    id: KeyframeId::new(track_id, 0),
                    index: 0,
                    time: Some(time),
                    value_preview: Some(preview),
                    value: KeyframeValue::None,
                }]
            })
            .unwrap_or_default()
    }

    fn clip_resource(&self, key: &str) -> Option<Arc<AnimationClip>> {
        if let Some(override_clip) = {
            let state = self.editor_ui_state();
            state.clip_edit_overrides.get(key).cloned()
        } {
            return Some(override_clip);
        }
        self.assets.clip(key).map(|clip| Arc::new(clip.clone()))
    }

    pub(super) fn apply_clip_override_to_instances(&mut self, clip_key: &str, clip: Arc<AnimationClip>) {
        let clip_key_arc: Arc<str> = Arc::from(clip_key.to_string());
        let mut query = self.ecs.world.query::<&mut ClipInstance>();
        for mut instance in query.iter_mut(&mut self.ecs.world) {
            if instance.clip_key.as_ref() == clip_key {
                instance.replace_clip(Arc::clone(&clip_key_arc), Arc::clone(&clip));
            }
        }
    }

    fn record_clip_edit(&mut self, clip_key: &str, before: Arc<AnimationClip>, after: Arc<AnimationClip>) {
        self.with_editor_ui_state_mut(|state| {
            state.clip_edit_history.push(ClipEditRecord { clip_key: clip_key.to_string(), before, after });
            state.clip_edit_redo.clear();
        });
    }

    fn undo_clip_edit(&mut self) {
        if let Some(record) = self.with_editor_ui_state_mut(|state| state.clip_edit_history.pop()) {
            self.with_editor_ui_state_mut(|state| state.clip_edit_redo.push(record.clone()));
            self.apply_clip_history_state(&record.clip_key, Arc::clone(&record.before));
            self.with_editor_ui_state_mut(|state| {
                state.animation_clip_status = Some(format!("Undid edit on '{}'", record.clip_key));
            });
        }
    }

    fn redo_clip_edit(&mut self) {
        if let Some(record) = self.with_editor_ui_state_mut(|state| state.clip_edit_redo.pop()) {
            let clip_key = record.clip_key.clone();
            self.with_editor_ui_state_mut(|state| state.clip_edit_history.push(record.clone()));
            self.apply_clip_history_state(&clip_key, Arc::clone(&record.after));
            self.with_editor_ui_state_mut(|state| {
                state.animation_clip_status = Some(format!("Redid edit on '{}'", clip_key));
            });
        }
    }

    fn apply_clip_history_state(&mut self, clip_key: &str, clip: Arc<AnimationClip>) {
        self.with_editor_ui_state_mut(|state| {
            state.clip_edit_overrides.insert(clip_key.to_string(), Arc::clone(&clip));
        });
        self.apply_clip_override_to_instances(clip_key, Arc::clone(&clip));
        self.persist_clip_edit(clip_key, clip);
    }

    fn persist_clip_edit(&mut self, clip_key: &str, clip: Arc<AnimationClip>) {
        self.with_editor_ui_state_mut(|state| {
            state.clip_dirty.insert(clip_key.to_string());
        });
        let clip_source_path = self.assets.clip_source(clip_key).map(|p| p.to_string());
        if let Some(path) = clip_source_path.as_deref() {
            self.suppress_validation_for_path(Path::new(path));
        }
        if let Err(err) = self.assets.save_clip(clip_key, clip.as_ref()) {
            eprintln!("[animation] failed to save clip '{clip_key}': {err:?}");
            self.with_editor_ui_state_mut(|state| {
                state.animation_clip_status = Some(format!("Failed to save '{clip_key}': {err}"));
            });
            return;
        }
        let mut status_note = format!("Saved clip '{clip_key}'");
        if let Some(path) = clip_source_path.as_deref() {
            if let Err(err) = self.assets.load_clip(clip_key, path) {
                eprintln!("[animation] failed to reload clip '{clip_key}' after save: {err:?}");
                self.with_editor_ui_state_mut(|state| {
                    state.animation_clip_status = Some(format!("Reload failed for '{clip_key}': {err}"));
                });
                return;
            }
        } else {
            status_note = format!("Saved clip '{clip_key}' (no source metadata available)");
        }
        if let Some(updated) = self.assets.clip(clip_key) {
            let canonical = Arc::new(updated.clone());
            self.apply_clip_override_to_instances(clip_key, Arc::clone(&canonical));
            self.with_editor_ui_state_mut(|state| {
                state.clip_edit_overrides.remove(clip_key);
            });
        }
        self.with_editor_ui_state_mut(|state| {
            state.clip_dirty.remove(clip_key);
        });
        if let Some(path) = clip_source_path {
            let path_buf = PathBuf::from(&path);
            let events = AnimationValidator::validate_path(path_buf.as_path());
            self.handle_validation_events("clip edit", path_buf.as_path(), events);
        }
        self.with_editor_ui_state_mut(|state| {
            state.animation_clip_status = Some(status_note);
        });
    }

    fn edit_vec2_track(
        &self,
        target: &mut Option<ClipVec2Track>,
        edit: TrackEditOperation,
        sample: Option<Vec2>,
        fallback: Vec2,
    ) -> bool {
        let interpolation =
            target.as_ref().map(|track| track.interpolation).unwrap_or(ClipInterpolation::Linear);
        let mut frames: Vec<ClipKeyframe<Vec2>> =
            target.as_ref().map(|track| track.keyframes.iter().copied().collect()).unwrap_or_default();
        match edit {
            TrackEditOperation::Insert { time, value } => {
                let insert_value = value
                    .and_then(|v| v.as_vec2())
                    .map(|arr| Vec2::new(arr[0], arr[1]))
                    .or(sample)
                    .unwrap_or(fallback);
                frames.push(ClipKeyframe { time, value: insert_value });
            }
            TrackEditOperation::Delete { indices } => Self::remove_key_indices(&mut frames, &indices),
            TrackEditOperation::Update { index, new_time, new_value } => {
                if frames.is_empty() || index >= frames.len() {
                    return false;
                }
                let mut changed = false;
                if let Some(time) = new_time {
                    let clamped = time.max(0.0);
                    if (frames[index].time - clamped).abs() > f32::EPSILON {
                        frames[index].time = clamped;
                        changed = true;
                    }
                }
                if let Some(KeyframeValue::Vec2(value)) = new_value {
                    let new_vec = Vec2::new(value[0], value[1]);
                    if frames[index].value != new_vec {
                        frames[index].value = new_vec;
                        changed = true;
                    }
                }
                if !changed {
                    return false;
                }
            }
            TrackEditOperation::Adjust { indices, time_delta, value_delta } => {
                if frames.is_empty() {
                    return false;
                }
                let mut changed = false;
                for index in indices {
                    if index >= frames.len() {
                        continue;
                    }
                    if let Some(delta) = time_delta {
                        let clamped = (frames[index].time + delta).max(0.0);
                        if (frames[index].time - clamped).abs() > f32::EPSILON {
                            frames[index].time = clamped;
                            changed = true;
                        }
                    }
                    if let Some(KeyframeValue::Vec2(offset)) = value_delta {
                        let offset_vec = Vec2::new(offset[0], offset[1]);
                        let new_value = frames[index].value + offset_vec;
                        if frames[index].value != new_value {
                            frames[index].value = new_value;
                            changed = true;
                        }
                    }
                }
                if !changed {
                    return false;
                }
            }
        }
        Self::apply_vec2_frames(target, frames, interpolation)
    }

    fn edit_scalar_track(
        &self,
        target: &mut Option<ClipScalarTrack>,
        edit: TrackEditOperation,
        sample: Option<f32>,
        fallback: f32,
    ) -> bool {
        let interpolation =
            target.as_ref().map(|track| track.interpolation).unwrap_or(ClipInterpolation::Linear);
        let mut frames: Vec<ClipKeyframe<f32>> =
            target.as_ref().map(|track| track.keyframes.iter().copied().collect()).unwrap_or_default();
        match edit {
            TrackEditOperation::Insert { time, value } => {
                let insert_value = value.and_then(|v| v.as_scalar()).or(sample).unwrap_or(fallback);
                frames.push(ClipKeyframe { time, value: insert_value });
            }
            TrackEditOperation::Delete { indices } => Self::remove_key_indices(&mut frames, &indices),
            TrackEditOperation::Update { index, new_time, new_value } => {
                if frames.is_empty() || index >= frames.len() {
                    return false;
                }
                let mut changed = false;
                if let Some(time) = new_time {
                    let clamped = time.max(0.0);
                    if (frames[index].time - clamped).abs() > f32::EPSILON {
                        frames[index].time = clamped;
                        changed = true;
                    }
                }
                if let Some(KeyframeValue::Scalar(value)) = new_value {
                    if (frames[index].value - value).abs() > f32::EPSILON {
                        frames[index].value = value;
                        changed = true;
                    }
                }
                if !changed {
                    return false;
                }
            }
            TrackEditOperation::Adjust { indices, time_delta, value_delta } => {
                if frames.is_empty() {
                    return false;
                }
                let mut changed = false;
                for index in indices {
                    if index >= frames.len() {
                        continue;
                    }
                    if let Some(delta) = time_delta {
                        let clamped = (frames[index].time + delta).max(0.0);
                        if (frames[index].time - clamped).abs() > f32::EPSILON {
                            frames[index].time = clamped;
                            changed = true;
                        }
                    }
                    if let Some(KeyframeValue::Scalar(offset)) = value_delta {
                        let new_value = frames[index].value + offset;
                        if (frames[index].value - new_value).abs() > f32::EPSILON {
                            frames[index].value = new_value;
                            changed = true;
                        }
                    }
                }
                if !changed {
                    return false;
                }
            }
        }
        Self::apply_scalar_frames(target, frames, interpolation)
    }

    fn edit_vec4_track(
        &self,
        target: &mut Option<ClipVec4Track>,
        edit: TrackEditOperation,
        sample: Option<Vec4>,
        fallback: Vec4,
    ) -> bool {
        let interpolation =
            target.as_ref().map(|track| track.interpolation).unwrap_or(ClipInterpolation::Linear);
        let mut frames: Vec<ClipKeyframe<Vec4>> =
            target.as_ref().map(|track| track.keyframes.iter().copied().collect()).unwrap_or_default();
        match edit {
            TrackEditOperation::Insert { time, value } => {
                let insert_value = value
                    .and_then(|v| v.as_vec4())
                    .map(|arr| Vec4::new(arr[0], arr[1], arr[2], arr[3]))
                    .or(sample)
                    .unwrap_or(fallback);
                frames.push(ClipKeyframe { time, value: insert_value });
            }
            TrackEditOperation::Delete { indices } => Self::remove_key_indices(&mut frames, &indices),
            TrackEditOperation::Update { index, new_time, new_value } => {
                if frames.is_empty() || index >= frames.len() {
                    return false;
                }
                let mut changed = false;
                if let Some(time) = new_time {
                    let clamped = time.max(0.0);
                    if (frames[index].time - clamped).abs() > f32::EPSILON {
                        frames[index].time = clamped;
                        changed = true;
                    }
                }
                if let Some(KeyframeValue::Vec4(value)) = new_value {
                    let new_vec = Vec4::new(value[0], value[1], value[2], value[3]);
                    if frames[index].value != new_vec {
                        frames[index].value = new_vec;
                        changed = true;
                    }
                }
                if !changed {
                    return false;
                }
            }
            TrackEditOperation::Adjust { indices, time_delta, value_delta } => {
                if frames.is_empty() {
                    return false;
                }
                let mut changed = false;
                for index in indices {
                    if index >= frames.len() {
                        continue;
                    }
                    if let Some(delta) = time_delta {
                        let clamped = (frames[index].time + delta).max(0.0);
                        if (frames[index].time - clamped).abs() > f32::EPSILON {
                            frames[index].time = clamped;
                            changed = true;
                        }
                    }
                    if let Some(KeyframeValue::Vec4(offset)) = value_delta {
                        let offset_vec = Vec4::new(offset[0], offset[1], offset[2], offset[3]);
                        let new_value = frames[index].value + offset_vec;
                        if frames[index].value != new_value {
                            frames[index].value = new_value;
                            changed = true;
                        }
                    }
                }
                if !changed {
                    return false;
                }
            }
        }
        Self::apply_vec4_frames(target, frames, interpolation)
    }

    fn remove_key_indices<T>(frames: &mut Vec<ClipKeyframe<T>>, indices: &[usize]) {
        if frames.is_empty() || indices.is_empty() {
            return;
        }
        let mut sorted = indices.to_vec();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        for index in sorted {
            if index < frames.len() {
                frames.remove(index);
            }
        }
    }

    fn apply_vec2_frames(
        target: &mut Option<ClipVec2Track>,
        frames: Vec<ClipKeyframe<Vec2>>,
        interpolation: ClipInterpolation,
    ) -> bool {
        if frames.is_empty() {
            let had_track = target.is_some();
            *target = None;
            return had_track;
        }
        let normalized = Self::normalize_keyframes(frames);
        let track = Self::build_vec2_track_from_frames(interpolation, normalized);
        let changed = target
            .as_ref()
            .map(|existing| {
                existing.keyframes.len() != track.keyframes.len() || existing.duration != track.duration
            })
            .unwrap_or(true);
        *target = Some(track);
        changed
    }

    fn apply_scalar_frames(
        target: &mut Option<ClipScalarTrack>,
        frames: Vec<ClipKeyframe<f32>>,
        interpolation: ClipInterpolation,
    ) -> bool {
        if frames.is_empty() {
            let had_track = target.is_some();
            *target = None;
            return had_track;
        }
        let normalized = Self::normalize_keyframes(frames);
        let track = Self::build_scalar_track_from_frames(interpolation, normalized);
        let changed = target
            .as_ref()
            .map(|existing| {
                existing.keyframes.len() != track.keyframes.len() || existing.duration != track.duration
            })
            .unwrap_or(true);
        *target = Some(track);
        changed
    }

    fn apply_vec4_frames(
        target: &mut Option<ClipVec4Track>,
        frames: Vec<ClipKeyframe<Vec4>>,
        interpolation: ClipInterpolation,
    ) -> bool {
        if frames.is_empty() {
            let had_track = target.is_some();
            *target = None;
            return had_track;
        }
        let normalized = Self::normalize_keyframes(frames);
        let track = Self::build_vec4_track_from_frames(interpolation, normalized);
        let changed = target
            .as_ref()
            .map(|existing| {
                existing.keyframes.len() != track.keyframes.len() || existing.duration != track.duration
            })
            .unwrap_or(true);
        *target = Some(track);
        changed
    }

    fn normalize_keyframes<T: Copy>(mut frames: Vec<ClipKeyframe<T>>) -> Vec<ClipKeyframe<T>> {
        if frames.is_empty() {
            return frames;
        }
        frames.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(Ordering::Equal));
        let mut normalized: Vec<ClipKeyframe<T>> = Vec::with_capacity(frames.len());
        for mut frame in frames {
            frame.time = frame.time.max(0.0);
            if let Some(last) = normalized.last_mut() {
                if (last.time - frame.time).abs() < 1e-4 {
                    *last = frame;
                    continue;
                }
            }
            normalized.push(frame);
        }
        normalized
    }

    fn build_vec2_track_from_frames(
        interpolation: ClipInterpolation,
        frames: Vec<ClipKeyframe<Vec2>>,
    ) -> ClipVec2Track {
        let duration = frames.last().map(|frame| frame.time).unwrap_or(0.0);
        let duration_inv = if duration > 0.0 { 1.0 / duration } else { 0.0 };
        let (segment_deltas, segments, segment_offsets) = Self::build_segment_cache_vec2(&frames);
        ClipVec2Track {
            interpolation,
            keyframes: Arc::from(frames.into_boxed_slice()),
            duration,
            duration_inv,
            segment_deltas,
            segments,
            segment_offsets,
        }
    }

    fn build_scalar_track_from_frames(
        interpolation: ClipInterpolation,
        frames: Vec<ClipKeyframe<f32>>,
    ) -> ClipScalarTrack {
        let duration = frames.last().map(|frame| frame.time).unwrap_or(0.0);
        let duration_inv = if duration > 0.0 { 1.0 / duration } else { 0.0 };
        let (segment_deltas, segments, segment_offsets) = Self::build_segment_cache_scalar(&frames);
        ClipScalarTrack {
            interpolation,
            keyframes: Arc::from(frames.into_boxed_slice()),
            duration,
            duration_inv,
            segment_deltas,
            segments,
            segment_offsets,
        }
    }

    fn build_vec4_track_from_frames(
        interpolation: ClipInterpolation,
        frames: Vec<ClipKeyframe<Vec4>>,
    ) -> ClipVec4Track {
        let duration = frames.last().map(|frame| frame.time).unwrap_or(0.0);
        let duration_inv = if duration > 0.0 { 1.0 / duration } else { 0.0 };
        let (segment_deltas, segments, segment_offsets) = Self::build_segment_cache_vec4(&frames);
        ClipVec4Track {
            interpolation,
            keyframes: Arc::from(frames.into_boxed_slice()),
            duration,
            duration_inv,
            segment_deltas,
            segments,
            segment_offsets,
        }
    }

    #[allow(clippy::type_complexity)]
    fn build_segment_cache_vec2(
        frames: &[ClipKeyframe<Vec2>],
    ) -> (Arc<[Vec2]>, Arc<[ClipSegment<Vec2>]>, Arc<[f32]>) {
        if frames.len() < 2 {
            return (Arc::from([]), Arc::from([]), Arc::from([]));
        }
        let mut deltas = Vec::with_capacity(frames.len() - 1);
        let mut segments = Vec::with_capacity(frames.len() - 1);
        let mut offsets = Vec::with_capacity(frames.len() - 1);
        for window in frames.windows(2) {
            let start = &window[0];
            let end = &window[1];
            let span = (end.time - start.time).max(f32::EPSILON);
            let inv_span = 1.0 / span;
            offsets.push(start.time);
            let delta = end.value - start.value;
            deltas.push(delta);
            segments.push(ClipSegment { slope: delta * inv_span, span, inv_span });
        }
        (
            Arc::from(deltas.into_boxed_slice()),
            Arc::from(segments.into_boxed_slice()),
            Arc::from(offsets.into_boxed_slice()),
        )
    }

    #[allow(clippy::type_complexity)]
    fn build_segment_cache_scalar(
        frames: &[ClipKeyframe<f32>],
    ) -> (Arc<[f32]>, Arc<[ClipSegment<f32>]>, Arc<[f32]>) {
        if frames.len() < 2 {
            return (Arc::from([]), Arc::from([]), Arc::from([]));
        }
        let mut deltas = Vec::with_capacity(frames.len() - 1);
        let mut segments = Vec::with_capacity(frames.len() - 1);
        let mut offsets = Vec::with_capacity(frames.len() - 1);
        for window in frames.windows(2) {
            let start = &window[0];
            let end = &window[1];
            let span = (end.time - start.time).max(f32::EPSILON);
            let inv_span = 1.0 / span;
            offsets.push(start.time);
            let delta = end.value - start.value;
            deltas.push(delta);
            segments.push(ClipSegment { slope: delta * inv_span, span, inv_span });
        }
        (
            Arc::from(deltas.into_boxed_slice()),
            Arc::from(segments.into_boxed_slice()),
            Arc::from(offsets.into_boxed_slice()),
        )
    }

    #[allow(clippy::type_complexity)]
    fn build_segment_cache_vec4(
        frames: &[ClipKeyframe<Vec4>],
    ) -> (Arc<[Vec4]>, Arc<[ClipSegment<Vec4>]>, Arc<[f32]>) {
        if frames.len() < 2 {
            return (Arc::from([]), Arc::from([]), Arc::from([]));
        }
        let mut deltas = Vec::with_capacity(frames.len() - 1);
        let mut segments = Vec::with_capacity(frames.len() - 1);
        let mut offsets = Vec::with_capacity(frames.len() - 1);
        for window in frames.windows(2) {
            let start = &window[0];
            let end = &window[1];
            let span = (end.time - start.time).max(f32::EPSILON);
            let inv_span = 1.0 / span;
            offsets.push(start.time);
            let delta = end.value - start.value;
            deltas.push(delta);
            segments.push(ClipSegment { slope: delta * inv_span, span, inv_span });
        }
        (
            Arc::from(deltas.into_boxed_slice()),
            Arc::from(segments.into_boxed_slice()),
            Arc::from(offsets.into_boxed_slice()),
        )
    }

    fn recompute_clip_duration(&self, clip: &mut AnimationClip) {
        let mut duration = 0.0_f32;
        if let Some(track) = clip.translation.as_ref() {
            duration = duration.max(track.duration);
        }
        if let Some(track) = clip.rotation.as_ref() {
            duration = duration.max(track.duration);
        }
        if let Some(track) = clip.scale.as_ref() {
            duration = duration.max(track.duration);
        }
        if let Some(track) = clip.tint.as_ref() {
            duration = duration.max(track.duration);
        }
        clip.duration = duration;
        clip.duration_inv = if duration > 0.0 { 1.0 / duration } else { 0.0 };
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::{SpriteAnimationFrame, SpriteAnimationLoopMode, SpriteFrameHotData, TransformClipInfo};
    use glam::{Vec2, Vec4};
    use std::sync::Arc;

    #[test]
    fn sprite_key_details_capture_active_frame() {
        let animation = SpriteAnimationInfo {
            timeline: "walk".to_string(),
            playing: true,
            looped: true,
            loop_mode: "Loop".to_string(),
            speed: 1.0,
            frame_index: 1,
            frame_count: 3,
            frame_elapsed: 0.25,
            frame_duration: 0.5,
            frame_region: Some("walk_01".to_string()),
            frame_region_id: Some(42),
            frame_uv: Some([0.0, 0.0, 0.5, 0.5]),
            frame_events: vec!["footstep".to_string()],
            start_offset: 0.0,
            random_start: false,
            group: Some("default".to_string()),
        };
        let track_id = AnimationTrackId::for_entity_slot(Entity::from_raw(1), 0);
        let details = App::sprite_key_details(track_id, &animation, None);
        assert_eq!(details.len(), animation.frame_count);
        assert_eq!(details[1].time, Some(animation.frame_elapsed));
        assert_eq!(details[0].value_preview.as_deref(), Some("walk_01"));
    }

    #[test]
    fn transform_clip_details_reflect_channels() {
        let clip = TransformClipInfo {
            clip_key: "transform_clip".to_string(),
            playing: true,
            looped: false,
            speed: 1.0,
            time: 0.5,
            duration: 2.0,
            group: None,
            has_translation: true,
            has_rotation: true,
            has_scale: false,
            has_tint: true,
            sample_translation: Some(Vec2::new(1.0, 2.0)),
            sample_rotation: Some(45.0),
            sample_scale: None,
            sample_tint: Some(Vec4::new(0.1, 0.2, 0.3, 0.9)),
        };
        let track_id = AnimationTrackId::for_entity_slot(Entity::from_raw(1), 1);
        let details = App::transform_channel_details(
            track_id,
            clip.time,
            clip.sample_tint.map(|value| {
                format!("Tint ({:.2}, {:.2}, {:.2}, {:.2})", value.x, value.y, value.z, value.w)
            }),
        );
        assert_eq!(details.len(), 1);
        assert_eq!(details[0].time, Some(clip.time));
        assert!(details[0].value_preview.as_ref().unwrap().contains("Tint"));
    }

    #[test]
    fn sprite_key_details_use_timeline_offsets() {
        let animation = SpriteAnimationInfo {
            timeline: "run".to_string(),
            playing: true,
            looped: true,
            loop_mode: "Loop".to_string(),
            speed: 1.0,
            frame_index: 1,
            frame_count: 2,
            frame_elapsed: 0.2,
            frame_duration: 0.4,
            frame_region: Some("run_01".to_string()),
            frame_region_id: Some(1),
            frame_uv: Some([0.0, 0.0, 1.0, 1.0]),
            frame_events: Vec::new(),
            start_offset: 0.0,
            random_start: false,
            group: None,
        };
        let frames = vec![
            SpriteAnimationFrame {
                name: Arc::from("run_00"),
                region: Arc::from("run_00"),
                region_id: 0,
                duration: 0.5,
                uv: [0.0; 4],
                events: Arc::from(Vec::new()),
            },
            SpriteAnimationFrame {
                name: Arc::from("run_01"),
                region: Arc::from("run_01"),
                region_id: 1,
                duration: 0.75,
                uv: [0.0; 4],
                events: Arc::from(Vec::new()),
            },
        ];
        let hot_frames = vec![
            SpriteFrameHotData { region_id: 0, uv: [0.0; 4] },
            SpriteFrameHotData { region_id: 1, uv: [0.0; 4] },
        ];
        let timeline = SpriteTimeline {
            name: Arc::from("run"),
            looped: true,
            loop_mode: SpriteAnimationLoopMode::Loop,
            frames: Arc::from(frames),
            hot_frames: Arc::from(hot_frames),
            durations: Arc::from(vec![0.5, 0.75].into_boxed_slice()),
            frame_offsets: Arc::from(vec![0.0, 0.5].into_boxed_slice()),
            total_duration: 1.25,
            total_duration_inv: 0.8,
        };
        let track_id = AnimationTrackId::for_entity_slot(Entity::from_raw(2), 0);
        let details = App::sprite_key_details(track_id, &animation, Some(&timeline));
        assert_eq!(details.len(), 2);
        assert_eq!(details[0].time, Some(0.0));
        assert_eq!(details[1].time, Some(0.5));
        assert!(details[1].value_preview.as_ref().unwrap().contains("0.75"));
    }
}
