// Dense block-matching optical flow (prev → curr) for CaptureView x2 interpolation.
// Coarse per-pixel search on luma; output RG = motion in pixels (curr - prev).

struct FlowParams {
    // xy = size, z = search radius (pixels), w unused
    size: vec4<f32>,
}

@group(0) @binding(0) var prev_tex: texture_2d<f32>;
@group(0) @binding(1) var curr_tex: texture_2d<f32>;
@group(0) @binding(2) var flow_tex: texture_storage_2d<rg32float, write>;
@group(0) @binding(3) var<uniform> params: FlowParams;

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn load_luma(tex: texture_2d<f32>, p: vec2<i32>, dims: vec2<i32>) -> f32 {
    let c = clamp(p, vec2<i32>(0), dims - vec2<i32>(1));
    return luma(textureLoad(tex, c, 0).rgb);
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = u32(params.size.x);
    let h = u32(params.size.y);
    if (gid.x >= w || gid.y >= h) {
        return;
    }
    let dims = vec2<i32>(i32(w), i32(h));
    let p = vec2<i32>(i32(gid.x), i32(gid.y));
    let radius = i32(params.size.z);

    // Skip every other pixel for speed; neighbors fill via bilinear in warp.
    // Still write all pixels with a small search for quality at 720p+.
    var best_sad = 1e10;
    var best = vec2<i32>(0, 0);

    // 3x3 patch SAD + search window.
    for (var dy = -radius; dy <= radius; dy = dy + 1) {
        for (var dx = -radius; dx <= radius; dx = dx + 1) {
            var sad = 0.0;
            for (var oy = -1; oy <= 1; oy = oy + 1) {
                for (var ox = -1; ox <= 1; ox = ox + 1) {
                    let a = load_luma(prev_tex, p + vec2<i32>(ox, oy), dims);
                    let b = load_luma(curr_tex, p + vec2<i32>(ox + dx, oy + dy), dims);
                    sad = sad + abs(a - b);
                }
            }
            // Slight preference for zero motion.
            let cost = sad + 0.002 * f32(abs(dx) + abs(dy));
            if (cost < best_sad) {
                best_sad = cost;
                best = vec2<i32>(dx, dy);
            }
        }
    }

    textureStore(flow_tex, p, vec4<f32>(f32(best.x), f32(best.y), 0.0, 0.0));
}
