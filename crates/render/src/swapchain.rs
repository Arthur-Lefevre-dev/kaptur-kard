//! Surface present-mode and format selection for VRR / HDR on Windows.
//!
//! wgpu 24 maps DX12 `Immediate` to DXGI allow-tearing (G-Sync / FreeSync when
//! the OS reports tearing support). `Rgba16Float` swapchains use scRGB on DX12
//! (and extended-sRGB linear on Vulkan).

/// Snapshot of what the surface / OS can do for present + HDR.
#[derive(Clone, Debug, Default)]
pub struct PresentCaps {
    /// DXGI ALLOW_TEARING / PresentMode::Immediate advertised by the surface.
    pub tearing_supported: bool,
    /// Mailbox (fast vsync) available.
    pub mailbox_supported: bool,
    /// Surface lists `Rgba16Float` (scRGB / HDR-capable swapchain).
    pub hdr_format_supported: bool,
    /// Active present mode after configure.
    pub present_mode: String,
    /// Active surface format after configure.
    pub surface_format: String,
    /// True when the configured format is the HDR (fp16) path.
    pub hdr_active: bool,
}

impl PresentCaps {
    pub fn from_surface(
        caps: &wgpu::SurfaceCapabilities,
        present_mode: wgpu::PresentMode,
        format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            tearing_supported: caps
                .present_modes
                .iter()
                .any(|m| *m == wgpu::PresentMode::Immediate),
            mailbox_supported: caps
                .present_modes
                .iter()
                .any(|m| *m == wgpu::PresentMode::Mailbox),
            hdr_format_supported: caps
                .formats
                .iter()
                .any(|f| *f == wgpu::TextureFormat::Rgba16Float),
            present_mode: present_mode_label(present_mode).into(),
            surface_format: format_label(format).into(),
            hdr_active: format == wgpu::TextureFormat::Rgba16Float,
        }
    }
}

/// Pick a concrete present mode from the VSync preference and surface caps.
///
/// When VSync is off (or Auto→NoVsync): prefer `Immediate` (VRR / tearing), then
/// `Mailbox`, then `AutoNoVsync`. When VSync is on: `Fifo` if listed, else `AutoVsync`.
pub fn pick_present_mode(
    use_vsync: bool,
    caps: &wgpu::SurfaceCapabilities,
) -> wgpu::PresentMode {
    let has = |m: wgpu::PresentMode| caps.present_modes.iter().any(|p| *p == m);
    if use_vsync {
        if has(wgpu::PresentMode::Fifo) {
            return wgpu::PresentMode::Fifo;
        }
        return wgpu::PresentMode::AutoVsync;
    }
    // Unrestricted / VRR-friendly: Immediate enables DXGI PRESENT_ALLOW_TEARING.
    if has(wgpu::PresentMode::Immediate) {
        return wgpu::PresentMode::Immediate;
    }
    if has(wgpu::PresentMode::Mailbox) {
        return wgpu::PresentMode::Mailbox;
    }
    wgpu::PresentMode::AutoNoVsync
}

/// Choose the swapchain format. Prefer fp16 scRGB when HDR output is requested
/// and the surface advertises it; otherwise a non-sRGB 8-bit video-friendly format.
pub fn pick_surface_format(
    caps: &wgpu::SurfaceCapabilities,
    prefer_hdr: bool,
) -> wgpu::TextureFormat {
    if prefer_hdr {
        if let Some(f) = caps
            .formats
            .iter()
            .copied()
            .find(|f| *f == wgpu::TextureFormat::Rgba16Float)
        {
            return f;
        }
    }
    caps.formats
        .iter()
        .copied()
        .find(|f| {
            !f.is_srgb()
                && *f != wgpu::TextureFormat::Rgba16Float
                && *f != wgpu::TextureFormat::Rgb10a2Unorm
        })
        .or_else(|| {
            caps.formats
                .iter()
                .copied()
                .find(|f| !f.is_srgb() && *f != wgpu::TextureFormat::Rgba16Float)
        })
        .or_else(|| caps.formats.iter().copied().find(|f| f.is_srgb()))
        .unwrap_or(caps.formats[0])
}

