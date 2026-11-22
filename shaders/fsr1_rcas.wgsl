// FSR 1 RCAS — WGSL compute port for CaptureView.
// Based on AMD FidelityFX FSR 1 (MIT). See licenses/AMD-FSR1-MIT.txt
// Copyright (c) 2021 Advanced Micro Devices, Inc.
//
// Matches FsrRcasF from ffx_fsr1.h (GPUOpen-Effects/FidelityFX-FSR).
// A previous port used the wrong hitMax denominator and min(hitMin,hitMax),
// then clamped the lobe to ≤0 in a way that zeroed sharpening entirely
// ("FSR does nothing") or left a positive lobe ("FSR blurs").

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

    // Luma approx used by AMD noise detection (not Rec.709).
    let b_l = b.b * 0.5 + (b.r * 0.5 + b.g);
    let d_l = d.b * 0.5 + (d.r * 0.5 + d.g);
    let e_l = e.b * 0.5 + (e.r * 0.5 + e.g);
    let f_l = f.b * 0.5 + (f.r * 0.5 + f.g);
    let h_l = hp.b * 0.5 + (hp.r * 0.5 + hp.g);

    var nz = 0.25 * b_l + 0.25 * d_l + 0.25 * f_l + 0.25 * h_l - e_l;
    let nz_range = max(max(max(b_l, d_l), max(e_l, f_l)), h_l)
        - min(min(min(b_l, d_l), min(e_l, f_l)), h_l);
    nz = clamp(abs(nz) * (1.0 / max(nz_range, 1e-5)), 0.0, 1.0);
    nz = -0.5 * nz + 1.0;

    let mn4 = min(min(b, d), min(f, hp));
    let mx4 = max(max(b, d), max(f, hp));

    // Immediate constants for peak range (AMD peakC).
    let peak_x = 1.0;
    let peak_y = -4.0;

    let hit_min = min(mn4, e) * (1.0 / max(vec3<f32>(1e-5), 4.0 * mx4));
    let hit_max = (vec3<f32>(peak_x) - max(mx4, e))
        * (1.0 / max(vec3<f32>(1e-5), 4.0 * mn4 + vec3<f32>(peak_y)));
    // Per-channel lobe uses max(-hitMin, hitMax), then take the strongest
    // (most negative after the ≤0 clamp) across RGB — matches FsrRcasF.
    let lobe_rgb = max(-hit_min, hit_max);
    var lobe = max(lobe_rgb.r, max(lobe_rgb.g, lobe_rgb.b));
    lobe = max(-FSR_RCAS_LIMIT, min(lobe, 0.0)) * params.con.x;
    // Optional denoise path from AMD (#define FSR_RCAS_DENOISE).
    lobe = lobe * nz;

    let rcp_l = 1.0 / (4.0 * lobe + 1.0);
    let outc = vec3<f32>(
        (lobe * b.r + lobe * d.r + lobe * f.r + lobe * hp.r + e.r) * rcp_l,
        (lobe * b.g + lobe * d.g + lobe * f.g + lobe * hp.g + e.g) * rcp_l,
        (lobe * b.b + lobe * d.b + lobe * f.b + lobe * hp.b + e.b) * rcp_l,
    );

    textureStore(
        dst_tex,
        sp,
        vec4<f32>(clamp(outc, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0),
    );
}
