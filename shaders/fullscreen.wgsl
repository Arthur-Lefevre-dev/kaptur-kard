// Fullscreen textured triangle with letterboxed UV mapping driven by uniforms.

struct Letterbox {
    // xy = scale (content / viewport), zw = offset in clip-ish UV space
    scale_offset: vec4<f32>,
}

@group(0) @binding(0) var frame_tex: texture_2d<f32>;
@group(0) @binding(1) var frame_sampler: sampler;
@group(0) @binding(2) var<uniform> letterbox: Letterbox;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VsOut {
    // Fullscreen triangle covering clip space.
    var pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var uv = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(2.0, 1.0),
        vec2<f32>(0.0, -1.0),
    );

    var out: VsOut;
    out.position = vec4<f32>(pos[idx], 0.0, 1.0);
    out.uv = uv[idx];
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let scale = letterbox.scale_offset.xy;
    let offset = letterbox.scale_offset.zw;
    // Map window UV into content UV; outside content → black bars.
    let content_uv = (in.uv - offset) / scale;
    if (content_uv.x < 0.0 || content_uv.x > 1.0 || content_uv.y < 0.0 || content_uv.y > 1.0) {
        return vec4<f32>(0.02, 0.02, 0.02, 1.0);
    }
    return textureSampleLevel(frame_tex, frame_sampler, content_uv, 0.0);
}
