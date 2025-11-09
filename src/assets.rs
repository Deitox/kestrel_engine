use crate::ecs::{SpriteAnimationFrame, SpriteAnimationLoopMode};
use anyhow::{anyhow, Result};
use glam::{Vec2, Vec4};
use serde::Deserialize;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub mod skeletal;

pub struct AssetManager {
    atlases: HashMap<String, TextureAtlas>,
    clips: HashMap<String, AnimationClip>,
    skeletons: HashMap<String, Arc<skeletal::SkeletonAsset>>,
    skeletal_clips: HashMap<String, Arc<skeletal::SkeletalClip>>,
    sampler: Option<wgpu::Sampler>,
    device: Option<wgpu::Device>,
    queue: Option<wgpu::Queue>,
    texture_cache: HashMap<PathBuf, (wgpu::TextureView, (u32, u32))>,
    atlas_sources: HashMap<String, String>,
    atlas_refs: HashMap<String, usize>,
    clip_sources: HashMap<String, String>,
    clip_refs: HashMap<String, usize>,
    skeleton_sources: HashMap<String, String>,
    skeleton_refs: HashMap<String, usize>,
    skeletal_clip_sources: HashMap<String, String>,
    skeleton_clip_index: HashMap<String, Vec<String>>,
}

fn build_vec2_track(raw: ClipVec2TrackFile) -> Result<(ClipVec2Track, f32)> {
    if raw.keyframes.is_empty() {
        return Err(anyhow!("Clip vec2 track must contain at least one keyframe"));
    }
    let interpolation = convert_interpolation(raw.interpolation);
    let (keyframes, duration) = build_keyframes(raw.keyframes, |kf| {
        let value = Vec2::new(kf.value[0], kf.value[1]);
        if !value.is_finite() {
            return Err(anyhow!("Clip keyframe contains non-finite translation/scale value"));
        }
        Ok(ClipKeyframe { time: kf.time, value })
    })?;
    let (segment_deltas, segments, segment_offsets) = build_segment_cache_vec2(keyframes.as_ref());
    Ok((
        ClipVec2Track {
            interpolation,
            keyframes,
            duration,
            duration_inv: if duration > 0.0 { 1.0 / duration } else { 0.0 },
            segment_deltas,
            segments,
            segment_offsets,
        },
        duration,
    ))
}

fn build_scalar_track(raw: ClipScalarTrackFile) -> Result<(ClipScalarTrack, f32)> {
    if raw.keyframes.is_empty() {
        return Err(anyhow!("Clip scalar track must contain at least one keyframe"));
    }
    let interpolation = convert_interpolation(raw.interpolation);
    let (keyframes, duration) = build_keyframes(raw.keyframes, |kf| {
        if !kf.value.is_finite() {
            return Err(anyhow!("Clip keyframe contains non-finite rotation value"));
        }
        Ok(ClipKeyframe { time: kf.time, value: kf.value })
    })?;
    let (segment_deltas, segments, segment_offsets) = build_segment_cache_scalar(keyframes.as_ref());
    Ok((
        ClipScalarTrack {
            interpolation,
            keyframes,
            duration,
            duration_inv: if duration > 0.0 { 1.0 / duration } else { 0.0 },
            segment_deltas,
            segments,
            segment_offsets,
        },
        duration,
    ))
}

