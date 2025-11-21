use super::{ClipInterpolation, ClipKeyframe};
use anyhow::{anyhow, bail, Context, Result};
use glam::{Mat4, Quat, Vec3};
use gltf::animation::util::{ReadOutputs, Rotations};
use gltf::animation::{Interpolation, Property};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone)]
pub struct SkeletonJoint {
    pub name: Arc<str>,
    pub parent: Option<u32>,
    pub rest_local: Mat4,
    pub rest_world: Mat4,
    pub rest_translation: Vec3,
    pub rest_rotation: Quat,
    pub rest_scale: Vec3,
    pub inverse_bind: Mat4,
}

#[derive(Clone)]
pub struct SkeletonAsset {
    pub name: Arc<str>,
    pub joints: Arc<[SkeletonJoint]>,
    pub roots: Arc<[u32]>,
}

#[derive(Clone)]
pub struct JointVec3Track {
    pub interpolation: ClipInterpolation,
    pub keyframes: Arc<[ClipKeyframe<Vec3>]>,
}

#[derive(Clone)]
pub struct JointQuatTrack {
    pub interpolation: ClipInterpolation,
    pub keyframes: Arc<[ClipKeyframe<Quat>]>,
}

#[derive(Clone)]
pub struct JointCurve {
    pub joint_index: u32,
    pub translation: Option<JointVec3Track>,
    pub rotation: Option<JointQuatTrack>,
    pub scale: Option<JointVec3Track>,
}

#[derive(Clone)]
pub struct SkeletalClip {
    pub name: Arc<str>,
    pub skeleton: Arc<str>,
    pub duration: f32,
    pub channels: Arc<[JointCurve]>,
    pub looped: bool,
}

pub struct SkeletonImport {
    pub skeleton: SkeletonAsset,
    pub clips: Vec<SkeletalClip>,
}

