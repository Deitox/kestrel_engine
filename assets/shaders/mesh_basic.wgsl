struct FrameUniform {
    view_proj : mat4x4<f32>,
    view : mat4x4<f32>,
    camera_pos : vec4<f32>,
    light_dir : vec4<f32>,
    light_color : vec4<f32>,
    ambient_color : vec4<f32>,
    exposure_params : vec4<f32>,
    cascade_splits : vec4<f32>,
}

const MAX_SKIN_JOINTS : u32 = 256u;
const MAX_SHADOW_CASCADES : u32 = 4u;
const MAX_CLUSTER_LIGHTS : u32 = 256u;
const CLUSTER_RECORD_STRIDE_WORDS : u32 = 2u;

struct DrawUniform {
    model : mat4x4<f32>,
    base_color : vec4<f32>,
    emissive : vec4<f32>,
    material_params : vec4<f32>,
}

struct SkinPalette {
    matrices : array<mat4x4<f32>, MAX_SKIN_JOINTS>,
}

struct MaterialUniform {
    base_color_factor : vec4<f32>,
   emissive_factor : vec4<f32>,
    params : vec4<f32>,
    texture_flags : vec4<f32>,
}

struct ShadowUniform {
    light_view_proj : array<mat4x4<f32>, MAX_SHADOW_CASCADES>,
    params : vec4<f32>,
    cascade_params : array<vec4<f32>, MAX_SHADOW_CASCADES>,
}

struct PointLight {
    position_radius : vec4<f32>,
    color_intensity : vec4<f32>,
}

struct ClusterRecord {
    offset : u32,
    count : u32,
    _pad0 : u32,
    _pad1 : u32,
}

struct ClusterConfig {
    viewport : vec4<f32>,
    depth_params : vec4<f32>,
    grid_dims : vec4<u32>,
    stats : vec4<u32>,
    data_meta : vec4<u32>,
}

struct ClusterLightUniform {
    config : ClusterConfig,
    lights : array<PointLight, MAX_CLUSTER_LIGHTS>,
}

@group(0) @binding(0)
var<uniform> frame : FrameUniform;

@group(0) @binding(1)
var<uniform> draw : DrawUniform;

@group(1) @binding(0)
var<uniform> skinning : SkinPalette;

@group(2) @binding(0)
var<uniform> material : MaterialUniform;

@group(2) @binding(1)
var base_color_tex : texture_2d<f32>;

@group(2) @binding(2)
var metallic_roughness_tex : texture_2d<f32>;

@group(2) @binding(3)
var normal_tex : texture_2d<f32>;

@group(2) @binding(4)
var emissive_tex : texture_2d<f32>;

@group(2) @binding(5)
var material_sampler : sampler;

@group(3) @binding(0)
var<uniform> shadow : ShadowUniform;

@group(3) @binding(1)
var shadow_map : texture_depth_2d_array;

@group(3) @binding(2)
var shadow_sampler : sampler_comparison;

@group(4) @binding(0)
var diffuse_env : texture_cube<f32>;

@group(4) @binding(1)
var specular_env : texture_cube<f32>;

@group(4) @binding(2)
var brdf_lut : texture_2d<f32>;

@group(4) @binding(3)
var env_sampler : sampler;

@group(5) @binding(0)
var<uniform> cluster_uniform : ClusterLightUniform;

@group(5) @binding(1)
var<storage, read> cluster_data_words : array<u32>;

struct VertexIn {
    @location(0) position : vec3<f32>,
    @location(1) normal : vec3<f32>,
    @location(2) tangent : vec4<f32>,
    @location(3) uv : vec2<f32>,
    @location(4) joints : vec4<u32>,
    @location(5) weights : vec4<f32>,
}

struct VertexOut {
    @builtin(position) position : vec4<f32>,
    @location(0) normal : vec3<f32>,
    @location(1) world_pos : vec3<f32>,
    @location(2) tangent : vec4<f32>,
    @location(3) uv : vec2<f32>,
    @location(4) clip_pos : vec4<f32>,
    @location(5) view_pos : vec3<f32>,
}

