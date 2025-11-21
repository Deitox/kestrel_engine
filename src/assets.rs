use crate::ecs::{SpriteAnimationFrame, SpriteAnimationLoopMode, SpriteFrameHotData};
use anyhow::{anyhow, Context, Result};
use glam::{Vec2, Vec4};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::convert::TryFrom;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

pub mod skeletal;

pub struct AssetManager {
    atlases: HashMap<String, TextureAtlas>,
    clips: HashMap<String, AnimationClip>,
    animation_graphs: HashMap<String, AnimationGraphAsset>,
    skeletons: HashMap<String, Arc<skeletal::SkeletonAsset>>,
    skeletal_clips: HashMap<String, Arc<skeletal::SkeletalClip>>,
    sampler: Option<wgpu::Sampler>,
    device: Option<wgpu::Device>,
    queue: Option<wgpu::Queue>,
    texture_cache: HashMap<PathBuf, (wgpu::TextureView, (u32, u32))>,
    atlas_image_cache: HashMap<PathBuf, CachedAtlasImage>,
    atlas_upload_scratch: Vec<u8>,
    atlas_image_cache_order: VecDeque<PathBuf>,
    atlas_sources: HashMap<String, String>,
    atlas_refs: HashMap<String, usize>,
    clip_sources: HashMap<String, String>,
    clip_refs: HashMap<String, usize>,
    animation_graph_sources: HashMap<String, String>,
    skeleton_sources: HashMap<String, String>,
    skeleton_refs: HashMap<String, usize>,
    skeletal_clip_sources: HashMap<String, String>,
    skeleton_clip_index: HashMap<String, Vec<String>>,
}

struct CachedAtlasImage {
    modified: SystemTime,
    width: u32,
    height: u32,
    pixels: Arc<[u8]>,
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

fn convert_interpolation_to_file(value: ClipInterpolation) -> ClipInterpolationFile {
    match value {
        ClipInterpolation::Linear => ClipInterpolationFile::Linear,
        ClipInterpolation::Step => ClipInterpolationFile::Step,
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
    pub lint: Vec<SpriteAtlasLint>,
}

#[derive(Clone, Default)]
pub struct TextureAtlasDiagnostics {
    pub warnings: Vec<String>,
}

impl TextureAtlasDiagnostics {
    pub fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }
}

pub struct TextureAtlasParseResult {
    pub atlas: TextureAtlas,
    pub diagnostics: TextureAtlasDiagnostics,
}

pub struct AtlasSnapshot<'a> {
    pub width: u32,
    pub height: u32,
    pub image_path: &'a Path,
    pub regions: &'a HashMap<Arc<str>, AtlasRegion>,
    pub animations: &'a HashMap<String, SpriteTimeline>,
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
    pub hot_frames: Arc<[SpriteFrameHotData]>,
    pub durations: Arc<[f32]>,
    pub frame_offsets: Arc<[f32]>,
    pub total_duration: f32,
    pub total_duration_inv: f32,
}

#[derive(Clone)]
pub struct SpriteAtlasLint {
    pub code: String,
    pub severity: SpriteAtlasLintSeverity,
    pub timeline: String,
    pub message: String,
    pub reference_ms: u32,
    pub max_diff_ms: u32,
    pub frames: Vec<SpriteAtlasLintFrame>,
}

#[derive(Clone)]
pub enum SpriteAtlasLintSeverity {
    Info,
    Warn,
}