pub fn load_skeleton_from_gltf(path: impl AsRef<Path>) -> Result<SkeletonImport> {
    let path_ref = path.as_ref();
    let (document, buffers, _) = gltf::import(path_ref)
        .with_context(|| format!("Failed to import GLTF skeleton from {}", path_ref.display()))?;

    let mut skins = document.skins();
    let skin =
        skins.next().ok_or_else(|| anyhow!("GLTF '{}' does not contain a skin", path_ref.display()))?;
    if skins.next().is_some() {
        eprintln!(
            "[assets] GLTF '{}' contains multiple skins; only the first will be imported.",
            path_ref.display()
        );
    }

    let skeleton_name: Arc<str> = Arc::<str>::from(
        skin.name()
            .map(|s| s.to_string())
            .or_else(|| {
                path_ref.file_stem().and_then(|stem| stem.to_str()).map(|stem| format!("{stem}_skeleton"))
            })
            .unwrap_or_else(|| "skeleton".to_string()),
    );

    let joint_nodes: Vec<_> = skin.joints().collect();
    if joint_nodes.is_empty() {
        bail!("GLTF '{}' skin '{}' has no joints", path_ref.display(), skeleton_name);
    }

    let node_to_joint: HashMap<usize, u32> =
        joint_nodes.iter().enumerate().map(|(idx, node)| (node.index(), idx as u32)).collect();

    let mut node_local: HashMap<usize, Mat4> = HashMap::new();
    let mut node_trs: HashMap<usize, (Vec3, Quat, Vec3)> = HashMap::new();
    let mut parent_of_node: HashMap<usize, usize> = HashMap::new();
    for node in document.nodes() {
        let node_index = node.index();
        node_local.insert(node_index, mat4_from_gltf(node.transform().matrix()));
        let (t, r, s) = node.transform().decomposed();
        let translation = Vec3::from_array(t);
        let rotation = Quat::from_xyzw(r[0], r[1], r[2], r[3]).normalize();
        let scale = Vec3::from_array(s);
        node_trs.insert(node_index, (translation, rotation, scale));
        for child in node.children() {
            parent_of_node.insert(child.index(), node_index);
        }
    }

    let skin_reader = skin.reader(|buffer| Some(&buffers[buffer.index()]));
    let mut inverse_bind = vec![Mat4::IDENTITY; joint_nodes.len()];
    if let Some(reader) = skin_reader.read_inverse_bind_matrices() {
        for (idx, matrix) in reader.enumerate() {
            if idx < inverse_bind.len() {
                inverse_bind[idx] = mat4_from_gltf(matrix);
            }
        }
    }

    let mut parent_by_joint: Vec<Option<u32>> = vec![None; joint_nodes.len()];
    for (parent_idx, node) in joint_nodes.iter().enumerate() {
        for child in node.children() {
            if let Some(&child_joint) = node_to_joint.get(&child.index()) {
                parent_by_joint[child_joint as usize] = Some(parent_idx as u32);
            }
        }
    }

    let mut world_cache: HashMap<usize, Mat4> = HashMap::new();
    let mut joints: Vec<SkeletonJoint> = Vec::with_capacity(joint_nodes.len());
    let mut root_indices: Vec<u32> = Vec::new();

    for (index, node) in joint_nodes.iter().enumerate() {
        let node_index = node.index();
        let parent_joint = parent_by_joint[index];
        if parent_joint.is_none() {
            root_indices.push(index as u32);
        }
        let rest_local = *node_local.get(&node_index).unwrap_or(&Mat4::IDENTITY);
        let rest_world = compute_world_matrix(node_index, &node_local, &parent_of_node, &mut world_cache);
        let joint_name = node.name().map(|n| n.to_string()).unwrap_or_else(|| format!("joint_{index}"));
        let (rest_translation, rest_rotation, rest_scale) =
            node_trs.get(&node_index).cloned().unwrap_or((Vec3::ZERO, Quat::IDENTITY, Vec3::ONE));
        joints.push(SkeletonJoint {
            name: Arc::<str>::from(joint_name),
            parent: parent_joint,
            rest_local,
            rest_world,
            rest_translation,
            rest_rotation,
            rest_scale,
            inverse_bind: inverse_bind[index],
        });
    }

    root_indices.sort_unstable();
    root_indices.dedup();

    let skeleton_asset = SkeletonAsset {
        name: Arc::clone(&skeleton_name),
        joints: Arc::from(joints.into_boxed_slice()),
        roots: Arc::from(root_indices.into_boxed_slice()),
    };

    let mut clips: Vec<SkeletalClip> = Vec::new();
    for (anim_index, animation) in document.animations().enumerate() {
        let clip_name: Arc<str> = animation
            .name()
            .map(|n| Arc::<str>::from(n.to_string()))
            .unwrap_or_else(|| Arc::<str>::from(format!("animation_{anim_index}")));

        let mut curve_builders: HashMap<u32, JointCurveBuilder> = HashMap::new();

        for channel in animation.channels() {
            let target_node = channel.target().node();
            let Some(joint_index) = node_to_joint.get(&target_node.index()).copied() else {
                continue;
            };

            let interpolation = match channel.sampler().interpolation() {
                Interpolation::Linear => ClipInterpolation::Linear,
                Interpolation::Step => ClipInterpolation::Step,
                Interpolation::CubicSpline => {
                    eprintln!(
                        "[assets] animation '{}' uses CubicSpline interpolation; skipping channel (node {}).",
                        clip_name,
                        target_node.index()
                    );
                    continue;
                }
            };

            let reader = channel.reader(|buffer| Some(&buffers[buffer.index()]));
            let Some(inputs) = reader.read_inputs() else {
                continue;
            };
            let times: Vec<f32> = inputs.collect();
            if times.is_empty() {
                continue;
            }

            let Some(outputs) = reader.read_outputs() else {
                continue;
            };

            let builder = curve_builders.entry(joint_index).or_default();
            match (channel.target().property(), outputs) {
                (Property::Translation, ReadOutputs::Translations(values)) => {
                    let vec_values: Vec<Vec3> = values.map(Vec3::from_array).collect();
                    if vec_values.len() != times.len() {
                        return Err(anyhow!(
                            "Animation '{}' translation channel count mismatch (node {})",
                            clip_name,
                            target_node.index()
                        ));
                    }
                    let track = build_vec3_track(&times, vec_values, interpolation)?;
                    builder.translation = Some(track);
                }
                (Property::Scale, ReadOutputs::Scales(values)) => {
                    let vec_values: Vec<Vec3> = values.map(Vec3::from_array).collect();
                    if vec_values.len() != times.len() {
                        return Err(anyhow!(
                            "Animation '{}' scale channel count mismatch (node {})",
                            clip_name,
                            target_node.index()
                        ));
                    }
                    let track = build_vec3_track(&times, vec_values, interpolation)?;
                    builder.scale = Some(track);
                }
                (Property::Rotation, ReadOutputs::Rotations(rotations)) => {
                    let quat_values = convert_rotations(rotations);
                    if quat_values.len() != times.len() {
                        return Err(anyhow!(
                            "Animation '{}' rotation channel count mismatch (node {})",
                            clip_name,
                            target_node.index()
                        ));
                    }
                    let track = build_quat_track(&times, quat_values, interpolation)?;
                    builder.rotation = Some(track);
                }
                (Property::MorphTargetWeights, _) => {
                    // Morph target information is not yet consumed by the animation stack.
                }
                _ => {}
            }
        }

        let mut curves: Vec<JointCurve> = Vec::new();
        for (joint_index, builder) in curve_builders {
            if let Some(curve) = builder.into_curve(joint_index) {
                curves.push(curve);
            }
        }

        if curves.is_empty() {
            continue;
        }

        let mut duration = 0.0_f32;
        for curve in &curves {
            if let Some(track) = &curve.translation {
                duration = duration.max(track.keyframes.last().map(|kf| kf.time).unwrap_or(0.0));
            }
            if let Some(track) = &curve.rotation {
                duration = duration.max(track.keyframes.last().map(|kf| kf.time).unwrap_or(0.0));
            }
            if let Some(track) = &curve.scale {
                duration = duration.max(track.keyframes.last().map(|kf| kf.time).unwrap_or(0.0));
            }
        }

        let clip = SkeletalClip {
            name: clip_name,
            skeleton: Arc::clone(&skeleton_name),
            duration,
            channels: Arc::from(curves.into_boxed_slice()),
            looped: true,
        };
        clips.push(clip);
    }

    Ok(SkeletonImport { skeleton: skeleton_asset, clips })
}