fn identity_matrix() -> mat4x4<f32> {
    return mat4x4<f32>(
        vec4<f32>(1.0, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, 1.0, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, 1.0, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, 1.0),
    );
}

fn accumulate_skin(joints : vec4<u32>, weights : vec4<f32>, joint_count : u32) -> mat4x4<f32> {
    if joint_count == 0u {
        return identity_matrix();
    }
    let max_joints = min(joint_count, MAX_SKIN_JOINTS);
    var skin = mat4x4<f32>(
        vec4<f32>(0.0, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, 0.0),
    );
    var accum = 0.0;
    var i : u32 = 0u;
    loop {
        if i >= 4u {
            break;
        }
        let weight = weights[i];
        if weight > 0.0 {
            let index = joints[i];
            if index < max_joints {
                let matrix = skinning.matrices[index];
                skin = skin + matrix * weight;
                accum = accum + weight;
            }
        }
        i = i + 1u;
    }
    if accum <= 0.0 {
        return identity_matrix();
    }
    return skin;
}

@vertex
fn vs_main(input : VertexIn) -> VertexOut {
    var out : VertexOut;
    let joint_count = u32(draw.material_params.w + 0.5);
    let skin_matrix = accumulate_skin(input.joints, input.weights, joint_count);
    let skinned_position = skin_matrix * vec4<f32>(input.position, 1.0);
    let skinned_normal = (skin_matrix * vec4<f32>(input.normal, 0.0)).xyz;
    let skinned_tangent = (skin_matrix * vec4<f32>(input.tangent.xyz, 0.0)).xyz;
    let world_pos = draw.model * skinned_position;
    let clip_position = frame.view_proj * world_pos;
    out.position = clip_position;
    let world_normal = (draw.model * vec4<f32>(skinned_normal, 0.0)).xyz;
    out.normal = normalize(world_normal);
    out.world_pos = world_pos.xyz;
    let world_tangent = (draw.model * vec4<f32>(skinned_tangent, 0.0)).xyz;
    out.tangent = vec4<f32>(normalize(world_tangent), input.tangent.w);
    out.uv = input.uv;
    out.clip_pos = clip_position;
    out.view_pos = (frame.view * world_pos).xyz;
    return out;
}

fn fresnel_schlick(cos_theta : f32, f0 : vec3<f32>) -> vec3<f32> {
    return f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - cos_theta, 5.0);
}

fn distribution_ggx(n : vec3<f32>, h : vec3<f32>, roughness : f32) -> f32 {
    let a = roughness * roughness;
    let a2 = a * a;
    let n_dot_h = max(dot(n, h), 0.0);
    let denom = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / (3.14159265 * denom * denom + 1e-7);
}

fn geometry_schlick_ggx(n_dot_v : f32, roughness : f32) -> f32 {
    let r = roughness + 1.0;
    let k = (r * r) * 0.125;
    return n_dot_v / (n_dot_v * (1.0 - k) + k);
}

fn geometry_smith(n : vec3<f32>, v : vec3<f32>, l : vec3<f32>, roughness : f32) -> f32 {
    let n_dot_v = max(dot(n, v), 0.0);
    let n_dot_l = max(dot(n, l), 0.0);
    return geometry_schlick_ggx(n_dot_v, roughness) * geometry_schlick_ggx(n_dot_l, roughness);
}

fn apply_normal_map(
    n : vec3<f32>,
    tangent : vec4<f32>,
    uv : vec2<f32>,
    normal_scale : f32,
) -> vec3<f32> {
    let t = normalize(tangent.xyz);
    let b = normalize(cross(n, t)) * tangent.w;
    let tex_sample = textureSample(normal_tex, material_sampler, uv).xyz * 2.0 - vec3<f32>(1.0);
    var tangent_normal = vec3<f32>(tex_sample.xy * normal_scale, tex_sample.z);
    tangent_normal = normalize(tangent_normal);
    let tbn = mat3x3<f32>(t, b, n);
    return normalize(tbn * tangent_normal);
}

