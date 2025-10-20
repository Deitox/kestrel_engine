struct Globals {
    view_proj : mat4x4<f32>,
    model : mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> globals : Globals;

struct VertexIn {
    @location(0) position : vec3<f32>,
    @location(1) normal : vec3<f32>,
};

struct VertexOut {
    @builtin(position) position : vec4<f32>,
    @location(0) normal : vec3<f32>,
};

@vertex
fn vs_main(input : VertexIn) -> VertexOut {
    var out : VertexOut;
    let world_pos = globals.model * vec4<f32>(input.position, 1.0);
    out.position = globals.view_proj * world_pos;
    let world_normal = (globals.model * vec4<f32>(input.normal, 0.0)).xyz;
    out.normal = normalize(world_normal);
    return out;
}

@fragment
fn fs_main(input : VertexOut) -> @location(0) vec4<f32> {
    let light_dir = normalize(vec3<f32>(0.6, 1.0, 0.8));
    let diffuse = max(dot(normalize(input.normal), light_dir), 0.0);
    let base_color = vec3<f32>(0.35, 0.65, 0.95);
    let color = base_color * (0.3 + diffuse * 0.7);
    return vec4<f32>(color, 1.0);
}
