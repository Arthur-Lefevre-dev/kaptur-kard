// FSR 1 EASU — WGSL compute port for CaptureView.
// Based on AMD FidelityFX FSR 1 (MIT). See licenses/AMD-FSR1-MIT.txt
// Copyright (c) 2021 Advanced Micro Devices, Inc.

struct EasuParams {
    con0: vec4<f32>,
    con1: vec4<f32>,
    con2: vec4<f32>,
    con3: vec4<f32>,
    // xy = output size, zw unused
    out_size: vec4<f32>,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_samp: sampler;
@group(0) @binding(2) var dst_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(3) var<uniform> params: EasuParams;

fn easu_tap(
    a_c: ptr<function, vec3<f32>>,
    a_w: ptr<function, f32>,
    off: vec2<f32>,
    dir: vec2<f32>,
    len: vec2<f32>,
    lob: f32,
    clp: f32,
    c: vec3<f32>,
) {
    var v = vec2<f32>(
        (off.x * dir.x) + (off.y * dir.y),
        (off.x * (-dir.y)) + (off.y * dir.x),
    );
    v = v * len;
    var d2 = v.x * v.x + v.y * v.y;
    d2 = min(d2, clp);
    var wB = 2.0 / 5.0 * d2 - 1.0;
    var wA = lob * d2 - 1.0;
    wB = wB * wB;
    wA = wA * wA;
    wB = 25.0 / 16.0 * wB - 1.0;
    let w = wB * wA;
    *a_c = *a_c + c * w;
    *a_w = *a_w + w;
}

fn load_rgb(p: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(src_tex, src_samp, p, 0.0).rgb;
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let out_w = u32(params.out_size.x);
    let out_h = u32(params.out_size.y);
    if (gid.x >= out_w || gid.y >= out_h) {
        return;
    }

    let pp = vec2<f32>(f32(gid.x), f32(gid.y)) * params.con0.xy + params.con0.zw;
    let fp = floor(pp);
    let dir_init = pp - fp;

    // Gather neighborhood around F (upper-left of 2x2).
    let p0 = fp * params.con1.xy + params.con1.zw;
    let b = load_rgb(p0 + vec2<f32>(0.0, -params.con1.y));
    let c = load_rgb(p0 + params.con1.zw + vec2<f32>(params.con1.x, 0.0));
    // Simplified taps for edge direction (green channel luma proxy).
    let i = load_rgb(p0 + vec2<f32>(-params.con1.x, params.con1.y));
    let j = load_rgb(p0 + vec2<f32>(0.0, params.con1.y));
    let f = load_rgb(p0);
    let e = load_rgb(p0 + vec2<f32>(-params.con1.x, 0.0));
    let k = load_rgb(p0 + vec2<f32>(params.con1.x, params.con1.y));
    let g = load_rgb(p0 + vec2<f32>(params.con1.x, 0.0));
    let o = load_rgb(p0 + vec2<f32>(0.0, 2.0 * params.con1.y));
    let n = load_rgb(p0 + vec2<f32>(-params.con1.x, 2.0 * params.con1.y));
    let h = load_rgb(p0 + vec2<f32>(2.0 * params.con1.x, 0.0));
    let l = load_rgb(p0 + vec2<f32>(2.0 * params.con1.x, params.con1.y));

    // Luma approx.
    let bL = b.g;
    let cL = c.g;
    let iL = i.g;
    let jL = j.g;
    let fL = f.g;
    let eL = e.g;
    let kL = k.g;
    let gL = g.g;
    let oL = o.g;
    let nL = n.g;
    let hL = h.g;
    let lL = l.g;

    var len = vec2<f32>(0.0);
    var a_c = vec3<f32>(0.0);
    var a_w = 0.0;

    // Stable 2D direction estimate (AMD EASU-style neighborhood).
    let dx = (cL + gL + lL + kL) - (bL + fL + jL + iL);
    let dy = (iL + jL + kL + lL) - (bL + eL + fL + gL) + (nL + oL) - (fL + gL);
    var dir = vec2<f32>(dx, dy);
    let dir_r = length(dir);
    if (dir_r > 1e-5) {
        dir = dir / dir_r;
    } else {
        dir = vec2<f32>(1.0, 0.0);
    }

    // Length / lobe shaped by stretch.
    let stretch = (dir.x * dir.x) / max(abs(dir.y), 1e-5) + abs(dir.y);
    len.x = 1.0 + (stretch - 1.0) * 0.5;
    len.y = 1.0 / len.x;
    let lob = 0.5 - 0.29 * stretch;
    let clp = 1.0 / lob;

    // 12 taps around resolve (offsets in input pixels).
    let taps = array<vec2<f32>, 12>(
        vec2<f32>(0.0, -1.0), vec2<f32>(1.0, -1.0),
        vec2<f32>(-1.0, 0.0), vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(2.0, 0.0),
        vec2<f32>(-1.0, 1.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0), vec2<f32>(2.0, 1.0),
        vec2<f32>(0.0, 2.0), vec2<f32>(1.0, 2.0),
    );
    let cols = array<vec3<f32>, 12>(b, c, e, f, g, h, i, j, k, l, n, o);
    for (var t = 0; t < 12; t = t + 1) {
        let off = taps[t] - dir_init;
        easu_tap(&a_c, &a_w, off, dir, len, lob, clp, cols[t]);
    }

    var pix = a_c / max(a_w, 1e-5);
    // Ringing clamp to 2x2 neighborhood min/max.
    let min4 = min(min(f, g), min(j, k));
    let max4 = max(max(f, g), max(j, k));
    pix = clamp(pix, min4, max4);

    textureStore(dst_tex, vec2<i32>(i32(gid.x), i32(gid.y)), vec4<f32>(pix, 1.0));
}