#[derive(Default)]
struct JointCurveBuilder {
    translation: Option<JointVec3Track>,
    rotation: Option<JointQuatTrack>,
    scale: Option<JointVec3Track>,
}

impl JointCurveBuilder {
    fn into_curve(self, joint_index: u32) -> Option<JointCurve> {
        if self.translation.is_none() && self.rotation.is_none() && self.scale.is_none() {
            None
        } else {
            Some(JointCurve {
                joint_index,
                translation: self.translation,
                rotation: self.rotation,
                scale: self.scale,
            })
        }
    }
}

fn build_vec3_track(
    times: &[f32],
    values: Vec<Vec3>,
    interpolation: ClipInterpolation,
) -> Result<JointVec3Track> {
    let keyframes = build_keyframes(times, values)?;
    Ok(JointVec3Track { interpolation, keyframes })
}

fn build_quat_track(
    times: &[f32],
    values: Vec<Quat>,
    interpolation: ClipInterpolation,
) -> Result<JointQuatTrack> {
    let keyframes = build_keyframes(times, values)?;
    Ok(JointQuatTrack { interpolation, keyframes })
}

fn build_keyframes<T: Clone>(times: &[f32], values: Vec<T>) -> Result<Arc<[ClipKeyframe<T>]>> {
    if times.len() != values.len() {
        bail!("Animation channel time/value count mismatch ({} vs {})", times.len(), values.len());
    }
    let mut frames: Vec<ClipKeyframe<T>> = Vec::with_capacity(times.len());
    for (time, value) in times.iter().copied().zip(values.into_iter()) {
        if !time.is_finite() {
            bail!("Animation channel contains non-finite time value");
        }
        if time < 0.0 {
            bail!("Animation channel time cannot be negative");
        }
        if let Some(last) = frames.last_mut() {
            if (time - last.time).abs() <= f32::EPSILON {
                last.value = value;
                continue;
            }
        }
        frames.push(ClipKeyframe { time, value });
    }
    Ok(Arc::from(frames.into_boxed_slice()))
}

fn convert_rotations(rotations: Rotations) -> Vec<Quat> {
    rotations
        .into_f32()
        .map(|components| {
            let quat = Quat::from_xyzw(components[0], components[1], components[2], components[3]);
            if quat.length_squared() > 0.0 {
                quat.normalize()
            } else {
                Quat::IDENTITY
            }
        })
        .collect()
}

fn mat4_from_gltf(matrix: [[f32; 4]; 4]) -> Mat4 {
    Mat4::from_cols_array_2d(&matrix)
}

fn compute_world_matrix(
    node_index: usize,
    node_local: &HashMap<usize, Mat4>,
    parent_map: &HashMap<usize, usize>,
    cache: &mut HashMap<usize, Mat4>,
) -> Mat4 {
    if let Some(world) = cache.get(&node_index) {
        return *world;
    }
    let local = *node_local.get(&node_index).unwrap_or(&Mat4::IDENTITY);
    let world = if let Some(parent_index) = parent_map.get(&node_index) {
        compute_world_matrix(*parent_index, node_local, parent_map, cache) * local
    } else {
        local
    };
    cache.insert(node_index, world);
    world
}
