struct FrameUniform {
    view_proj : mat4x4<f32>,
    camera_pos : vec4<f32>,
    light_dir : vec4<f32>,
    light_color : vec4<f32>,
    ambient_color : vec4<f32>,
    exposure_params : vec4<f32>,
    padding : vec4<f32>,
}

struct DrawUniform {
    model : mat4x4<f32>,
    base_color : vec4<f32>,
    emissive : vec4<f32>,
    material_params : vec4<f32>,
}

struct MaterialUniform {
    base_color_factor : vec4<f32>,
   emissive_factor : vec4<f32>,
    params : vec4<f32>,
    texture_flags : vec4<f32>,
}

struct ShadowUniform {
    light_view_proj : mat4x4<f32>,
    params : vec4<f32>,
}

@group(0) @binding(0)
var<uniform> frame : FrameUniform;

@group(1) @binding(0)
var<uniform> draw : DrawUniform;

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
var shadow_map : texture_depth_2d;

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

struct VertexIn {
    @location(0) position : vec3<f32>,
    @location(1) normal : vec3<f32>,
    @location(2) tangent : vec4<f32>,
    @location(3) uv : vec2<f32>,
}

struct VertexOut {
    @builtin(position) position : vec4<f32>,
    @location(0) normal : vec3<f32>,
    @location(1) world_pos : vec3<f32>,
    @location(2) tangent : vec4<f32>,
    @location(3) uv : vec2<f32>,
}

@vertex
fn vs_main(input : VertexIn) -> VertexOut {
    var out : VertexOut;
    let world_pos = draw.model * vec4<f32>(input.position, 1.0);
    out.position = frame.view_proj * world_pos;
    let world_normal = (draw.model * vec4<f32>(input.normal, 0.0)).xyz;
    out.normal = normalize(world_normal);
    out.world_pos = world_pos.xyz;
    let world_tangent = (draw.model * vec4<f32>(input.tangent.xyz, 0.0)).xyz;
    out.tangent = vec4<f32>(normalize(world_tangent), input.tangent.w);
    out.uv = input.uv;
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

fn sample_shadow(world_pos : vec3<f32>) -> f32 {
    let clip = shadow.light_view_proj * vec4<f32>(world_pos, 1.0);
    if abs(clip.w) < 1e-5 {
        return 1.0;
    }
    let inv_w = 1.0 / clip.w;
    let ndc = clip.xyz * inv_w;
    let uv = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 {
        return 1.0;
    }
    let depth = clamp(ndc.z * 0.5 + 0.5, 0.0, 1.0);
    let bias = shadow.params.x;
    return textureSampleCompare(shadow_map, shadow_sampler, uv, depth - bias);
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

    return vec4<f32>(color, base_alpha);
}