fn build_vec4_track(raw: ClipVec4TrackFile) -> Result<(ClipVec4Track, f32)> {
    if raw.keyframes.is_empty() {
        return Err(anyhow!("Clip vec4 track must contain at least one keyframe"));
    }
    let interpolation = convert_interpolation(raw.interpolation);
    let (keyframes, duration) = build_keyframes(raw.keyframes, |kf| {
        let value = Vec4::new(kf.value[0], kf.value[1], kf.value[2], kf.value[3]);
        if !value.is_finite() {
            return Err(anyhow!("Clip keyframe contains non-finite tint value"));
        }
        Ok(ClipKeyframe { time: kf.time, value })
    })?;
    let (segment_deltas, segments, segment_offsets) = build_segment_cache_vec4(keyframes.as_ref());
    Ok((
        ClipVec4Track {
            interpolation,
            keyframes,
            duration,
            duration_inv: if duration > 0.0 { 1.0 / duration } else { 0.0 },
            segment_deltas,
            segments,
            segment_offsets,
        },
        duration,
    ))
}

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
        let span = (end.time - start.time).max(std::f32::EPSILON);
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
        let span = (end.time - start.time).max(std::f32::EPSILON);
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
        let span = (end.time - start.time).max(std::f32::EPSILON);
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

fn build_keyframes<T, F, R>(raw_frames: Vec<R>, mut convert: F) -> Result<(Arc<[ClipKeyframe<T>]>, f32)>
where
    T: Clone,
    F: FnMut(R) -> Result<ClipKeyframe<T>>,
{
    let mut frames: Vec<(usize, ClipKeyframe<T>)> = Vec::new();
    for (index, raw) in raw_frames.into_iter().enumerate() {
        let frame = convert(raw)?;
        if !frame.time.is_finite() {
            return Err(anyhow!("Clip keyframe time must be finite"));
        }
        if frame.time < 0.0 {
            return Err(anyhow!("Clip keyframe time cannot be negative"));
        }
        frames.push((index, frame));
    }
    frames.sort_by(|a, b| {
        let time_order = a.1.time.partial_cmp(&b.1.time).unwrap_or(Ordering::Equal);
        if time_order == Ordering::Equal {
            a.0.cmp(&b.0)
        } else {
            time_order
        }
    });
    let mut deduped: Vec<ClipKeyframe<T>> = Vec::with_capacity(frames.len());
    for (_, frame) in frames {
        if let Some(last) = deduped.last_mut() {
            if (frame.time - last.time).abs() <= f32::EPSILON {
                *last = frame;
                continue;
            }
        }
        deduped.push(frame);
    }
    let duration = deduped.last().map(|kf| kf.time).unwrap_or(0.0);
    let arc = Arc::<[ClipKeyframe<T>]>::from(deduped.into_boxed_slice());
    Ok((arc, duration))
}

fn convert_interpolation(file: ClipInterpolationFile) -> ClipInterpolation {
    match file {
        ClipInterpolationFile::Linear => ClipInterpolation::Linear,
        ClipInterpolationFile::Step => ClipInterpolation::Step,
    }
}

#[derive(Clone)]
pub struct TextureAtlas {
    pub image_key: String,
    pub image_path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub regions: HashMap<Arc<str>, AtlasRegion>,
    pub animations: HashMap<String, SpriteTimeline>,
}

#[derive(Clone)]
pub struct AtlasRegion {
    pub id: u16,
    pub rect: Rect,
    pub uv: [f32; 4],
}

#[derive(Clone)]
pub struct SpriteTimeline {
    pub name: Arc<str>,
    pub looped: bool,
    pub loop_mode: SpriteAnimationLoopMode,
    pub frames: Arc<[SpriteAnimationFrame]>,
    pub durations: Arc<[f32]>,
    pub frame_offsets: Arc<[f32]>,
    pub total_duration: f32,
    pub total_duration_inv: f32,
}

#[derive(Clone)]
pub struct AnimationClip {
    pub name: Arc<str>,
    pub duration: f32,
    pub duration_inv: f32,
    pub translation: Option<ClipVec2Track>,
    pub rotation: Option<ClipScalarTrack>,
    pub scale: Option<ClipVec2Track>,
    pub tint: Option<ClipVec4Track>,
    pub looped: bool,
    pub version: u32,
}

#[derive(Clone, Copy)]
pub struct ClipSegment<T: Copy> {
    pub slope: T,
    pub span: f32,
    pub inv_span: f32,
}

impl<T: Copy> ClipSegment<T> {
    #[inline(always)]
    pub fn span(&self) -> f32 {
        self.span
    }