fn cascade_for_depth(depth : f32, splits : vec4<f32>, cascade_count : u32) -> u32 {
    var index : u32 = 0u;
    if cascade_count > 1u && depth > splits.x {
        index = 1u;
    }
    if cascade_count > 2u && depth > splits.y {
        index = 2u;
    }
    if cascade_count > 3u && depth > splits.z {
        index = 3u;
    }
    return min(index, max(cascade_count, 1u) - 1u);
}

fn sample_shadow(world_pos : vec3<f32>) -> f32 {
    let view_pos = frame.view * vec4<f32>(world_pos, 1.0);
    let cascade_depth = -view_pos.z;
    let cascade_count = u32(clamp(shadow.params.z, 1.0, f32(MAX_SHADOW_CASCADES)) + 0.5);
    let cascade_index = cascade_for_depth(cascade_depth, frame.cascade_splits, cascade_count);
    let clip = shadow.light_view_proj[cascade_index] * vec4<f32>(world_pos, 1.0);
    if abs(clip.w) < 1e-5 {
        return 1.0;
    }
    let inv_w = 1.0 / clip.w;
    let ndc = clip.xyz * inv_w;
    let uv = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 {
        return 1.0;
    }
    let sample_depth = clamp(ndc.z * 0.5 + 0.5, 0.0, 1.0);
    let bias = shadow.params.x;
    let comparison_depth = sample_depth - bias;
    let base = textureSampleCompare(shadow_map, shadow_sampler, uv, i32(cascade_index), comparison_depth);
    let cascade_data = shadow.cascade_params[cascade_index];
    let radius = max(cascade_data.y, 0.0);
    let texel = cascade_data.x;
    if radius <= 0.001 || texel <= 0.0 {
        return base;
    }
    var sum = base;
    var taps = 1.0;
    let step_size = vec2<f32>(texel * radius, texel * radius);
    var y : i32 = -1;
    loop {
        if y > 1 {
            break;
        }
        var x : i32 = -1;
        loop {
            if x > 1 {
                break;
            }
            if x == 0 && y == 0 {
                x = x + 1;
                continue;
            }
            let offset = vec2<f32>(f32(x), f32(y)) * step_size;
            let offset_uv = uv + offset;
            var tap = 1.0;
            if offset_uv.x >= 0.0 && offset_uv.x <= 1.0 && offset_uv.y >= 0.0 && offset_uv.y <= 1.0 {
                tap =
                    textureSampleCompare(shadow_map, shadow_sampler, offset_uv, i32(cascade_index), comparison_depth);
            }
            sum = sum + tap;
            taps = taps + 1.0;
            x = x + 1;
        }
        y = y + 1;
    }
    return sum / taps;
}

