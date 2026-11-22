// FSR 1 RCAS — WGSL compute port for CaptureView.
// Based on AMD FidelityFX FSR 1 (MIT). See licenses/AMD-FSR1-MIT.txt
// Copyright (c) 2021 Advanced Micro Devices, Inc.
//
// Lobe must stay ≤ 0 (sharpen / identity). A positive lobe blurs — that was
// Arthur's "FSR adds blur" bug from a bad hitMin/hitMax port.

const FSR_RCAS_LIMIT: f32 = 0.25 - (1.0 / 16.0);

struct RcasParams {
    // x = exp2(-sharpness_stops); UI sharp=1 → stops=0 → con=1 (max sharp)
    con: vec4<f32>,
    // xy = size
    size: vec4<f32>,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var dst_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> params: RcasParams;

fn load(p: vec2<i32>) -> vec3<f32> {
    let dims = vec2<i32>(textureDimensions(src_tex));
    let c = clamp(p, vec2<i32>(0), dims - vec2<i32>(1));
    return textureLoad(src_tex, c, 0).rgb;
}

fn lobe_channel(b: f32, d: f32, e: f32, f: f32, h: f32) -> f32 {
    // AMD FsrRcasF ring min/max (b,d,f,h) then limit vs center e.
    let mn4 = min(min(b, d), min(f, h));
    let mx4 = max(max(b, d), max(f, h));
    // hitMin / hitMax from ffx_fsr1.h — both drive a non-positive lobe.
    let hit_min = min(mn4, e) * (1.0 / max(1e-5, 4.0 * mx4));
    let hit_max = (1.0 - max(mx4, e)) * (1.0 / max(1e-5, 4.0 - 4.0 * mx4));
    return max(-FSR_RCAS_LIMIT, min(hit_min, hit_max));
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = u32(params.size.x);
    let h = u32(params.size.y);
    if (gid.x >= w || gid.y >= h) {
        return;
    }
    let sp = vec2<i32>(i32(gid.x), i32(gid.y));
    let b = load(sp + vec2<i32>(0, -1));
    let d = load(sp + vec2<i32>(-1, 0));
    let e = load(sp);
    let f = load(sp + vec2<i32>(1, 0));
    let hp = load(sp + vec2<i32>(0, 1));

    var lobe = min(
        min(lobe_channel(b.r, d.r, e.r, f.r, hp.r), lobe_channel(b.g, d.g, e.g, f.g, hp.g)),
        lobe_channel(b.b, d.b, e.b, f.b, hp.b),
    );
    // Never allow a positive lobe (would blur). Scale by sharpness constant.
    lobe = min(0.0, lobe) * params.con.x;

    // Noise detection highpass (AMD): reduce sharpening on noisy flats.
    let nz = 0.25 * b.r + 0.25 * d.r + 0.25 * f.r + 0.25 * hp.r - e.r;
    let nz_g = 0.25 * b.g + 0.25 * d.g + 0.25 * f.g + 0.25 * hp.g - e.g;
    let nz_b = 0.25 * b.b + 0.25 * d.b + 0.25 * f.b + 0.25 * hp.b - e.b;
    let nz_amp = max(abs(nz), max(abs(nz_g), abs(nz_b)));
    let mn4 = min(min(min(b, d), min(f, hp)), e);
    let mx4 = max(max(max(b, d), max(f, hp)), e);
    let contrast = max(mx4.r - mn4.r, max(mx4.g - mn4.g, mx4.b - mn4.b));
    let noise = clamp(nz_amp * (1.0 / max(contrast, 1e-5)), 0.0, 1.0);
    lobe = lobe * (1.0 - 0.5 * noise);

    let denom = 4.0 * lobe + 1.0;
    let r = (lobe * b.r + lobe * d.r + lobe * f.r + lobe * hp.r + e.r) / denom;
    let g = (lobe * b.g + lobe * d.g + lobe * f.g + lobe * hp.g + e.g) / denom;
    let bl = (lobe * b.b + lobe * d.b + lobe * f.b + lobe * hp.b + e.b) / denom;

    textureStore(dst_tex, sp, vec4<f32>(clamp(vec3<f32>(r, g, bl), vec3<f32>(0.0), vec3<f32>(1.0)), 1.0));
}
