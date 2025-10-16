// Batched sprite shader
struct Globals {
  proj: mat4x4<f32>,
};

struct VSOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) uv: vec2<f32>,
};

@group(0) @binding(0) var<uniform> u_globals: Globals;

struct VIn {
  @location(0) pos: vec3<f32>,
  @location(1) uv: vec2<f32>,
};
struct IIn {
  @location(2) m0: vec4<f32>,
  @location(3) m1: vec4<f32>,
  @location(4) m2: vec4<f32>,
  @location(5) m3: vec4<f32>,
  @location(6) uv_rect: vec4<f32>,
};
@group(1) @binding(0) var t_atlas: texture_2d<f32>;
@group(1) @binding(1) var s_linear: sampler;
@vertex
fn vs_main(v: VIn, i: IIn) -> VSOut {
  let model = mat4x4<f32>(i.m0, i.m1, i.m2, i.m3);
  var out: VSOut;
  out.pos = u_globals.proj * model * vec4<f32>(v.pos, 1.0);
  out.uv = vec2<f32>(mix(i.uv_rect.x, i.uv_rect.z, v.uv.x), mix(i.uv_rect.y, i.uv_rect.w, v.uv.y));
  return out;
}
@fragment
fn fs_main(input: VSOut) -> @location(0) vec4<f32> {
  return textureSample(t_atlas, s_linear, input.uv);
}
