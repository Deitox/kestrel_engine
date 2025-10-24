struct ShadowFrame {
    light_view_proj : mat4x4<f32>,
    params : vec4<f32>,
}

struct ShadowDraw {
    model : mat4x4<f32>,
}

struct VertexIn {
    @location(0) position : vec3<f32>,
    @location(1) normal : vec3<f32>,
    @location(2) tangent : vec4<f32>,
    @location(3) uv : vec2<f32>,
}

struct VertexOut {
    @builtin(position) position : vec4<f32>,
}

@group(0) @binding(0)
var<uniform> frame : ShadowFrame;

@group(1) @binding(0)
var<uniform> draw : ShadowDraw;

@vertex
fn vs_main(input : VertexIn) -> VertexOut {
    var out : VertexOut;
    let world_pos = draw.model * vec4<f32>(input.position, 1.0);
    out.position = frame.light_view_proj * world_pos;
    return out;
}

