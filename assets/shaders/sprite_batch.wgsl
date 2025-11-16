// Batched sprite shader
struct Globals {
  proj: mat4x4<f32>,
};

struct VSOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) uv: vec2<f32>,
  @location(1) color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u_globals: Globals;

struct VIn {
  @location(0) pos: vec3<f32>,
  @location(1) uv: vec2<f32>,
};
struct IIn {
  @location(2) axis_x: vec4<f32>,
  @location(3) axis_y: vec4<f32>,
  @location(4) translation: vec4<f32>,
  @location(5) uv_rect: vec4<f32>,
  @location(6) tint: vec4<f32>,
};
@group(1) @binding(0) var t_atlas: texture_2d<f32>;
@group(1) @binding(1) var s_linear: sampler;
@vertex
fn vs_main(v: VIn, i: IIn) -> VSOut {
  let dx = v.pos.x;
  let dy = v.pos.y;
  let world = vec4<f32>(
    i.translation.x + i.axis_x.x * dx + i.axis_y.x * dy,
    i.translation.y + i.axis_x.y * dx + i.axis_y.y * dy,
    i.translation.z + i.axis_x.z * dx + i.axis_y.z * dy,
    1.0
  );
  var out: VSOut;
  out.pos = u_globals.proj * world;
  out.uv = vec2<f32>(mix(i.uv_rect.x, i.uv_rect.z, v.uv.x), mix(i.uv_rect.y, i.uv_rect.w, v.uv.y));
  out.color = i.tint;
  return out;
}
@fragment
fn fs_main(input: VSOut) -> @location(0) vec4<f32> {
  return textureSample(t_atlas, s_linear, input.uv) * input.color;
}