    #[inline(always)]
    pub fn inv_span(&self) -> f32 {
        self.inv_span
    }
}

#[derive(Clone)]
pub struct ClipVec2Track {
    pub interpolation: ClipInterpolation,
    pub keyframes: Arc<[ClipKeyframe<Vec2>]>,
    pub duration: f32,
    pub duration_inv: f32,
    pub segment_deltas: Arc<[Vec2]>,
    pub segments: Arc<[ClipSegment<Vec2>]>,
    pub segment_offsets: Arc<[f32]>,
}

#[derive(Clone)]
pub struct ClipScalarTrack {
    pub interpolation: ClipInterpolation,
    pub keyframes: Arc<[ClipKeyframe<f32>]>,
    pub duration: f32,
    pub duration_inv: f32,
    pub segment_deltas: Arc<[f32]>,
    pub segments: Arc<[ClipSegment<f32>]>,
    pub segment_offsets: Arc<[f32]>,
}

#[derive(Clone)]
pub struct ClipVec4Track {
    pub interpolation: ClipInterpolation,
    pub keyframes: Arc<[ClipKeyframe<Vec4>]>,
    pub duration: f32,
    pub duration_inv: f32,
    pub segment_deltas: Arc<[Vec4]>,
    pub segments: Arc<[ClipSegment<Vec4>]>,
    pub segment_offsets: Arc<[f32]>,
}

