// NVIDIA Image Scaling — WGSL compute adaptation for CaptureView.
// Based on NVIDIA Image Scaling SDK (MIT). See licenses/NVIDIA-NIS-MIT.txt
// Copyright (c) 2022 NVIDIA CORPORATION & AFFILIATES.
//
// This is a WGSL spatial scaler inspired by NVScaler: 6-tap support +
// directional edge sharpening. Coefficient LUTs are approximated analytically
// for a dependency-light wgpu path (full SDK ships coef textures).

struct NisParams {
    scale: vec4<f32>,      // xy = src/dst scale, zw = sharpness (0..1) + detect
    src_norm: vec4<f32>,   // xy = 1/srcSize
    dst_size: vec4<f32>,   // xy = dst size
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_samp: sampler;
@group(0) @binding(2) var dst_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(3) var<uniform> params: NisParams;

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn tap6(t: f32) -> f32 {
    // Approximate 6-tap scaler support (NIS-like cubic lobe).
    let x = abs(t);
    if (x < 1.0) {
        return ((1.2 * x - 2.2) * x) * x + 1.0;
    } else if (x < 2.0) {
        return (((-0.4 * x + 2.0) * x - 2.8) * x + 0.8);
    } else if (x < 3.0) {
        return (((0.1 * x - 0.6) * x + 0.9) * x - 0.2);
    }
    return 0.0;
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let out_w = u32(params.dst_size.x);
    let out_h = u32(params.dst_size.y);
    if (gid.x >= out_w || gid.y >= out_h) {
        return;
    }

    let dst = vec2<f32>(f32(gid.x) + 0.5, f32(gid.y) + 0.5);
    let src = dst * params.scale.xy;
    let src_uv_base = src * params.src_norm.xy;

    // Horizontal / vertical 6-tap separable scale.
    var sum = vec3<f32>(0.0);
    var wsum = 0.0;
    let fx = fract(src.x) - 0.5;
    let fy = fract(src.y) - 0.5;
    for (var j = -2; j <= 3; j = j + 1) {
        for (var i = -2; i <= 3; i = i + 1) {
            let w = tap6(f32(i) - fx) * tap6(f32(j) - fy);
            let uv = (floor(src) + vec2<f32>(f32(i), f32(j)) + vec2<f32>(0.5)) * params.src_norm.xy;
            let c = textureSampleLevel(src_tex, src_samp, uv, 0.0).rgb;
            sum = sum + c * w;
            wsum = wsum + w;
        }
    }
    var scaled = sum / max(wsum, 1e-5);

    // Directional edge detect + USM-style sharpen (NVScaler spirit).
    let c0 = textureSampleLevel(src_tex, src_samp, src_uv_base, 0.0).rgb;
    let cx1 = textureSampleLevel(src_tex, src_samp, src_uv_base + vec2<f32>(params.src_norm.x, 0.0), 0.0).rgb;
    let cy1 = textureSampleLevel(src_tex, src_samp, src_uv_base + vec2<f32>(0.0, params.src_norm.y), 0.0).rgb;
    let cx0 = textureSampleLevel(src_tex, src_samp, src_uv_base - vec2<f32>(params.src_norm.x, 0.0), 0.0).rgb;
    let cy0 = textureSampleLevel(src_tex, src_samp, src_uv_base - vec2<f32>(0.0, params.src_norm.y), 0.0).rgb;

    let gx = luma(cx1) - luma(cx0);
    let gy = luma(cy1) - luma(cy0);
    let edge = clamp(length(vec2<f32>(gx, gy)) * 4.0, 0.0, 1.0);
    let sharp = params.scale.z; // 0..1
    let detect = params.scale.w;
    let usm = (scaled - c0) * (sharp * (0.35 + 0.65 * edge) * detect);
    var outc = clamp(scaled + usm, vec3<f32>(0.0), vec3<f32>(1.0));

    textureStore(dst_tex, vec2<i32>(i32(gid.x), i32(gid.y)), vec4<f32>(outc, 1.0));
}
