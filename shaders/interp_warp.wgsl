// Warp prev/curr with optical flow to synthesize the mid frame (t = 0.5).
// Optional motion blur: multi-tap along flow when size.w > 0 (light cost).

struct WarpParams {
    // xy = size, z = blend t (0.5), w = motion blur intensity (0 = off)
    size: vec4<f32>,
}

@group(0) @binding(0) var prev_tex: texture_2d<f32>;
@group(0) @binding(1) var curr_tex: texture_2d<f32>;
@group(0) @binding(2) var flow_tex: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;
@group(0) @binding(4) var mid_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(5) var<uniform> params: WarpParams;

fn sample_at_t(p: vec2<f32>, dims: vec2<f32>, flow: vec2<f32>, t: f32) -> vec3<f32> {
    let prev_uv = clamp((p - flow * t) / dims, vec2<f32>(0.0), vec2<f32>(1.0));
    let curr_uv = clamp((p + flow * (1.0 - t)) / dims, vec2<f32>(0.0), vec2<f32>(1.0));
    let a = textureSampleLevel(prev_tex, samp, prev_uv, 0.0).rgb;
    let b = textureSampleLevel(curr_tex, samp, curr_uv, 0.0).rgb;
    // Occlusion-ish: prefer the sample closer to the source when flow is large.
    let mag = length(flow);
    let w_prev = select(1.0 - t, 0.35, mag > 8.0);
    let w_curr = 1.0 - w_prev;
    return clamp(a * w_prev + b * w_curr, vec3<f32>(0.0), vec3<f32>(1.0));
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = u32(params.size.x);
    let h = u32(params.size.y);
    if (gid.x >= w || gid.y >= h) {
        return;
    }
    let t = params.size.z;
    let blur = params.size.w;
    let p = vec2<f32>(f32(gid.x) + 0.5, f32(gid.y) + 0.5);
    let dims = vec2<f32>(f32(w), f32(h));

    let flow = textureLoad(flow_tex, vec2<i32>(i32(gid.x), i32(gid.y)), 0).xy;

    var outc: vec3<f32>;
    if (blur < 0.001) {
        // Sharp mid: single temporal sample at t.
        outc = sample_at_t(p, dims, flow, t);
    } else {
        // 5-tap smear along flow about mid-time. Extent scales with intensity.
        // Max spread ~0.4 of the prev→curr interval at blur=1 (cheap, smooth).
        let extent = blur * 0.4;
        var acc = vec3<f32>(0.0);
        var wsum = 0.0;
        for (var i = 0; i < 5; i = i + 1) {
            let u = (f32(i) / 4.0) * 2.0 - 1.0; // -1 … +1
            let ti = clamp(t + u * extent, 0.0, 1.0);
            // Triangle weights peak at the center tap.
            let wt = 1.0 - abs(u) * 0.5;
            acc = acc + sample_at_t(p, dims, flow, ti) * wt;
            wsum = wsum + wt;
        }
        outc = clamp(acc / max(wsum, 1e-5), vec3<f32>(0.0), vec3<f32>(1.0));
    }

    textureStore(mid_tex, vec2<i32>(i32(gid.x), i32(gid.y)), vec4<f32>(outc, 1.0));
}