#[derive(Clone)]
pub struct ClipKeyframe<T> {
    pub time: f32,
    pub value: T,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ClipInterpolation {
    Step,
    Linear,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}
#[derive(Deserialize)]
struct AtlasFile {
    image: String,
    width: u32,
    height: u32,
    regions: HashMap<String, Rect>,
    #[serde(default)]
    animations: HashMap<String, AtlasTimelineFile>,
}

#[derive(Debug, Deserialize)]
struct AtlasTimelineFile {
    #[serde(default)]
    frames: Vec<AtlasTimelineFrameFile>,
    #[serde(default = "default_timeline_loop")]
    looped: bool,
    #[serde(default)]
    loop_mode: Option<String>,
    #[serde(default)]
    events: Vec<AtlasTimelineEventFile>,
}

#[derive(Debug, Deserialize)]
struct AtlasTimelineFrameFile {
    #[serde(default)]
    name: Option<String>,
    region: String,
    #[serde(default = "default_frame_duration_ms")]
    duration_ms: u32,
}

#[derive(Debug, Deserialize)]
struct AtlasTimelineEventFile {
    frame: usize,
    name: String,
}

#[derive(Debug, Deserialize)]
struct ClipFile {
    version: u32,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    looped: bool,
    #[serde(default)]
    tracks: ClipTracksFile,
}

#[derive(Debug, Default, Deserialize)]
struct ClipTracksFile {
    #[serde(default)]
    translation: Option<ClipVec2TrackFile>,
    #[serde(default)]
    rotation: Option<ClipScalarTrackFile>,
    #[serde(default)]
    scale: Option<ClipVec2TrackFile>,
    #[serde(default)]
    tint: Option<ClipVec4TrackFile>,
}

#[derive(Debug, Deserialize)]
struct ClipVec2TrackFile {
    #[serde(default = "default_clip_interpolation")]
    interpolation: ClipInterpolationFile,
    keyframes: Vec<ClipVec2KeyframeFile>,
}

#[derive(Debug, Deserialize)]
struct ClipScalarTrackFile {
    #[serde(default = "default_clip_interpolation")]
    interpolation: ClipInterpolationFile,
    keyframes: Vec<ClipScalarKeyframeFile>,
}

#[derive(Debug, Deserialize)]
struct ClipVec4TrackFile {
    #[serde(default = "default_clip_interpolation")]
    interpolation: ClipInterpolationFile,
    keyframes: Vec<ClipVec4KeyframeFile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ClipInterpolationFile {
    Linear,
    Step,
}

#[derive(Debug, Deserialize)]
struct ClipVec2KeyframeFile {
    time: f32,
    value: [f32; 2],
}

#[derive(Debug, Deserialize)]
struct ClipScalarKeyframeFile {
    time: f32,
    value: f32,
}

#[derive(Debug, Deserialize)]
struct ClipVec4KeyframeFile {
    time: f32,
    value: [f32; 4],
}

const fn default_timeline_loop() -> bool {
    true
}

const fn default_frame_duration_ms() -> u32 {
    100
}

fn default_clip_interpolation() -> ClipInterpolationFile {
    ClipInterpolationFile::Linear
}

impl AssetManager {
    pub fn new() -> Self {
        Self {
            atlases: HashMap::new(),
            clips: HashMap::new(),
            skeletons: HashMap::new(),
            skeletal_clips: HashMap::new(),
            sampler: None,
            device: None,
            queue: None,
            texture_cache: HashMap::new(),
            atlas_sources: HashMap::new(),
            atlas_refs: HashMap::new(),
            clip_sources: HashMap::new(),
            clip_refs: HashMap::new(),
            skeleton_sources: HashMap::new(),
            skeleton_refs: HashMap::new(),
            skeletal_clip_sources: HashMap::new(),
            skeleton_clip_index: HashMap::new(),
        }
    }
    pub fn set_device(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.device = Some(device.clone());
        self.queue = Some(queue.clone());
        self.sampler = Some(device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Default Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        }));
        self.texture_cache.clear();
    }
    pub fn default_sampler(&self) -> &wgpu::Sampler {
        self.sampler.as_ref().expect("sampler")
    }
    pub fn load_atlas(&mut self, key: &str, json_path: &str) -> Result<()> {
        self.load_atlas_internal(key, json_path)?;
        Ok(())
    }
    fn load_atlas_internal(&mut self, key: &str, json_path: &str) -> Result<()> {
        let bytes = fs::read(json_path)?;
        let af: AtlasFile = serde_json::from_slice(&bytes)?;
        let mut regions = HashMap::new();
        let image_path = resolve_atlas_image_path(json_path, &af.image);
        for (index, (name, rect)) in af.regions.into_iter().enumerate() {
            let id =
                u16::try_from(index).map_err(|_| anyhow!("Atlas '{key}' has more than 65535 regions"))?;
            let name_arc: Arc<str> = Arc::from(name);
            let uv = [
                rect.x as f32 / af.width as f32,
                rect.y as f32 / af.height as f32,
                (rect.x + rect.w) as f32 / af.width as f32,
                (rect.y + rect.h) as f32 / af.height as f32,
            ];
            regions.insert(Arc::clone(&name_arc), AtlasRegion { id, rect, uv });
        }
        let animations = Self::parse_timelines(key, &regions, af.animations);
        let atlas = TextureAtlas {
            image_key: af.image.clone(),
            image_path: image_path.clone(),
            width: af.width,
            height: af.height,
            regions,
            animations,
        };
        self.atlases.insert(key.to_string(), atlas);
        self.atlas_sources.insert(key.to_string(), json_path.to_string());
        Ok(())
    }

