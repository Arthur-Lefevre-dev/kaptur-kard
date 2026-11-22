// Bob deinterlace: synthesize missing field lines from vertical neighbors.
// Input is a woven interlaced RGBA frame; output is progressive RGBA.

struct Params {
    // xy = size
    size: vec4<f32>,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var dst_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> params: Params;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = u32(params.size.x);
    let h = u32(params.size.y);
    if (gid.x >= w || gid.y >= h) {
        return;
    }
    let p = vec2<i32>(i32(gid.x), i32(gid.y));
    let dims = vec2<i32>(i32(w), i32(h));
    let y = p.y;
    // Keep even lines; interpolate odd lines from above/below (Bob).
    if ((y & 1) == 0) {
        let c = textureLoad(src_tex, p, 0);
        textureStore(dst_tex, p, c);
    } else {
        let above = textureLoad(src_tex, clamp(p + vec2<i32>(0, -1), vec2<i32>(0), dims - vec2<i32>(1)), 0).rgb;
        let below = textureLoad(src_tex, clamp(p + vec2<i32>(0, 1), vec2<i32>(0), dims - vec2<i32>(1)), 0).rgb;
        let mid = (above + below) * 0.5;
        textureStore(dst_tex, p, vec4<f32>(mid, 1.0));
    }
}