@fragment
fn fs_main(input : VertexOut) -> @location(0) vec4<f32> {
    var N = normalize(input.normal);
    let V = normalize(frame.camera_pos.xyz - input.world_pos);
    let L = normalize(-frame.light_dir.xyz);
    let H = normalize(V + L);
    let n_dot_v = max(dot(N, V), 0.0);

    let base_sample = textureSample(base_color_tex, material_sampler, input.uv);
    let material_color = material.base_color_factor;
    var base_color = draw.base_color.xyz * material_color.xyz * base_sample.xyz;
    let base_alpha = clamp(base_sample.w * material_color.w, 0.0, 1.0);

    var metallic = material.params.x;
    var roughness = material.params.y;
    let normal_scale = material.params.z;

    if (material.texture_flags.y > 0.5) {
        let mr_sample = textureSample(metallic_roughness_tex, material_sampler, input.uv);
        metallic = metallic * mr_sample.b;
        roughness = roughness * mr_sample.g;
    }

    metallic = clamp(metallic + draw.material_params.x, 0.0, 1.0);
    roughness = clamp(roughness + (draw.material_params.y - 0.5), 0.04, 1.0);

    if (material.texture_flags.z > 0.5) {
        N = apply_normal_map(N, input.tangent, input.uv, normal_scale);
    }

    let n_dot_l = max(dot(N, L), 0.0);
    let f0 = mix(vec3<f32>(0.04), base_color, vec3<f32>(metallic));

    var emissive = draw.emissive.xyz;
    var material_emissive = material.emissive_factor.xyz;
    if (material.texture_flags.w > 0.5) {
        let emissive_sample = textureSample(emissive_tex, material_sampler, input.uv).xyz;
        material_emissive = material_emissive * emissive_sample;
    }
    emissive = emissive + material_emissive;

    let exposure = frame.exposure_params.x;
    var color = frame.ambient_color.xyz * base_color + emissive;
    let receives_shadow = draw.material_params.z > 0.5;
    let shadow_strength = clamp(shadow.params.y, 0.0, 1.0);
    var shadow_factor = 1.0;
    if (receives_shadow && shadow_strength > 0.001) {
        let shadow_sample = sample_shadow(input.world_pos);
        shadow_factor = mix(1.0, shadow_sample, shadow_strength);
    }

    if (n_dot_l > 0.0 && n_dot_v > 0.0) {
        let F = fresnel_schlick(max(dot(H, V), 0.0), f0);
        let D = distribution_ggx(N, H, roughness);
        let G = geometry_smith(N, V, L, roughness);
        let spec = (D * G) * F / max(4.0 * n_dot_v * n_dot_l, 0.001);

        let kd = (vec3<f32>(1.0) - F) * (1.0 - metallic);
        let diffuse = kd * base_color / 3.14159265;
        let radiance = frame.light_color.xyz * n_dot_l * exposure * shadow_factor;
        color = color + (diffuse + spec) * radiance;
    }

    let env_intensity = frame.exposure_params.z;
    if (env_intensity > 0.0001) {
        let mip_count = max(frame.exposure_params.y, 1.0);
        let max_mip = max(mip_count - 1.0, 0.0);
        let irradiance = textureSample(diffuse_env, env_sampler, N).xyz;
        let ks = fresnel_schlick(n_dot_v, f0);
        let kd = (vec3<f32>(1.0) - ks) * (1.0 - metallic);
        let diffuse_ibl = irradiance * base_color * kd;
        let R = reflect(-V, N);
        let lod = roughness * max_mip;
        let prefiltered = textureSampleLevel(specular_env, env_sampler, R, lod).xyz;
        let brdf_sample = textureSample(brdf_lut, env_sampler, vec2<f32>(n_dot_v, roughness)).xy;
        let specular_ibl = prefiltered * (ks * brdf_sample.x + brdf_sample.y);
        color = color + (diffuse_ibl + specular_ibl) * env_intensity;
    }

    color = color
        + shade_clustered_point_lights(
            input.world_pos,
            N,
            V,
            base_color,
            metallic,
            roughness,
            f0,
            input.view_pos,
            input.clip_pos,
        );

    return vec4<f32>(color, base_alpha);
}

fn clamp_int(value : i32, min_value : i32, max_value : i32) -> i32 {
    return max(min_value, min(value, max_value));
}

fn cluster_index_for_fragment(frag_uv : vec2<f32>, view_pos : vec3<f32>) -> i32 {
    let config = cluster_uniform.config;
    if config.stats.x == 0u || config.grid_dims.w == 0u {
        return -1;
    }
    let grid_x = max(config.grid_dims.x, 1u);
    let grid_y = max(config.grid_dims.y, 1u);
    let grid_z = max(config.grid_dims.z, 1u);
    var cluster_x = i32(frag_uv.x * f32(grid_x));
    cluster_x = clamp_int(cluster_x, 0, i32(grid_x) - 1);
    var cluster_y = i32(frag_uv.y * f32(grid_y));
    cluster_y = clamp_int(cluster_y, 0, i32(grid_y) - 1);
    let depth = (-view_pos.z - config.depth_params.x) * config.depth_params.z;
    let depth_clamped = clamp(depth, 0.0, 0.999);
    var cluster_z = i32(depth_clamped * f32(grid_z));
    cluster_z = clamp_int(cluster_z, 0, i32(grid_z) - 1);
    let xy = i32(grid_x) * i32(grid_y);
    return cluster_x + cluster_y * i32(grid_x) + cluster_z * xy;
}