    fn parse_timelines(
        atlas_key: &str,
        regions: &HashMap<Arc<str>, AtlasRegion>,
        raw: HashMap<String, AtlasTimelineFile>,
    ) -> HashMap<String, SpriteTimeline> {
        let mut animations = HashMap::new();
        for (timeline_key, mut data) in raw {
            let mut frames = Vec::new();
            let mut durations = Vec::new();
            let mut offsets = Vec::new();
            let mut event_map: HashMap<usize, Vec<String>> = HashMap::new();
            for event in data.events.drain(..) {
                event_map.entry(event.frame).or_default().push(event.name);
            }
            let mut accumulated = 0.0_f32;
            for (frame_index, frame) in data.frames.into_iter().enumerate() {
                let Some((region_key, region_info)) = regions.get_key_value(frame.region.as_str()) else {
                    eprintln!(
                        "[assets] atlas '{atlas_key}': timeline '{timeline_key}' references unknown region '{}', skipping frame.",
                        frame.region
                    );
                    continue;
                };
                let frame_name_arc =
                    frame.name.map(Arc::<str>::from).unwrap_or_else(|| Arc::clone(region_key));
                let duration = (frame.duration_ms.max(1) as f32) / 1000.0;
                let event_names = event_map.remove(&frame_index).unwrap_or_default();
                let events: Vec<Arc<str>> =
                    event_names.into_iter().map(|name| Arc::<str>::from(name)).collect();
                offsets.push(accumulated);
                frames.push(SpriteAnimationFrame {
                    name: frame_name_arc,
                    region: Arc::clone(region_key),
                    region_id: region_info.id,
                    duration,
                    uv: region_info.uv,
                    events: Arc::from(events),
                });
                durations.push(duration);
                accumulated += duration;
            }
            if frames.is_empty() {
                eprintln!(
                    "[assets] atlas '{atlas_key}': timeline '{timeline_key}' has no valid frames, ignoring."
                );
                continue;
            }
            let mode_str = data.loop_mode.clone().unwrap_or_else(|| {
                if data.looped {
                    "loop".to_string()
                } else {
                    "once_stop".to_string()
                }
            });
            let mode_enum = SpriteAnimationLoopMode::from_str(&mode_str);
            let looped = mode_enum.looped();
            for (frame, names) in event_map {
                eprintln!(
                    "[assets] atlas '{atlas_key}': timeline '{timeline_key}' has events {:?} referencing missing frame index {}.",
                    names, frame
                );
            }
            let timeline_arc = Arc::<str>::from(timeline_key.clone());
            animations.insert(
                timeline_key.clone(),
                SpriteTimeline {
                    name: timeline_arc,
                    looped,
                    loop_mode: mode_enum,
                    frames: Arc::from(frames),
                    durations: Arc::from(durations),
                    frame_offsets: Arc::from(offsets.into_boxed_slice()),
                    total_duration: accumulated,
                    total_duration_inv: if accumulated > 0.0 { 1.0 / accumulated } else { 0.0 },
                },
            );
        }
        animations
    }
    pub fn load_clip(&mut self, key: &str, json_path: &str) -> Result<()> {
        self.load_clip_internal(key, json_path)
    }

