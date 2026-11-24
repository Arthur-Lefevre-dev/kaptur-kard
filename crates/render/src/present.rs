//! Letterbox / integer / 1:1 present math for fitting content into the swapchain.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentFilter {
    /// Pixel-perfect 1:1 or integer magnification — avoids bilinear softness.
    Nearest,
    /// Fractional contain-fit (upscale or downscale).
    Linear,
}

#[derive(Clone, Copy, Debug)]
pub struct LetterboxUniforms {
    /// xy = content scale in UV, zw = offset in UV.
    pub scale_offset: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
pub struct PresentFit {
    pub scale_offset: [f32; 4],
    pub filter: PresentFilter,
    /// Preferred mid/output size for FSR/NIS (content or integer multiple).
    pub out_w: u32,
    pub out_h: u32,
}

/// Within this many pixels, treat window ≈ content as 1:1 (title bar / DPI jitter).
const NEAR_1TO1_PX: i32 = 16;

/// Prefer 1:1 centered over a tiny fractional stretch when fill scale is below this.
const NEAR_FILL_SCALE: f32 = 1.08;

/// Compute contain-fit letterbox uniforms for a content size inside a viewport.
pub fn compute_letterbox(
    content_w: u32,
    content_h: u32,
    view_w: u32,
    view_h: u32,
) -> LetterboxUniforms {
    let fit = contain_linear(content_w, content_h, view_w, view_h);
    LetterboxUniforms {
        scale_offset: fit.scale_offset,
    }
}

/// Choose a crisp present: 1:1 when window≈content, integer× nearest when it fits,
/// otherwise fractional linear contain-fit.
pub fn compute_present_fit(
    content_w: u32,
    content_h: u32,
    view_w: u32,
    view_h: u32,
) -> PresentFit {
    compute_present_fit_ex(content_w, content_h, view_w, view_h, false)
}

/// Like [`compute_present_fit`], but `fill_viewport` (F11 / borderless) always
/// contain-fits so 1080p capture fills a larger monitor instead of 1:1 gutters.
pub fn compute_present_fit_ex(
    content_w: u32,
    content_h: u32,
    view_w: u32,
    view_h: u32,
    fill_viewport: bool,
) -> PresentFit {
    let cw = content_w.max(1);
    let ch = content_h.max(1);
    let vw = view_w.max(1);
    let vh = view_h.max(1);

    // Exact (or near) 1:1 — nearest, no gutters.
    if near_dim(vw, cw) && near_dim(vh, ch) {
        return pixel_perfect(cw, ch, vw, vh);
    }

    // Integer magnification (2×, 3×, …) — nearest keeps pixels crisp.
    let max_n = (vw / cw).min(vh / ch);
    if max_n >= 2 {
        return integer_fit(cw, ch, vw, vh, max_n);
    }

    // F11 / borderless: always fill the swapchain (contain), never 1:1 bars.
    if fill_viewport {
        return contain_linear(cw, ch, vw, vh);
    }

    // Windowed: content fits with only a slight stretch → keep 1:1 + bars (no soft scale).
    if cw <= vw && ch <= vh {
        let fill = (vw as f32 / cw as f32).min(vh as f32 / ch as f32);
        if fill < NEAR_FILL_SCALE {
            return pixel_perfect(cw, ch, vw, vh);
        }
    }

    contain_linear(cw, ch, vw, vh)
}

/// Output size for FSR/NIS / present mid buffers, snapped to avoid fractional EASU blur.
pub fn present_out_size(content_w: u32, content_h: u32, view_w: u32, view_h: u32) -> (u32, u32) {
    let fit = compute_present_fit(content_w, content_h, view_w, view_h);
    (fit.out_w, fit.out_h)
}

/// Output size with an explicit fill-viewport policy (fullscreen).
pub fn present_out_size_ex(
    content_w: u32,
    content_h: u32,
    view_w: u32,
    view_h: u32,
    fill_viewport: bool,
) -> (u32, u32) {
    let fit = compute_present_fit_ex(content_w, content_h, view_w, view_h, fill_viewport);
    (fit.out_w, fit.out_h)
}

fn near_dim(a: u32, b: u32) -> bool {
    (a as i32 - b as i32).abs() <= NEAR_1TO1_PX
}

/// 1:1 pixel mapping centered in the viewport (crop when content > view).
fn pixel_perfect(cw: u32, ch: u32, vw: u32, vh: u32) -> PresentFit {
    let scale_x = cw as f32 / vw.max(1) as f32;
    let scale_y = ch as f32 / vh.max(1) as f32;
    let offset_x = (1.0 - scale_x) * 0.5;
    let offset_y = (1.0 - scale_y) * 0.5;
    PresentFit {
        scale_offset: [scale_x, scale_y, offset_x, offset_y],
        filter: PresentFilter::Nearest,
        out_w: cw,
        out_h: ch,
    }
}

fn integer_fit(cw: u32, ch: u32, vw: u32, vh: u32, n: u32) -> PresentFit {
    let n = n.max(1);
    let out_w = cw.saturating_mul(n).max(1);
    let out_h = ch.saturating_mul(n).max(1);
    // Present the integer-sized rect 1:1 into the viewport (bars as needed).
    let scale_x = out_w as f32 / vw.max(1) as f32;
    let scale_y = out_h as f32 / vh.max(1) as f32;
    let offset_x = (1.0 - scale_x) * 0.5;
    let offset_y = (1.0 - scale_y) * 0.5;
    PresentFit {
        scale_offset: [scale_x, scale_y, offset_x, offset_y],
        filter: PresentFilter::Nearest,
        out_w,
        out_h,
    }
}

fn contain_linear(content_w: u32, content_h: u32, view_w: u32, view_h: u32) -> PresentFit {
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
    let out_w = (vw * scale_x).round().max(1.0) as u32;
    let out_h = (vh * scale_y).round().max(1.0) as u32;

    PresentFit {
        scale_offset: [scale_x, scale_y, offset_x, offset_y],
        filter: PresentFilter::Linear,
        out_w,
        out_h,
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

    #[test]
    fn exact_1080_is_pixel_perfect_nearest() {
        let fit = compute_present_fit(1920, 1080, 1920, 1080);
        assert_eq!(fit.filter, PresentFilter::Nearest);
        assert_eq!((fit.out_w, fit.out_h), (1920, 1080));
        assert!((fit.scale_offset[0] - 1.0).abs() < 1e-5);
        assert!((fit.scale_offset[1] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn near_1080_window_stays_1to1_nearest() {
        // Client area slightly short of 1080 (title bar / DPI) must not bilinear-stretch.
        let fit = compute_present_fit(1920, 1080, 1920, 1068);
        assert_eq!(fit.filter, PresentFilter::Nearest);
        assert_eq!((fit.out_w, fit.out_h), (1920, 1080));
        assert!(fit.scale_offset[1] > 1.0); // vertical crop
    }

    #[test]
    fn integer_2x_uses_nearest() {
        let fit = compute_present_fit(960, 540, 1920, 1080);
        assert_eq!(fit.filter, PresentFilter::Nearest);
        assert_eq!((fit.out_w, fit.out_h), (1920, 1080));
    }

    #[test]
    fn fractional_upscale_stays_linear() {
        // 1080 → 1440: scale 1.333, not integer — keep filtered contain.
        let fit = compute_present_fit(1920, 1080, 2560, 1440);
        assert_eq!(fit.filter, PresentFilter::Linear);
        assert!(fit.out_w > 1920 && fit.out_h > 1080);
    }

    #[test]
    fn default_720_window_with_1080_content_is_linear_downscale() {
        let fit = compute_present_fit(1920, 1080, 1280, 720);
        assert_eq!(fit.filter, PresentFilter::Linear);
        assert_eq!((fit.out_w, fit.out_h), (1280, 720));
    }

    #[test]
    fn fullscreen_fill_skips_near_1to1_gutters() {
        // Windowed near-fill prefers crisp 1:1 bars; F11 must contain-fill instead.
        let windowed = compute_present_fit_ex(1920, 1080, 2000, 1125, false);
        assert_eq!(windowed.filter, PresentFilter::Nearest);
        assert_eq!((windowed.out_w, windowed.out_h), (1920, 1080));

        let full = compute_present_fit_ex(1920, 1080, 2000, 1125, true);
        assert_eq!(full.filter, PresentFilter::Linear);
        assert!(full.out_w >= 1990 && full.out_h >= 1115);
        assert!((full.scale_offset[0] - 1.0).abs() < 1e-3 || (full.scale_offset[1] - 1.0).abs() < 1e-3);
    }

    #[test]
    fn fullscreen_1440_fills_from_1080() {
        let fit = compute_present_fit_ex(1920, 1080, 2560, 1440, true);
        assert_eq!(fit.filter, PresentFilter::Linear);
        assert!(fit.out_w > 1920 && fit.out_h > 1080);
        assert!((fit.scale_offset[0] - 1.0).abs() < 1e-3);
        assert!((fit.scale_offset[1] - 1.0).abs() < 1e-3);
    }
}
