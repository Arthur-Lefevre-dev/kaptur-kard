// FSR 1 RCAS — WGSL compute port for CaptureView.
// Based on AMD FidelityFX FSR 1 (MIT). See licenses/AMD-FSR1-MIT.txt
// Copyright (c) 2021 Advanced Micro Devices, Inc.
//
// Matches FsrRcasF from ffx_fsr1.h (GPUOpen-Effects/FidelityFX-FSR).
//
// BUG (Arthur RX 7800 XT — "FSR does nothing"): a prior port clamped the
// hitMax denominator with max(eps, 4*mn4 + peakC.y). peakC.y is -4, so that
// denom is almost always negative; forcing it positive made hitMax huge +,
// then min(lobe, 0) zeroed the lobe → identity. AMD relies on a signed RCP.

const FSR_RCAS_LIMIT: f32 = 0.25 - (1.0 / 16.0);

struct RcasParams {
    // x = exp2(-sharpness_stops); UI sharp=1 → stops=0 → con=1 (max sharp)
    // y = extra gain (≥1) for CaptureView visibility at high Netteté
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

fn rcp_safe(v: f32) -> f32 {
    // Signed reciprocal; only guard exact zero (AMD uses high-precision RCP).
    // Do NOT force negative denoms (e.g. 4*mn4-4) to +eps — that zeroed RCAS.
    if (abs(v) < 1e-8) {
        return select(-1.0e8, 1.0e8, v >= 0.0);
    }
    return 1.0 / v;
}

fn rcp3(v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(rcp_safe(v.x), rcp_safe(v.y), rcp_safe(v.z));
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
    nz = clamp(abs(nz) * rcp_safe(max(nz_range, 1e-5)), 0.0, 1.0);
    nz = -0.5 * nz + 1.0;

    // Ring min/max — neighbors only (AMD mn4/mx4), do not fold in e.
    let mn4 = min(min(b, d), min(f, hp));
    let mx4 = max(max(b, d), max(f, hp));

    // Immediate constants for peak range (AMD peakC = (1, -4)).
    let peak_x = 1.0;
    let peak_y = -4.0;

    // Signed RCP — do NOT clamp (4*mn4-4) to +eps (that zeroed sharpening).
    let hit_min = mn4 * rcp3(vec3<f32>(4.0) * mx4);
    let hit_max = (vec3<f32>(peak_x) - mx4) * rcp3(vec3<f32>(4.0) * mn4 + vec3<f32>(peak_y));
    let lobe_rgb = max(-hit_min, hit_max);
    var lobe = max(lobe_rgb.r, max(lobe_rgb.g, lobe_rgb.b));
    lobe = max(-FSR_RCAS_LIMIT, min(lobe, 0.0)) * params.con.x;
    // Optional denoise path from AMD (#define FSR_RCAS_DENOISE).
    lobe = lobe * nz;

    // CaptureView boost: con.y ≥ 1 makes Netteté 0.7–1 clearly visible vs Off
    // without changing the RCAS limiter shape (still ≤0 lobe).
    let gain = max(params.con.y, 1.0);
    lobe = max(-FSR_RCAS_LIMIT, lobe * gain);

    let rcp_l = rcp_safe(4.0 * lobe + 1.0);
    var outc = vec3<f32>(
        (lobe * b.r + lobe * d.r + lobe * f.r + lobe * hp.r + e.r) * rcp_l,
        (lobe * b.g + lobe * d.g + lobe * f.g + lobe * hp.g + e.g) * rcp_l,
        (lobe * b.b + lobe * d.b + lobe * f.b + lobe * hp.b + e.b) * rcp_l,
    );

    // Extra high-frequency unsharp so Off↔Mode A is obvious on soft HDMI
    // (RCAS alone can look subtle at 1:1). Strength tracks con.x + gain.
    let usm_w = clamp(params.con.x * (0.45 + 0.55 * (gain - 1.0)), 0.0, 1.35);
    if (usm_w > 0.02) {
        let blur = (b + d + f + hp) * 0.25;
        outc = clamp(outc + (e - blur) * (0.75 * usm_w), vec3<f32>(0.0), vec3<f32>(1.0));
    } else {
        outc = clamp(outc, vec3<f32>(0.0), vec3<f32>(1.0));
    }

    textureStore(dst_tex, sp, vec4<f32>(outc, 1.0));
}