    fn load_clip_internal(&mut self, key: &str, json_path: &str) -> Result<()> {
        let bytes = fs::read(json_path)?;
        let clip_file: ClipFile = serde_json::from_slice(&bytes)?;
        if clip_file.version == 0 {
            return Err(anyhow!("Clip '{key}' has unsupported version 0 (expected >= 1) in {}", json_path));
        }
        let interpolation_translation = clip_file.tracks.translation;
        let interpolation_rotation = clip_file.tracks.rotation;
        let interpolation_scale = clip_file.tracks.scale;
        let interpolation_tint = clip_file.tracks.tint;

        let mut duration = 0.0_f32;

        let translation = if let Some(track) = interpolation_translation {
            let (parsed, track_duration) = build_vec2_track(track)?;
            duration = duration.max(track_duration);
            Some(parsed)
        } else {
            None
        };
        let rotation = if let Some(track) = interpolation_rotation {
            let (parsed, track_duration) = build_scalar_track(track)?;
            duration = duration.max(track_duration);
            Some(parsed)
        } else {
            None
        };
        let scale = if let Some(track) = interpolation_scale {
            let (parsed, track_duration) = build_vec2_track(track)?;
            duration = duration.max(track_duration);
            Some(parsed)
        } else {
            None
        };
        let tint = if let Some(track) = interpolation_tint {
            let (parsed, track_duration) = build_vec4_track(track)?;
            duration = duration.max(track_duration);
            Some(parsed)
        } else {
            None
        };

        let duration = if duration <= 0.0 { 0.0 } else { duration };
        let name = clip_file.name.unwrap_or_else(|| key.to_string());
        let clip = AnimationClip {
            name: Arc::from(name),
            duration,
            duration_inv: if duration > 0.0 { 1.0 / duration } else { 0.0 },
            translation,
            rotation,
            scale,
            tint,
            looped: clip_file.looped,
            version: clip_file.version,
        };
        self.clips.insert(key.to_string(), clip);
        self.clip_sources.insert(key.to_string(), json_path.to_string());
        Ok(())
    }
    pub fn retain_atlas(&mut self, key: &str, json_path: Option<&str>) -> Result<()> {
        if self.atlases.contains_key(key) {
            *self.atlas_refs.entry(key.to_string()).or_insert(0) += 1;
            if let Some(path) = json_path {
                self.atlas_sources.insert(key.to_string(), path.to_string());
            }
            return Ok(());
        }
        let path_owned = if let Some(path) = json_path {
            path.to_string()
        } else if let Some(stored) = self.atlas_sources.get(key) {
            stored.clone()
        } else {
            return Err(anyhow!("Atlas '{key}' is not loaded and no JSON path provided to retain it."));
        };
        self.load_atlas_internal(key, &path_owned)?;
        self.atlas_sources.insert(key.to_string(), path_owned);
        self.atlas_refs.insert(key.to_string(), 1);
        Ok(())
    }
    pub fn atlas_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.atlases.keys().cloned().collect();
        keys.sort();
        keys
    }
    pub fn retain_clip(&mut self, key: &str, json_path: Option<&str>) -> Result<()> {
        if self.clips.contains_key(key) {
            *self.clip_refs.entry(key.to_string()).or_insert(0) += 1;
            if let Some(path) = json_path {
                self.clip_sources.insert(key.to_string(), path.to_string());
            }
            return Ok(());
        }
        let path_owned = if let Some(path) = json_path {
            path.to_string()
        } else if let Some(stored) = self.clip_sources.get(key) {
            stored.clone()
        } else {
            return Err(anyhow!("Clip '{key}' is not loaded and no JSON path provided to retain it."));
        };
        self.load_clip_internal(key, &path_owned)?;
        self.clip_sources.insert(key.to_string(), path_owned);
        self.clip_refs.insert(key.to_string(), 1);
        Ok(())
    }
    pub fn release_clip(&mut self, key: &str) -> bool {
        if let Some(count) = self.clip_refs.get_mut(key) {
            if *count > 0 {
                *count -= 1;
                if *count == 0 {
                    self.clip_refs.remove(key);
                    self.clips.remove(key);
                    self.clip_sources.remove(key);
                }
                return true;
            }
        }
        false
    }
    pub fn clip_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.clips.keys().cloned().collect();
        keys.sort();
        keys
    }
    pub fn clip(&self, key: &str) -> Option<&AnimationClip> {
        self.clips.get(key)
    }
    pub fn clip_source(&self, key: &str) -> Option<&str> {
        self.clip_sources.get(key).map(|s| s.as_str())
    }
    fn load_skeleton_internal(&mut self, key: &str, gltf_path: &str) -> Result<()> {
        let skeletal::SkeletonImport { skeleton, clips } = skeletal::load_skeleton_from_gltf(gltf_path)?;
        self.skeletons.insert(key.to_string(), Arc::new(skeleton));
        if let Some(existing) = self.skeleton_clip_index.remove(key) {
            for clip_key in existing {
                self.skeletal_clips.remove(&clip_key);
                self.skeletal_clip_sources.remove(&clip_key);
            }
        }
        let mut clip_keys: Vec<String> = Vec::new();
        for clip in clips {
            let clip_key = format!("{key}::{}", clip.name.as_ref());
            self.skeletal_clip_sources.insert(clip_key.clone(), gltf_path.to_string());
            self.skeletal_clips.insert(clip_key.clone(), Arc::new(clip));
            clip_keys.push(clip_key);
        }
        self.skeleton_clip_index.insert(key.to_string(), clip_keys);
        Ok(())
    }
    pub fn retain_skeleton(&mut self, key: &str, gltf_path: Option<&str>) -> Result<()> {
        if self.skeletons.contains_key(key) {
            *self.skeleton_refs.entry(key.to_string()).or_insert(0) += 1;
            if let Some(path) = gltf_path {
                self.skeleton_sources.insert(key.to_string(), path.to_string());
            }
            return Ok(());
        }
        let path_owned = if let Some(path) = gltf_path {
            path.to_string()
        } else if let Some(stored) = self.skeleton_sources.get(key) {
            stored.clone()
        } else {
            return Err(anyhow!("Skeleton '{key}' is not loaded and no GLTF path provided to retain it."));
        };
        self.load_skeleton_internal(key, &path_owned)?;
        self.skeleton_sources.insert(key.to_string(), path_owned);
        self.skeleton_refs.insert(key.to_string(), 1);
        Ok(())
    }
    pub fn release_skeleton(&mut self, key: &str) -> bool {
        if let Some(count) = self.skeleton_refs.get_mut(key) {
            if *count > 0 {
                *count -= 1;
                if *count == 0 {
                    self.skeleton_refs.remove(key);
                    self.skeletons.remove(key);
                    self.skeleton_sources.remove(key);
                    if let Some(clip_keys) = self.skeleton_clip_index.remove(key) {
                        for clip_key in clip_keys {
                            self.skeletal_clips.remove(&clip_key);
                            self.skeletal_clip_sources.remove(&clip_key);
                        }
                    }
                }
                return true;
            }
        }
        false
    }
    pub fn skeleton(&self, key: &str) -> Option<Arc<skeletal::SkeletonAsset>> {
        self.skeletons.get(key).cloned()
    }
    pub fn skeleton_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.skeletons.keys().cloned().collect();
        keys.sort();
        keys
    }
    pub fn skeleton_source(&self, key: &str) -> Option<&str> {
        self.skeleton_sources.get(key).map(|s| s.as_str())
    }
    pub fn skeletal_clip(&self, key: &str) -> Option<Arc<skeletal::SkeletalClip>> {
        self.skeletal_clips.get(key).cloned()
    }
    pub fn skeletal_clip_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.skeletal_clips.keys().cloned().collect();
        keys.sort();
        keys
    }
    pub fn skeletal_clip_keys_for(&self, skeleton_key: &str) -> Option<&[String]> {
        self.skeleton_clip_index.get(skeleton_key).map(|vec| vec.as_slice())
    }
    pub fn release_atlas(&mut self, key: &str) -> bool {
        if let Some(count) = self.atlas_refs.get_mut(key) {
            if *count > 0 {
                *count -= 1;
                if *count == 0 {
                    self.atlas_refs.remove(key);
                    if let Some(atlas) = self.atlases.remove(key) {
                        self.texture_cache.remove(&atlas.image_path);
                    }
                    self.atlas_sources.remove(key);
                }
                return true;
            }
        }
        false
    }
    pub fn atlas_ref_count(&self, key: &str) -> usize {
        self.atlas_refs.get(key).copied().unwrap_or(0)
    }
    pub fn atlas_texture_view(&mut self, key: &str) -> Result<wgpu::TextureView> {
        self.load_or_reload_view(key, false)
    }
    fn load_or_reload_view(&mut self, key: &str, force: bool) -> Result<wgpu::TextureView> {
        let atlas = self.atlases.get(key).ok_or_else(|| anyhow!("atlas '{key}' not loaded"))?;
        let image_path = atlas.image_path.clone();
        if !force {
            if let Some((view, _)) = self.texture_cache.get(&image_path) {
                return Ok(view.clone());
            }
        }
        let dev = self.device.as_ref().ok_or_else(|| anyhow!("GPU device not initialized"))?;
        let q = self.queue.as_ref().ok_or_else(|| anyhow!("GPU queue not initialized"))?;
        let bytes = std::fs::read(&image_path)?;
        let img = image::load_from_memory(&bytes)?.to_rgba8();
        let (w, h) = img.dimensions();
        let rgba = img.into_raw();
        let texture = dev.create_texture(&wgpu::TextureDescriptor {
            label: Some("Atlas Texture"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        q.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.texture_cache.insert(image_path, (view.clone(), (w, h)));
        Ok(view)
    }
    pub fn atlas_region_uv(&self, atlas_key: &str, region: &str) -> Result<[f32; 4]> {
        let atlas = self.atlases.get(atlas_key).ok_or_else(|| anyhow!("atlas '{atlas_key}' not loaded"))?;
        let (_, info) = atlas
            .regions
            .get_key_value(region)
            .ok_or_else(|| anyhow!("region '{region}' not found in atlas '{atlas_key}'"))?;
        Ok(info.uv)
    }
    pub fn atlas_region_exists(&self, atlas_key: &str, region: &str) -> bool {
        self.atlases.get(atlas_key).and_then(|atlas| atlas.regions.get(region)).is_some()
    }
    pub fn atlas_region_info(&self, atlas_key: &str, region: &str) -> Option<(&Arc<str>, &AtlasRegion)> {
        self.atlases.get(atlas_key).and_then(|atlas| atlas.regions.get_key_value(region))
    }
    pub fn atlas_region_names(&self, atlas_key: &str) -> Vec<String> {
        self.atlases
            .get(atlas_key)
            .map(|atlas| {
                let mut names: Vec<String> =
                    atlas.regions.keys().map(|name| name.as_ref().to_string()).collect();
                names.sort();
                names
            })
            .unwrap_or_default()
    }
    pub fn atlas_timeline(&self, atlas_key: &str, name: &str) -> Option<&SpriteTimeline> {
        self.atlases.get(atlas_key).and_then(|atlas| atlas.animations.get(name))
    }
    pub fn atlas_timeline_names(&self, atlas_key: &str) -> Vec<String> {
        self.atlases
            .get(atlas_key)
            .map(|atlas| atlas.animations.keys().cloned().collect())
            .unwrap_or_default()
    }
    pub fn has_atlas(&self, key: &str) -> bool {
        self.atlases.contains_key(key)
    }
    pub fn atlas_source(&self, key: &str) -> Option<&str> {
        self.atlas_sources.get(key).map(|s| s.as_str())
    }

    pub fn atlas_sources(&self) -> Vec<(String, String)> {
        self.atlas_sources.iter().map(|(key, path)| (key.clone(), path.clone())).collect()
    }

    pub fn reload_atlas(&mut self, key: &str) -> Result<()> {
        let source = self
            .atlas_sources
            .get(key)
            .cloned()
            .ok_or_else(|| anyhow!("Atlas '{key}' has no recorded source; cannot hot-reload"))?;

        let previous_image = self.atlases.get(key).map(|atlas| atlas.image_path.clone());

        self.load_atlas_internal(key, &source)?;

        if let Some(image_path) = previous_image {
            self.texture_cache.remove(&image_path);
        }
        if let Some(current) = self.atlases.get(key) {
            let image_path = current.image_path.clone();
            self.texture_cache.remove(&image_path);
            if self.device.is_some() {
                if let Err(err) = self.load_or_reload_view(key, true) {
                    eprintln!("[assets] Warning: failed to refresh GPU texture for atlas '{key}': {err}");
                }
            }
        }
        Ok(())
    }
}

fn resolve_atlas_image_path(json_path: &str, image: &str) -> PathBuf {
    let image_path = Path::new(image);
    if image_path.is_absolute() {
        return image_path.to_path_buf();
    }
    match Path::new(json_path).parent() {
        Some(parent) => parent.join(image_path),
        None => image_path.to_path_buf(),
    }
}
