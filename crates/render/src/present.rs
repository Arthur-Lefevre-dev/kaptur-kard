//! Letterbox math for fitting content into the swapchain.

#[derive(Clone, Copy, Debug)]
pub struct LetterboxUniforms {
    /// xy = content scale in UV, zw = offset in UV.
    pub scale_offset: [f32; 4],
}

/// Compute contain-fit letterbox uniforms for a content size inside a viewport.
pub fn compute_letterbox(
    content_w: u32,
    content_h: u32,
    view_w: u32,
    view_h: u32,
) -> LetterboxUniforms {
    let cw = content_w.max(1) as f32;
    let ch = content_h.max(1) as f32;
    let vw = view_w.max(1) as f32;
    let vh = view_h.max(1) as f32;

    let content_aspect = cw / ch;
    let view_aspect = vw / vh;

    let (scale_x, scale_y) = if view_aspect > content_aspect {
        // Pillarbox: fit height.
        let sx = content_aspect / view_aspect;
        (sx, 1.0)
    } else {
        // Letterbox: fit width.
        let sy = view_aspect / content_aspect;
        (1.0, sy)
    };

    let offset_x = (1.0 - scale_x) * 0.5;
    let offset_y = (1.0 - scale_y) * 0.5;

    LetterboxUniforms {
        scale_offset: [scale_x, scale_y, offset_x, offset_y],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_in_wide_gets_pillarbox() {
        let u = compute_letterbox(100, 100, 200, 100);
        assert!(u.scale_offset[0] < 1.0);
        assert!((u.scale_offset[1] - 1.0).abs() < 1e-5);
    }
}
