// FSR 1 RCAS — WGSL compute port for CaptureView.
// Based on AMD FidelityFX FSR 1 (MIT). See licenses/AMD-FSR1-MIT.txt
// Copyright (c) 2021 Advanced Micro Devices, Inc.

const FSR_RCAS_LIMIT: f32 = 0.25 - (1.0 / 16.0);

struct RcasParams {
    // x = exp2(-sharpness_stops), yzw unused
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
    let hpix = load(sp + vec2<i32>(0, 1));

    let bR = b.r; let bG = b.g; let bB = b.b;
    let dR = d.r; let dG = d.g; let dB = d.b;
    let eR = e.r; let eG = e.g; let eB = e.b;
    let fR = f.r; let fG = f.g; let fB = f.b;
    let hR = hpix.r; let hG = hpix.g; let hB = hpix.b;

    // Min/max of ring + center for lobe solve.
    let mn4R = min(min(bR, dR), min(eR, min(fR, hR)));
    let mx4R = max(max(bR, dR), max(eR, max(fR, hR)));
    let mn4G = min(min(bG, dG), min(eG, min(fG, hG)));
    let mx4G = max(max(bG, dG), max(eG, max(fG, hG)));
    let mn4B = min(min(bB, dB), min(eB, min(fB, hB)));
    let mx4B = max(max(bB, dB), max(eB, max(fB, hB)));

    var peakC = vec3<f32>(0.0);
    peakC.x = (1.0 / max(1e-5, mx4R));
    peakC.y = (1.0 / max(1e-5, mx4G));
    peakC.z = (1.0 / max(1e-5, mx4B));

    // Resolve lobe limiting (AMD RCAS).
    var hitMinR = min(mn4R, eR) * (1.0 / max(1e-5, mx4R));
    var hitMinG = min(mn4G, eG) * (1.0 / max(1e-5, mx4G));
    var hitMinB = min(mn4B, eB) * (1.0 / max(1e-5, mx4B));
    var hitMaxR = (peakC.x - max(mx4R, eR)) * (1.0 / max(1e-5, 4.0 - mx4R * 4.0));
    var hitMaxG = (peakC.y - max(mx4G, eG)) * (1.0 / max(1e-5, 4.0 - mx4G * 4.0));
    var hitMaxB = (peakC.z - max(mx4B, eB)) * (1.0 / max(1e-5, 4.0 - mx4B * 4.0));
    // Use simpler stable lobe from AMD reference shape:
    var lobeR = max(-FSR_RCAS_LIMIT, min(hitMinR, hitMaxR));
    var lobeG = max(-FSR_RCAS_LIMIT, min(hitMinG, hitMaxG));
    var lobeB = max(-FSR_RCAS_LIMIT, min(hitMinB, hitMaxB));
    var lobe = max(-FSR_RCAS_LIMIT, min(min(lobeR, lobeG), lobeB)) * params.con.x;

    // Noise detection highpass.
    let nz = 0.25 * bR + 0.25 * dR + 0.25 * fR + 0.25 * hR - eR;
    let nzG = 0.25 * bG + 0.25 * dG + 0.25 * fG + 0.25 * hG - eG;
    let nzB = 0.25 * bB + 0.25 * dB + 0.25 * fB + 0.25 * hB - eB;
    let nzAmp = max(abs(nz), max(abs(nzG), abs(nzB)));
    let contrast = max(mx4R - mn4R, max(mx4G - mn4G, mx4B - mn4B));
    let noise = clamp(abs(nzAmp) * (1.0 / max(contrast, 1e-5)), 0.0, 1.0);
    lobe = lobe * (1.0 - 0.5 * noise);

    let r = (lobe * bR + lobe * dR + lobe * fR + lobe * hR + eR) / (4.0 * lobe + 1.0);
    let g = (lobe * bG + lobe * dG + lobe * fG + lobe * hG + eG) / (4.0 * lobe + 1.0);
    let bl = (lobe * bB + lobe * dB + lobe * fB + lobe * hB + eB) / (4.0 * lobe + 1.0);

    textureStore(dst_tex, sp, vec4<f32>(clamp(vec3<f32>(r, g, bl), vec3<f32>(0.0), vec3<f32>(1.0)), 1.0));
}
