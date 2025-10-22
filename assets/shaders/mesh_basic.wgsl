struct Globals {
    view_proj : mat4x4<f32>,
    model : mat4x4<f32>,
    camera_pos : vec4<f32>,
    light_dir : vec4<f32>,
    base_color : vec4<f32>,
    emissive : vec4<f32>,
    material_params : vec4<f32>,
}

@group(0) @binding(0)
var<uniform> globals : Globals;

struct VertexIn {
    @location(0) position : vec3<f32>,
    @location(1) normal : vec3<f32>,
}

struct VertexOut {
    @builtin(position) position : vec4<f32>,
    @location(0) normal : vec3<f32>,
    @location(1) world_pos : vec3<f32>,
}

@vertex
fn vs_main(input : VertexIn) -> VertexOut {
    var out : VertexOut;
    let world_pos = globals.model * vec4<f32>(input.position, 1.0);
    out.position = globals.view_proj * world_pos;
    let world_normal = (globals.model * vec4<f32>(input.normal, 0.0)).xyz;
    out.normal = normalize(world_normal);
    out.world_pos = world_pos.xyz;
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

@fragment
fn fs_main(input : VertexOut) -> @location(0) vec4<f32> {
    let N = normalize(input.normal);
    let camera_pos = globals.camera_pos.xyz;
    let V = normalize(camera_pos - input.world_pos);
    let L = normalize(-globals.light_dir.xyz);
    let H = normalize(V + L);

    let metallic = clamp(globals.material_params.x, 0.0, 1.0);
    let roughness = clamp(globals.material_params.y, 0.04, 1.0);
    let base_color = globals.base_color.xyz;
    let emissive = globals.emissive.xyz;

    let n_dot_l = max(dot(N, L), 0.0);
    let n_dot_v = max(dot(N, V), 0.0);
    let ambient = 0.03 * base_color;
    var color = ambient + emissive;

    if (n_dot_l > 0.0 && n_dot_v > 0.0) {
        let f0 = mix(vec3<f32>(0.04, 0.04, 0.04), base_color, vec3<f32>(metallic, metallic, metallic));
        let F = fresnel_schlick(max(dot(H, V), 0.0), f0);
        let D = distribution_ggx(N, H, roughness);
        let G = geometry_smith(N, V, L, roughness);
        let spec = (D * G) * F / max(4.0 * n_dot_v * n_dot_l, 0.001);

        let kd = (vec3<f32>(1.0) - F) * (1.0 - metallic);
        let diffuse = kd * base_color / 3.14159265;
        let light_color = vec3<f32>(1.05, 0.98, 0.92);
        let radiance = light_color * n_dot_l;
        color += (diffuse + spec) * radiance;
    }

    return vec4<f32>(color, 1.0);
}