pub fn present_mode_label(mode: wgpu::PresentMode) -> &'static str {
    match mode {
        wgpu::PresentMode::AutoVsync => "AutoVsync",
        wgpu::PresentMode::AutoNoVsync => "AutoNoVsync",
        wgpu::PresentMode::Fifo => "Fifo",
        wgpu::PresentMode::FifoRelaxed => "FifoRelaxed",
        wgpu::PresentMode::Immediate => "Immediate",
        wgpu::PresentMode::Mailbox => "Mailbox",
    }
}

pub fn format_label(format: wgpu::TextureFormat) -> &'static str {
    match format {
        wgpu::TextureFormat::Bgra8Unorm => "BGRA8",
        wgpu::TextureFormat::Bgra8UnormSrgb => "BGRA8sRGB",
        wgpu::TextureFormat::Rgba8Unorm => "RGBA8",
        wgpu::TextureFormat::Rgba8UnormSrgb => "RGBA8sRGB",
        wgpu::TextureFormat::Rgba16Float => "RGBA16F (scRGB)",
        wgpu::TextureFormat::Rgb10a2Unorm => "RGB10A2",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps_with(
        formats: Vec<wgpu::TextureFormat>,
        present_modes: Vec<wgpu::PresentMode>,
    ) -> wgpu::SurfaceCapabilities {
        wgpu::SurfaceCapabilities {
            formats,
            present_modes,
            alpha_modes: vec![wgpu::CompositeAlphaMode::Opaque],
            usages: wgpu::TextureUsages::RENDER_ATTACHMENT,
        }
    }

    #[test]
    fn no_vsync_prefers_immediate_when_tearing() {
        let caps = caps_with(
            vec![wgpu::TextureFormat::Bgra8Unorm],
            vec![
                wgpu::PresentMode::Fifo,
                wgpu::PresentMode::Mailbox,
                wgpu::PresentMode::Immediate,
            ],
        );
        assert_eq!(
            pick_present_mode(false, &caps),
            wgpu::PresentMode::Immediate
        );
    }

    #[test]
    fn no_vsync_falls_back_to_mailbox() {
        let caps = caps_with(
            vec![wgpu::TextureFormat::Bgra8Unorm],
            vec![wgpu::PresentMode::Fifo, wgpu::PresentMode::Mailbox],
        );
        assert_eq!(pick_present_mode(false, &caps), wgpu::PresentMode::Mailbox);
    }

    #[test]
    fn vsync_prefers_fifo() {
        let caps = caps_with(
            vec![wgpu::TextureFormat::Bgra8Unorm],
            vec![
                wgpu::PresentMode::Fifo,
                wgpu::PresentMode::Immediate,
            ],
        );
        assert_eq!(pick_present_mode(true, &caps), wgpu::PresentMode::Fifo);
    }

    #[test]
    fn hdr_picks_rgba16f_when_requested() {
        let caps = caps_with(
            vec![
                wgpu::TextureFormat::Bgra8UnormSrgb,
                wgpu::TextureFormat::Bgra8Unorm,
                wgpu::TextureFormat::Rgba16Float,
            ],
            vec![wgpu::PresentMode::Fifo],
        );
        assert_eq!(
            pick_surface_format(&caps, true),
            wgpu::TextureFormat::Rgba16Float
        );
        assert_eq!(
            pick_surface_format(&caps, false),
            wgpu::TextureFormat::Bgra8Unorm
        );
    }

    #[test]
    fn hdr_falls_back_when_fp16_missing() {
        let caps = caps_with(
            vec![
                wgpu::TextureFormat::Bgra8UnormSrgb,
                wgpu::TextureFormat::Bgra8Unorm,
            ],
            vec![wgpu::PresentMode::Fifo],
        );
        assert_eq!(
            pick_surface_format(&caps, true),
            wgpu::TextureFormat::Bgra8Unorm
        );
    }

    #[test]
    fn present_caps_flags_tearing_and_hdr() {
        let caps = caps_with(
            vec![wgpu::TextureFormat::Rgba16Float],
            vec![wgpu::PresentMode::Immediate],
        );
        let snap = PresentCaps::from_surface(
            &caps,
            wgpu::PresentMode::Immediate,
            wgpu::TextureFormat::Rgba16Float,
        );
        assert!(snap.tearing_supported);
        assert!(snap.hdr_format_supported);
        assert!(snap.hdr_active);
        assert_eq!(snap.present_mode, "Immediate");
    }
}