fn load_cluster_record(index : u32) -> ClusterRecord {
    let base = index * CLUSTER_RECORD_STRIDE_WORDS;
    return ClusterRecord(cluster_data_words[base + 0u], cluster_data_words[base + 1u], 0u, 0u);
}

fn load_cluster_light_index(index : u32) -> u32 {
    let indices_offset = cluster_uniform.config.data_meta.z;
    let word_index = index / 2u;
    let packed = cluster_data_words[indices_offset + word_index];
    if (index & 1u) == 0u {
        return packed & 0xFFFFu;
    }
    return (packed >> 16u) & 0xFFFFu;
}

fn shade_point_light(
    world_pos : vec3<f32>,
    normal : vec3<f32>,
    view_dir : vec3<f32>,
    base_color : vec3<f32>,
    metallic : f32,
    roughness : f32,
    f0 : vec3<f32>,
    light : PointLight,
) -> vec3<f32> {
    let radius = max(light.position_radius.w, 0.001);
    let to_light = light.position_radius.xyz - world_pos;
    let dist = length(to_light);
    if dist <= 0.001 || dist > radius {
        return vec3<f32>(0.0);
    }
    let attenuation = pow(max(1.0 - dist / radius, 0.0), 2.0);
    if attenuation <= 0.0 {
        return vec3<f32>(0.0);
    }
    let L = to_light / dist;
    let n_dot_l = max(dot(normal, L), 0.0);
    if n_dot_l <= 0.0 {
        return vec3<f32>(0.0);
    }
    let H = normalize(view_dir + L);
    let n_dot_v = max(dot(normal, view_dir), 0.0);
    let F = fresnel_schlick(max(dot(H, view_dir), 0.0), f0);
    let D = distribution_ggx(normal, H, roughness);
    let G = geometry_smith(normal, view_dir, L, roughness);
    let spec = (D * G) * F / max(4.0 * n_dot_v * n_dot_l, 0.001);
    let kd = (vec3<f32>(1.0) - F) * (1.0 - metallic);
    let diffuse = kd * base_color / 3.14159265;
    let radiance = light.color_intensity.xyz * light.color_intensity.w * attenuation * n_dot_l;
    return (diffuse + spec) * radiance;
}

fn shade_clustered_point_lights(
    world_pos : vec3<f32>,
    normal : vec3<f32>,
    view_dir : vec3<f32>,
    base_color : vec3<f32>,
    metallic : f32,
    roughness : f32,
    f0 : vec3<f32>,
    view_pos : vec3<f32>,
    clip_pos : vec4<f32>,
) -> vec3<f32> {
    let config = cluster_uniform.config;
    if config.stats.x == 0u || config.grid_dims.w == 0u {
        return vec3<f32>(0.0);
    }
    if abs(clip_pos.w) <= 1e-5 {
        return vec3<f32>(0.0);
    }
    let ndc = clip_pos.xy / clip_pos.w;
    let frag_uv = vec2<f32>(ndc.x * 0.5 + 0.5, 1.0 - (ndc.y * 0.5 + 0.5));
    let cluster_index = cluster_index_for_fragment(frag_uv, view_pos);
    if cluster_index < 0 {
        return vec3<f32>(0.0);
    }
    let record = load_cluster_record(u32(cluster_index));
    if record.count == 0u {
        return vec3<f32>(0.0);
    }
    var lighting = vec3<f32>(0.0);
    var i : u32 = 0u;
    loop {
        if i >= record.count {
            break;
        }
        let list_index = record.offset + i;
        if list_index >= cluster_uniform.config.data_meta.w {
            break;
        }
        let light_index = load_cluster_light_index(list_index);
        if light_index < config.stats.x {
            lighting = lighting
                + shade_point_light(
                    world_pos,
                    normal,
                    view_dir,
                    base_color,
                    metallic,
                    roughness,
                    f0,
                    cluster_uniform.lights[light_index],
                );
        }
        i = i + 1u;
    }
    return lighting;
}