#[derive(Clone)]
pub struct SpriteAtlasLintFrame {
    pub frame: usize,
    pub duration_ms: u32,
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

#[derive(Clone, Copy)]
pub struct ClipKeyframe<T> {
    pub time: f32,
    pub value: T,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ClipInterpolation {
    Step,
    Linear,
}

#[derive(Clone)]
pub struct AnimationGraphAsset {
    pub name: Arc<str>,
    pub version: u32,
    pub entry_state: Arc<str>,
    pub states: Arc<[AnimationGraphState]>,
    pub transitions: Arc<[AnimationGraphTransition]>,
    pub parameters: Arc<[AnimationGraphParameter]>,
}

#[derive(Clone)]
pub struct AnimationGraphState {
    pub name: Arc<str>,
    pub clip: Option<String>,
}

#[derive(Clone)]
pub struct AnimationGraphTransition {
    pub from: Arc<str>,
    pub to: Arc<str>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnimationGraphParameterKind {
    Bool,
    Float,
}

#[derive(Clone)]
pub struct AnimationGraphParameter {
    pub name: Arc<str>,
    pub kind: AnimationGraphParameterKind,
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
    #[serde(default)]
    lint: Vec<AtlasLintFile>,
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
struct AtlasLintFile {
    code: String,
    severity: String,
    timeline: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    reference_ms: Option<u32>,
    #[serde(default)]
    max_diff_ms: Option<u32>,
    #[serde(default)]
    frames: Vec<AtlasLintFrameFile>,
}

#[derive(Debug, Deserialize)]
struct AtlasLintFrameFile {
    frame: usize,
    duration_ms: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct ClipFile {
    version: u32,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    looped: bool,
    #[serde(default)]
    tracks: ClipTracksFile,
}

#[derive(Debug, Default, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
struct ClipVec2TrackFile {
    #[serde(default = "default_clip_interpolation")]
    interpolation: ClipInterpolationFile,
    keyframes: Vec<ClipVec2KeyframeFile>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ClipScalarTrackFile {
    #[serde(default = "default_clip_interpolation")]
    interpolation: ClipInterpolationFile,
    keyframes: Vec<ClipScalarKeyframeFile>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ClipVec4TrackFile {
    #[serde(default = "default_clip_interpolation")]
    interpolation: ClipInterpolationFile,
    keyframes: Vec<ClipVec4KeyframeFile>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ClipInterpolationFile {
    Linear,
    Step,
}

#[derive(Debug, Serialize, Deserialize)]
struct ClipVec2KeyframeFile {
    time: f32,
    value: [f32; 2],
}

#[derive(Debug, Serialize, Deserialize)]
struct ClipScalarKeyframeFile {
    time: f32,
    value: f32,
}

#[derive(Debug, Serialize, Deserialize)]
struct ClipVec4KeyframeFile {
    time: f32,
    value: [f32; 4],
}

#[derive(Debug, Deserialize)]
struct AnimationGraphFile {
    version: Option<u32>,
    name: Option<String>,
    entry_state: Option<String>,
    states: Vec<AnimationGraphStateFile>,
    #[serde(default)]
    transitions: Vec<AnimationGraphTransitionFile>,
    #[serde(default)]
    parameters: Vec<AnimationGraphParameterFile>,
}

#[derive(Debug, Deserialize)]
struct AnimationGraphStateFile {
    name: String,
    clip: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnimationGraphTransitionFile {
    from: String,
    to: String,
}

#[derive(Debug, Deserialize)]
struct AnimationGraphParameterFile {
    name: String,
    #[serde(default)]
    kind: Option<AnimationGraphParameterKind>,
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

fn clip_to_file(clip: &AnimationClip) -> ClipFile {
    ClipFile {
        version: clip.version,
        name: Some(clip.name.as_ref().to_string()),
        looped: clip.looped,
        tracks: ClipTracksFile {
            translation: clip.translation.as_ref().map(vec2_track_to_file),
            rotation: clip.rotation.as_ref().map(scalar_track_to_file),
            scale: clip.scale.as_ref().map(vec2_track_to_file),
            tint: clip.tint.as_ref().map(vec4_track_to_file),
        },
    }
}

fn vec2_track_to_file(track: &ClipVec2Track) -> ClipVec2TrackFile {
    ClipVec2TrackFile {
        interpolation: convert_interpolation_to_file(track.interpolation),
        keyframes: track
            .keyframes
            .iter()
            .map(|kf| ClipVec2KeyframeFile { time: kf.time, value: [kf.value.x, kf.value.y] })
            .collect(),
    }
}

fn scalar_track_to_file(track: &ClipScalarTrack) -> ClipScalarTrackFile {
    ClipScalarTrackFile {
        interpolation: convert_interpolation_to_file(track.interpolation),
        keyframes: track
            .keyframes
            .iter()
            .map(|kf| ClipScalarKeyframeFile { time: kf.time, value: kf.value })
            .collect(),
    }
}

fn vec4_track_to_file(track: &ClipVec4Track) -> ClipVec4TrackFile {
    ClipVec4TrackFile {
        interpolation: convert_interpolation_to_file(track.interpolation),
        keyframes: track
            .keyframes
            .iter()
            .map(|kf| ClipVec4KeyframeFile {
                time: kf.time,
                value: [kf.value.x, kf.value.y, kf.value.z, kf.value.w],
            })
            .collect(),
    }
}

pub fn parse_animation_clip_bytes(bytes: &[u8], key_hint: &str, source_label: &str) -> Result<AnimationClip> {
    let clip_file: ClipFile = serde_json::from_slice(bytes)
        .with_context(|| format!("parse animation clip JSON ({source_label})"))?;
    if clip_file.version == 0 {
        return Err(anyhow!(
            "Clip '{}' has unsupported version 0 (expected >= 1) in {source_label}",
            clip_file.name.as_deref().unwrap_or(key_hint)
        ));
    }
    let ClipTracksFile { translation, rotation, scale, tint } = clip_file.tracks;
    let mut duration = 0.0_f32;
    let translation = if let Some(track) = translation {
        let (parsed, track_duration) = build_vec2_track(track)?;
        duration = duration.max(track_duration);
        Some(parsed)
    } else {
        None
    };
    let rotation = if let Some(track) = rotation {
        let (parsed, track_duration) = build_scalar_track(track)?;
        duration = duration.max(track_duration);
        Some(parsed)
    } else {
        None
    };
    let scale = if let Some(track) = scale {
        let (parsed, track_duration) = build_vec2_track(track)?;
        duration = duration.max(track_duration);
        Some(parsed)
    } else {
        None
    };
    let tint = if let Some(track) = tint {
        let (parsed, track_duration) = build_vec4_track(track)?;
        duration = duration.max(track_duration);
        Some(parsed)
    } else {
        None
    };
    let duration = if duration <= 0.0 { 0.0 } else { duration };
    let name = clip_file.name.unwrap_or_else(|| key_hint.to_string());
    Ok(AnimationClip {
        name: Arc::from(name),
        duration,
        duration_inv: if duration > 0.0 { 1.0 / duration } else { 0.0 },
        translation,
        rotation,
        scale,
        tint,
        looped: clip_file.looped,
        version: clip_file.version,
    })
}

pub fn parse_animation_graph_bytes(
    bytes: &[u8],
    key_hint: &str,
    source_label: &str,
) -> Result<AnimationGraphAsset> {
    let file: AnimationGraphFile = serde_json::from_slice(bytes)
        .with_context(|| format!("parse animation graph JSON ({source_label})"))?;
    let version = file.version.unwrap_or(1);
    if version == 0 {
        return Err(anyhow!(
            "Graph '{}' has unsupported version 0 (expected >= 1) in {source_label}",
            file.name.as_deref().unwrap_or(key_hint)
        ));
    }
    if file.states.is_empty() {
        return Err(anyhow!("Graph '{}' does not define any states in {source_label}", key_hint));
    }
    let mut states: Vec<AnimationGraphState> = Vec::with_capacity(file.states.len());
    for state in file.states {
        if state.name.trim().is_empty() {
            return Err(anyhow!("Animation graph contains a state with an empty name in {source_label}"));
        }
        states.push(AnimationGraphState { name: Arc::from(state.name), clip: state.clip });
    }
    let mut transitions: Vec<AnimationGraphTransition> = Vec::new();
    for transition in file.transitions {
        if transition.from.trim().is_empty() || transition.to.trim().is_empty() {
            return Err(anyhow!("Animation graph transition names cannot be empty in {source_label}"));
        }
        transitions.push(AnimationGraphTransition {
            from: Arc::from(transition.from),
            to: Arc::from(transition.to),
        });
    }
    let mut parameters: Vec<AnimationGraphParameter> = Vec::new();
    for param in file.parameters {
        if param.name.trim().is_empty() {
            return Err(anyhow!("Animation graph parameter names cannot be empty in {source_label}"));
        }
        parameters.push(AnimationGraphParameter {
            name: Arc::from(param.name),
            kind: param.kind.unwrap_or(AnimationGraphParameterKind::Float),
        });
    }
    let entry_state = file
        .entry_state
        .or_else(|| states.first().map(|state| state.name.to_string()))
        .ok_or_else(|| anyhow!("Animation graph could not determine entry state in {source_label}"))?;
    let graph_name = file.name.unwrap_or_else(|| key_hint.to_string());
    Ok(AnimationGraphAsset {
        name: Arc::from(graph_name),
        version,
        entry_state: Arc::from(entry_state),
        states: Arc::from(states.into_boxed_slice()),
        transitions: Arc::from(transitions.into_boxed_slice()),
        parameters: Arc::from(parameters.into_boxed_slice()),
    })
}

pub fn parse_texture_atlas_bytes(
    bytes: &[u8],
    key_hint: &str,
    source_path: &str,
) -> Result<TextureAtlasParseResult> {
    let af: AtlasFile =
        serde_json::from_slice(bytes).with_context(|| format!("parse texture atlas JSON ({source_path})"))?;
    if af.width == 0 || af.height == 0 {
        return Err(anyhow!("Atlas '{}' has zero width or height in {source_path}", key_hint));
    }
    if af.regions.is_empty() {
        return Err(anyhow!("Atlas '{}' does not define any regions in {source_path}", key_hint));
    }
    let mut diagnostics = TextureAtlasDiagnostics::default();
    let mut regions = HashMap::new();
    let image_path = resolve_atlas_image_path(source_path, &af.image);
    for (index, (name, rect)) in af.regions.into_iter().enumerate() {
        let id =
            u16::try_from(index).map_err(|_| anyhow!("Atlas '{key_hint}' has more than 65535 regions"))?;
        let name_arc: Arc<str> = Arc::from(name);
        let uv = [
            rect.x as f32 / af.width as f32,
            rect.y as f32 / af.height as f32,
            (rect.x + rect.w) as f32 / af.width as f32,
            (rect.y + rect.h) as f32 / af.height as f32,
        ];
        regions.insert(Arc::clone(&name_arc), AtlasRegion { id, rect, uv });
    }
    let animations = parse_timelines(key_hint, &regions, af.animations, &mut diagnostics);
    let lint = convert_lint_entries(af.lint)?;
    let atlas = TextureAtlas {
        image_key: af.image.clone(),
        image_path: image_path.clone(),
        width: af.width,
        height: af.height,
        regions,
        animations,
        lint,
    };
    Ok(TextureAtlasParseResult { atlas, diagnostics })
}

fn parse_timelines(
    atlas_key: &str,
    regions: &HashMap<Arc<str>, AtlasRegion>,
    raw: HashMap<String, AtlasTimelineFile>,
    diagnostics: &mut TextureAtlasDiagnostics,
) -> HashMap<String, SpriteTimeline> {
    let mut animations = HashMap::new();
    for (timeline_key, mut data) in raw {
        let mut frames = Vec::new();
        let mut hot_frames = Vec::new();
        let mut durations = Vec::new();
        let mut offsets = Vec::new();
        let mut event_map: HashMap<usize, Vec<String>> = HashMap::new();
        for event in data.events.drain(..) {
            event_map.entry(event.frame).or_default().push(event.name);
        }
        let mut accumulated = 0.0_f32;
        for (frame_index, frame) in data.frames.into_iter().enumerate() {
            let Some((region_key, region_info)) = regions.get_key_value(frame.region.as_str()) else {
                diagnostics.warn(format!(
                    "atlas '{atlas_key}': timeline '{timeline_key}' references unknown region '{}', skipping frame.",
                    frame.region
                ));
                continue;
            };
            let frame_name_arc = frame.name.map(Arc::<str>::from).unwrap_or_else(|| Arc::clone(region_key));
            let duration = (frame.duration_ms.max(1) as f32) / 1000.0;
            let event_names = event_map.remove(&frame_index).unwrap_or_default();
            let events: Vec<Arc<str>> = event_names.into_iter().map(|name| Arc::<str>::from(name)).collect();
            offsets.push(accumulated);
            frames.push(SpriteAnimationFrame {
                name: frame_name_arc,
                region: Arc::clone(region_key),
                region_id: region_info.id,
                duration,
                uv: region_info.uv,
                events: Arc::from(events),
            });
            hot_frames.push(SpriteFrameHotData { region_id: region_info.id, uv: region_info.uv });
            durations.push(duration);
            accumulated += duration;
        }
        if frames.is_empty() {
            diagnostics.warn(format!("atlas '{atlas_key}': timeline '{timeline_key}' has no valid frames."));
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
            diagnostics.warn(format!(
                "atlas '{atlas_key}': timeline '{timeline_key}' has events {:?} referencing missing frame index {}.",
                names, frame
            ));
        }
        let timeline_arc = Arc::<str>::from(timeline_key.clone());
        animations.insert(
            timeline_key.clone(),
            SpriteTimeline {
                name: timeline_arc,
                looped,
                loop_mode: mode_enum,
                frames: Arc::from(frames),
                hot_frames: Arc::from(hot_frames),
                durations: Arc::from(durations),
                frame_offsets: Arc::from(offsets.into_boxed_slice()),
                total_duration: accumulated,
                total_duration_inv: if accumulated > 0.0 { 1.0 / accumulated } else { 0.0 },
            },
        );
    }
    animations
}

fn convert_lint_entries(entries: Vec<AtlasLintFile>) -> Result<Vec<SpriteAtlasLint>> {
    let mut out = Vec::new();
    for entry in entries {
        match SpriteAtlasLint::try_from(entry) {
            Ok(lint) => out.push(lint),
            Err(err) => {
                eprintln!("[assets] warning: failed to parse atlas lint entry: {err}");
            }
        }
    }
    Ok(out)
}

impl TryFrom<AtlasLintFile> for SpriteAtlasLint {
    type Error = anyhow::Error;

    fn try_from(value: AtlasLintFile) -> Result<Self> {
        let severity = match value.severity.to_ascii_lowercase().as_str() {
            "warn" => SpriteAtlasLintSeverity::Warn,
            _ => SpriteAtlasLintSeverity::Info,
        };
        let frames = value
            .frames
            .into_iter()
            .map(|frame| SpriteAtlasLintFrame { frame: frame.frame, duration_ms: frame.duration_ms })
            .collect();
        Ok(Self {
            code: value.code,
            severity,
            timeline: value.timeline,
            message: value.message.unwrap_or_else(|| "atlas lint entry".to_string()),
            reference_ms: value.reference_ms.unwrap_or(0),
            max_diff_ms: value.max_diff_ms.unwrap_or(0),
            frames,
        })
    }
}

impl AssetManager {
    pub fn new() -> Self {
        Self {
            atlases: HashMap::new(),
            clips: HashMap::new(),
            animation_graphs: HashMap::new(),
            skeletons: HashMap::new(),
            skeletal_clips: HashMap::new(),
            sampler: None,
            device: None,
            queue: None,
            texture_cache: HashMap::new(),
            atlas_image_cache: HashMap::new(),
            atlas_upload_scratch: Vec::new(),
            atlas_image_cache_order: VecDeque::new(),
            atlas_sources: HashMap::new(),
            atlas_refs: HashMap::new(),
            clip_sources: HashMap::new(),
            clip_refs: HashMap::new(),
            animation_graph_sources: HashMap::new(),
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
        self.atlas_image_cache.clear();
        self.atlas_image_cache_order.clear();
    }
    pub fn default_sampler(&self) -> &wgpu::Sampler {
        self.sampler.as_ref().expect("sampler")
    }
    pub fn load_atlas(&mut self, key: &str, json_path: &str) -> Result<()> {
        let _ = self.load_atlas_internal(key, json_path)?;
        Ok(())
    }
    fn load_atlas_internal(&mut self, key: &str, json_path: &str) -> Result<TextureAtlasDiagnostics> {
        let bytes = fs::read(json_path)?;
        let TextureAtlasParseResult { atlas, diagnostics } =
            parse_texture_atlas_bytes(&bytes, key, json_path)?;
        for warning in &diagnostics.warnings {
            eprintln!("[assets] {warning}");
        }
        self.atlases.insert(key.to_string(), atlas);
        self.atlas_sources.insert(key.to_string(), json_path.to_string());
        Ok(diagnostics)
    }

    pub fn load_clip(&mut self, key: &str, json_path: &str) -> Result<()> {
        self.load_clip_internal(key, json_path)
    }

    fn load_clip_internal(&mut self, key: &str, json_path: &str) -> Result<()> {
        let bytes = fs::read(json_path)?;
        self.load_clip_from_bytes(key, json_path, &bytes)
    }

    pub fn load_clip_from_bytes(&mut self, key: &str, json_path: &str, bytes: &[u8]) -> Result<()> {
        let clip = parse_animation_clip_bytes(bytes, key, json_path)?;
        self.replace_clip(key, json_path, clip);
        Ok(())
    }

    pub fn replace_clip(&mut self, key: &str, json_path: &str, clip: AnimationClip) {
        self.clips.insert(key.to_string(), clip);
        self.clip_sources.insert(key.to_string(), json_path.to_string());
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

    pub fn save_clip(&self, key: &str, clip: &AnimationClip) -> Result<()> {
        let Some(path) = self.clip_sources.get(key) else {
            anyhow::bail!("Clip '{key}' does not have a source path; cannot save");
        };
        let clip_file = clip_to_file(clip);
        let json = serde_json::to_vec_pretty(&clip_file)?;
        fs::write(path, json)?;
        Ok(())
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

    pub fn clip_sources(&self) -> Vec<(String, String)> {
        self.clip_sources.iter().map(|(key, path)| (key.clone(), path.clone())).collect()
    }

    pub fn clip_key_for_source_path<P: AsRef<Path>>(&self, path: P) -> Option<String> {
        let target = normalize_asset_path(path.as_ref());
        self.clip_sources.iter().find_map(|(key, stored)| {
            let stored_path = normalize_asset_path(Path::new(stored));
            if stored_path == target {
                Some(key.clone())
            } else {
                None
            }
        })
    }

    pub fn load_animation_graph(&mut self, key: &str, json_path: &str) -> Result<()> {
        let bytes = fs::read(json_path)?;
        self.load_animation_graph_from_bytes(key, json_path, &bytes)
    }

    pub fn load_animation_graph_from_bytes(
        &mut self,
        key: &str,
        json_path: &str,
        bytes: &[u8],
    ) -> Result<()> {
        let graph = parse_animation_graph_bytes(bytes, key, json_path)?;
        self.replace_animation_graph(key, json_path, graph);
        Ok(())
    }

    pub fn replace_animation_graph(&mut self, key: &str, json_path: &str, graph: AnimationGraphAsset) {
        self.animation_graphs.insert(key.to_string(), graph);
        self.animation_graph_sources.insert(key.to_string(), json_path.to_string());
    }

    pub fn animation_graph(&self, key: &str) -> Option<&AnimationGraphAsset> {
        self.animation_graphs.get(key)
    }

    pub fn animation_graph_sources(&self) -> Vec<(String, String)> {
        self.animation_graph_sources.iter().map(|(key, path)| (key.clone(), path.clone())).collect()
    }

    pub fn animation_graph_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.animation_graphs.keys().cloned().collect();
        keys.sort();
        keys
    }

    pub fn graph_key_for_source_path<P: AsRef<Path>>(&self, path: P) -> Option<String> {
        let target = normalize_asset_path(path.as_ref());
        self.animation_graph_sources.iter().find_map(|(key, stored)| {
            let stored_path = normalize_asset_path(Path::new(stored));
            if stored_path == target {
                Some(key.clone())
            } else {
                None
            }
        })
    }

    fn load_skeleton_internal(&mut self, key: &str, gltf_path: &str) -> Result<()> {
        let import = skeletal::load_skeleton_from_gltf(gltf_path)?;
        self.apply_skeleton_import(key, gltf_path, import);
        Ok(())
    }

    fn apply_skeleton_import(&mut self, key: &str, gltf_path: &str, import: skeletal::SkeletonImport) {
        self.skeletons.insert(key.to_string(), Arc::new(import.skeleton));
        if let Some(existing) = self.skeleton_clip_index.remove(key) {
            for clip_key in existing {
                self.skeletal_clips.remove(&clip_key);
                self.skeletal_clip_sources.remove(&clip_key);
            }
        }
        let mut clip_keys: Vec<String> = Vec::new();
        for clip in import.clips {
            let clip_key = format!("{key}::{}", clip.name.as_ref());
            self.skeletal_clip_sources.insert(clip_key.clone(), gltf_path.to_string());
            self.skeletal_clips.insert(clip_key.clone(), Arc::new(clip));
            clip_keys.push(clip_key);
        }
        self.skeleton_clip_index.insert(key.to_string(), clip_keys);
    }

    pub fn replace_skeleton_from_import(
        &mut self,
        key: &str,
        gltf_path: &str,
        import: skeletal::SkeletonImport,
    ) {
        self.apply_skeleton_import(key, gltf_path, import);
        self.skeleton_sources.insert(key.to_string(), gltf_path.to_string());
    }
    pub fn load_skeleton(&mut self, key: &str, gltf_path: &str) -> Result<()> {
        self.load_skeleton_internal(key, gltf_path)
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
    pub fn skeleton_sources(&self) -> Vec<(String, String)> {
        self.skeleton_sources.iter().map(|(key, path)| (key.clone(), path.clone())).collect()
    }
    pub fn skeleton_key_for_source_path<P: AsRef<Path>>(&self, path: P) -> Option<String> {
        let target = normalize_asset_path(path.as_ref());
        self.skeleton_sources.iter().find_map(|(key, stored)| {
            let stored_path = normalize_asset_path(Path::new(stored));
            if stored_path == target {
                Some(key.clone())
            } else {
                None
            }
        })
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
                        self.remove_cached_atlas_image(&atlas.image_path);
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
        let (rgba, w, h) = self.cached_atlas_pixels(&image_path)?;
        let dev = self.device.as_ref().ok_or_else(|| anyhow!("GPU device not initialized"))?;
        let q = self.queue.as_ref().ok_or_else(|| anyhow!("GPU queue not initialized"))?;
        let rgba_slice = rgba.as_ref();
        let row_stride = (4 * w) as usize;
        let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
        let (upload_slice, padded_stride) = if row_stride % alignment == 0 {
            (rgba_slice, row_stride)
        } else {
            let padded_stride = ((row_stride + alignment - 1) / alignment) * alignment;
            let required = padded_stride * h as usize;
            if self.atlas_upload_scratch.len() < required {
                self.atlas_upload_scratch.resize(required, 0);
            }
            for row in 0..h as usize {
                let src_offset = row * row_stride;
                let dst_offset = row * padded_stride;
                self.atlas_upload_scratch[dst_offset..dst_offset + row_stride]
                    .copy_from_slice(&rgba_slice[src_offset..src_offset + row_stride]);
            }
            (&self.atlas_upload_scratch[..required], padded_stride)
        };
        let bytes_per_row =
            u32::try_from(padded_stride).map_err(|_| anyhow!("atlas '{}' too wide for GPU upload", key))?;
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
            upload_slice,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.texture_cache.insert(image_path, (view.clone(), (w, h)));
        Ok(view)
    }

    fn cached_atlas_pixels(&mut self, image_path: &Path) -> Result<(Arc<[u8]>, u32, u32)> {
        let metadata = fs::metadata(image_path)?;
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if let Some(entry) = self.atlas_image_cache.get(image_path) {
            if entry.modified == modified {
                let cached = (Arc::clone(&entry.pixels), entry.width, entry.height);
                self.touch_cached_atlas_image(image_path);
                return Ok(cached);
            }
        }
        let bytes = std::fs::read(image_path)?;
        let img = image::load_from_memory(&bytes)?.to_rgba8();
        let (width, height) = img.dimensions();
        let pixels: Arc<[u8]> = Arc::from(img.into_raw().into_boxed_slice());
        self.atlas_image_cache.insert(
            image_path.to_path_buf(),
            CachedAtlasImage { modified, width, height, pixels: Arc::clone(&pixels) },
        );
        self.touch_cached_atlas_image(image_path);
        Ok((pixels, width, height))
    }

    fn touch_cached_atlas_image(&mut self, path: &Path) {
        if let Some(pos) = self.atlas_image_cache_order.iter().position(|p| p == path) {
            self.atlas_image_cache_order.remove(pos);
        }
        self.atlas_image_cache_order.push_back(path.to_path_buf());
        while self.atlas_image_cache_order.len() > ATLAS_IMAGE_CACHE_LIMIT {
            if let Some(evicted) = self.atlas_image_cache_order.pop_front() {
                self.atlas_image_cache.remove(&evicted);
            }
        }
    }

    fn remove_cached_atlas_image(&mut self, path: &Path) {
        self.atlas_image_cache.remove(path);
        if let Some(pos) = self.atlas_image_cache_order.iter().position(|p| p == path) {
            self.atlas_image_cache_order.remove(pos);
        }
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
    pub fn atlas_snapshot(&self, key: &str) -> Option<AtlasSnapshot<'_>> {
        let atlas = self.atlases.get(key)?;
        Some(AtlasSnapshot {
            width: atlas.width,
            height: atlas.height,
            image_path: atlas.image_path.as_path(),
            regions: &atlas.regions,
            animations: &atlas.animations,
        })
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

    pub fn reload_atlas(&mut self, key: &str) -> Result<TextureAtlasDiagnostics> {
        let source = self
            .atlas_sources
            .get(key)
            .cloned()
            .ok_or_else(|| anyhow!("Atlas '{key}' has no recorded source; cannot hot-reload"))?;

        let previous_image = self.atlases.get(key).map(|atlas| atlas.image_path.clone());

        let diagnostics = self.load_atlas_internal(key, &source)?;

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
        Ok(diagnostics)
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

fn normalize_asset_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    } else if let Ok(cwd) = env::current_dir() {
        let absolute = cwd.join(path);
        fs::canonicalize(&absolute).unwrap_or(absolute)
    } else {
        path.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn clip_key_for_source_path_handles_equivalent_paths() {
        let mut assets = AssetManager::new();
        assets
            .clip_sources
            .insert("slime".to_string(), "fixtures/animation_clips/slime_bob.json".to_string());
        let relative = PathBuf::from("fixtures/animation_clips/slime_bob.json");
        let canonical = normalize_asset_path(&relative);
        assert_eq!(assets.clip_key_for_source_path(&relative).as_deref(), Some("slime"));
        assert_eq!(assets.clip_key_for_source_path(&canonical).as_deref(), Some("slime"));
    }

    #[test]
    fn graph_key_for_source_path_handles_equivalent_paths() {
        let mut assets = AssetManager::new();
        assets
            .animation_graph_sources
            .insert("example".to_string(), "assets/animations/graphs/example.json".to_string());
        let relative = PathBuf::from("assets/animations/graphs/example.json");
        let canonical = normalize_asset_path(&relative);
        assert_eq!(assets.graph_key_for_source_path(&relative).as_deref(), Some("example"));
        assert_eq!(assets.graph_key_for_source_path(&canonical).as_deref(), Some("example"));
    }
}
const ATLAS_IMAGE_CACHE_LIMIT: usize = 16;
