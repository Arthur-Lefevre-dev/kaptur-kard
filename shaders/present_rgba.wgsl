// Letterboxed present of a single RGBA texture (post-upscale).

struct Params {
    scale_offset: vec4<f32>,
    // x = 1 → textureLoad (1:1 / integer), 0 → bilinear sample (fractional fit)
    flags: vec4<u32>,
}

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> params: Params;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VsOut {
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
    let scale = params.scale_offset.xy;
    let offset = params.scale_offset.zw;
    let content_uv = (in.uv - offset) / scale;
    if (content_uv.x < 0.0 || content_uv.x > 1.0 || content_uv.y < 0.0 || content_uv.y > 1.0) {
        return vec4<f32>(0.02, 0.02, 0.02, 1.0);
    }
    // Nearest path: integer / 1:1 present must not bilinear-blend neighboring texels.
    if (params.flags.x != 0u) {
        let dims = vec2<f32>(textureDimensions(tex));
        let coord = vec2<i32>(floor(clamp(content_uv * dims, vec2<f32>(0.0), dims - vec2<f32>(1.0))));
        return textureLoad(tex, coord, 0);
    }
    return textureSampleLevel(tex, samp, content_uv, 0.0);
}
