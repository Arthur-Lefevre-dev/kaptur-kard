// Letterboxed present with optional YUY2 / NV12 → RGB conversion.

struct Params {
    // xy = content scale in UV, zw = offset
    scale_offset: vec4<f32>,
    // x = format: 0=BGRA, 1=YUY2, 2=NV12
    format_pad: vec4<u32>,
}

@group(0) @binding(0) var tex_primary: texture_2d<f32>;
@group(0) @binding(1) var tex_uv: texture_2d<f32>;
@group(0) @binding(2) var frame_sampler: sampler;
@group(0) @binding(3) var<uniform> params: Params;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

fn yuv_to_rgb(y: f32, u: f32, v: f32) -> vec3<f32> {
    let c = y - (16.0 / 255.0);
    let d = u - 0.5;
    let e = v - 0.5;
    let r = clamp(1.164 * c + 1.596 * e, 0.0, 1.0);
    let g = clamp(1.164 * c - 0.392 * d - 0.813 * e, 0.0, 1.0);
    let b = clamp(1.164 * c + 2.017 * d, 0.0, 1.0);
    return vec3<f32>(r, g, b);
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

    let fmt = params.format_pad.x;
    if (fmt == 0u) {
        // BGRA uploaded as Bgra8Unorm — sampled as rgba in shader (wgpu swizzle).
        return textureSampleLevel(tex_primary, frame_sampler, content_uv, 0.0);
    }

    if (fmt == 1u) {
        // YUY2 packed as RGBA8 with width/2: R=Y0 G=U B=Y1 A=V
        let dims = vec2<f32>(textureDimensions(tex_primary));
        let x_luma = content_uv.x * dims.x * 2.0;
        let pair = i32(floor(x_luma * 0.5));
        let odd = (i32(floor(x_luma)) & 1) == 1;
        let y_coord = i32(clamp(content_uv.y * dims.y, 0.0, dims.y - 1.0));
        let sample = textureLoad(tex_primary, vec2<i32>(pair, y_coord), 0);
        let y = select(sample.r, sample.b, odd);
        let u = sample.g;
        let v = sample.a;
        let rgb = yuv_to_rgb(y, u, v);
        return vec4<f32>(rgb, 1.0);
    }

    // NV12: Y plane + UV plane
    let y = textureSampleLevel(tex_primary, frame_sampler, content_uv, 0.0).r;
    let uv = textureSampleLevel(tex_uv, frame_sampler, content_uv, 0.0).rg;
    let rgb = yuv_to_rgb(y, uv.r, uv.g);
    return vec4<f32>(rgb, 1.0);
}
