// Copy RGBA via textureLoad → storage (works for storage or sampled sources).
// Used so FSR EASU / NIS always sample a stable filterable/storage-friendly buffer
// after interpolate Mid (storage) in fullscreen.

struct BlitParams {
    size: vec4<f32>, // xy = width, height
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var dst_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> params: BlitParams;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = u32(params.size.x);
    let h = u32(params.size.y);
    if (gid.x >= w || gid.y >= h) {
        return;
    }
    let p = vec2<i32>(i32(gid.x), i32(gid.y));
    let dims = vec2<i32>(textureDimensions(src_tex));
    let c = clamp(p, vec2<i32>(0), dims - vec2<i32>(1));
    let rgba = textureLoad(src_tex, c, 0);
    textureStore(dst_tex, p, rgba);
}
