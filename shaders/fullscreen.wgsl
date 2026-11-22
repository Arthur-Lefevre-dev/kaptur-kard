// Letterboxed present with optional YUY2 / NV12 → RGB conversion.
// Supports limited/full luma range and BT.601 / BT.709 matrices.

struct Params {
    // xy = content scale in UV, zw = offset
    scale_offset: vec4<f32>,
    // x = format: 0=BGRA, 1=YUY2, 2=NV12
    // y = range: 0=full (0–255), 1=limited (16–235)
    // z = matrix: 0=BT.601, 1=BT.709
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

fn yuv_to_rgb(y: f32, u: f32, v: f32, limited: bool, bt709: bool) -> vec3<f32> {
    let d = u - 0.5;
    let e = v - 0.5;
    var yy = y;
    var kr = 0.0;
    var kg_u = 0.0;
    var kg_v = 0.0;
    var kb = 0.0;

    if (limited) {
        // Studio swing: Y 16–235 → 0–1 via (Y-16)*(255/219)
        yy = (y - (16.0 / 255.0)) * (255.0 / 219.0);
        if (bt709) {
            // BT.709 limited
            kr = 1.793;
            kg_u = 0.213;
            kg_v = 0.533;
            kb = 2.112;
        } else {
            // BT.601 limited
            kr = 1.596;
            kg_u = 0.392;
            kg_v = 0.813;
            kb = 2.017;
        }
        // yy already expanded; chroma uses same Kr/Kb form on centered U/V
        let r = clamp(yy + kr * e, 0.0, 1.0);
        let g = clamp(yy - kg_u * d - kg_v * e, 0.0, 1.0);
        let b = clamp(yy + kb * d, 0.0, 1.0);
        return vec3<f32>(r, g, b);
    } else {
        // Full range (PC levels): Y already 0–1
        if (bt709) {
            kr = 1.575;
            kg_u = 0.187;
            kg_v = 0.468;
            kb = 1.856;
        } else {
            kr = 1.402;
            kg_u = 0.344;
            kg_v = 0.714;
            kb = 1.772;
        }
        let r = clamp(yy + kr * e, 0.0, 1.0);
        let g = clamp(yy - kg_u * d - kg_v * e, 0.0, 1.0);
        let b = clamp(yy + kb * d, 0.0, 1.0);
        return vec3<f32>(r, g, b);
    }
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
    let limited = params.format_pad.y != 0u;
    let bt709 = params.format_pad.z != 0u;

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
        let rgb = yuv_to_rgb(y, u, v, limited, bt709);
        return vec4<f32>(rgb, 1.0);
    }

    // NV12: Y plane + UV plane
    let y = textureSampleLevel(tex_primary, frame_sampler, content_uv, 0.0).r;
    let uv = textureSampleLevel(tex_uv, frame_sampler, content_uv, 0.0).rg;
    let rgb = yuv_to_rgb(y, uv.r, uv.g, limited, bt709);
    return vec4<f32>(rgb, 1.0);
}
