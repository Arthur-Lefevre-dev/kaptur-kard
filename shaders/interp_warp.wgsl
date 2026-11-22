// Warp prev/curr with optical flow to synthesize the mid frame (t = 0.5).

struct WarpParams {
    // xy = size, z = blend t (0.5), w unused
    size: vec4<f32>,
}

@group(0) @binding(0) var prev_tex: texture_2d<f32>;
@group(0) @binding(1) var curr_tex: texture_2d<f32>;
@group(0) @binding(2) var flow_tex: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;
@group(0) @binding(4) var mid_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(5) var<uniform> params: WarpParams;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = u32(params.size.x);
    let h = u32(params.size.y);
    if (gid.x >= w || gid.y >= h) {
        return;
    }
    let t = params.size.z;
    let p = vec2<f32>(f32(gid.x) + 0.5, f32(gid.y) + 0.5);
    let dims = vec2<f32>(f32(w), f32(h));

    let flow = textureLoad(flow_tex, vec2<i32>(i32(gid.x), i32(gid.y)), 0).xy;
    // Forward/backward half-step.
    let prev_uv = clamp((p - flow * t) / dims, vec2<f32>(0.0), vec2<f32>(1.0));
    let curr_uv = clamp((p + flow * (1.0 - t)) / dims, vec2<f32>(0.0), vec2<f32>(1.0));

    let a = textureSampleLevel(prev_tex, samp, prev_uv, 0.0).rgb;
    let b = textureSampleLevel(curr_tex, samp, curr_uv, 0.0).rgb;
    // Occlusion-ish: prefer the sample closer to the source when flow is large.
    let mag = length(flow);
    let w_prev = select(1.0 - t, 0.35, mag > 8.0);
    let w_curr = 1.0 - w_prev;
    let outc = clamp(a * w_prev + b * w_curr, vec3<f32>(0.0), vec3<f32>(1.0));
    textureStore(mid_tex, vec2<i32>(i32(gid.x), i32(gid.y)), vec4<f32>(outc, 1.0));
}
