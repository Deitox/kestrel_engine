struct ShadowFrame {
    light_view_proj : mat4x4<f32>,
    params : vec4<f32>,
}

const MAX_SKIN_JOINTS : u32 = 256u;

struct ShadowDraw {
    model : mat4x4<f32>,
    joint_count : u32,
    _padding : vec3<u32>,
}

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
}

@group(0) @binding(0)
var<uniform> frame : ShadowFrame;

@group(1) @binding(0)
var<uniform> draw : ShadowDraw;

struct SkinPalette {
    matrices : array<mat4x4<f32>, MAX_SKIN_JOINTS>,
}

@group(2) @binding(0)
var<uniform> skinning : SkinPalette;

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
    let skin_matrix = accumulate_skin(input.joints, input.weights, draw.joint_count);
    let skinned_position = skin_matrix * vec4<f32>(input.position, 1.0);
    let world_pos = draw.model * skinned_position;
    out.position = frame.light_view_proj * world_pos;
    return out;
}



