//! Color conversion helpers for the present shader (range + YUV matrix).

use capture::PixelFormat;

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuPixelMode {
    Bgra = 0,
    Yuy2 = 1,
    Nv12 = 2,
}

/// Luma/chroma digital range for YUV → RGB.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorRangeMode {
    /// Prefer limited for capture cards / HDMI; full otherwise.
    #[default]
    Auto,
    /// Studio swing 16–235 (typical HDMI / capture cards).
    Limited,
    /// PC levels 0–255 (many webcams / virtual cams).
    Full,
}

impl ColorRangeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Limited => "limited",
            Self::Full => "full",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "limited" | "Limited" => Self::Limited,
            "full" | "Full" => Self::Full,
            _ => Self::Auto,
        }
    }

    /// Cycle Limited ↔ Full for the UI “try the other mode” control.
    /// Auto resolves first, then toggles the concrete range.
    pub fn toggle_concrete(self, resolved_limited: bool) -> Self {
        match self {
            Self::Limited => Self::Full,
            Self::Full => Self::Limited,
            Self::Auto => {
                if resolved_limited {
                    Self::Full
                } else {
                    Self::Limited
                }
            }
        }
    }
}

/// YUV ↔ RGB matrix.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorMatrixMode {
    /// BT.601 for SD (&lt; 720p), BT.709 for HD+.
    #[default]
    Auto,
    Bt601,
    Bt709,
}

impl ColorMatrixMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Bt601 => "bt601",
            Self::Bt709 => "bt709",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "bt601" | "BT.601" | "601" => Self::Bt601,
            "bt709" | "BT.709" | "709" => Self::Bt709,
            _ => Self::Auto,
        }
    }
}

/// Resolved GPU flags written into the fullscreen uniform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorConvertParams {
    pub limited: bool,
    pub bt709: bool,
}

/// True when this source is likely HDMI / capture-card studio levels.
pub fn prefer_limited_range(display_name: &str, device_id: &str) -> bool {
    let n = display_name.to_ascii_lowercase();
    let id = device_id.to_ascii_lowercase();
    if id.contains("vcamdevapi") || n.contains("virtual") || n.contains("virtuelle") {
        return false;
    }
    for kw in [
        "nzxt",
        "signal",
        "hd60",
        "elgato",
        "cam link",
        "avermedia",
        "magewell",
        "blackmagic",
        "intensity",
        "decklink",
        "live gamer",
        "game capture",
        "capture",
        "hdmi",
        "usb video",
        "uvc",
    ] {
        if n.contains(kw) {
            return true;
        }
    }
    // Generic USB capture nodes often use studio range on Windows.
    id.contains("vid_1e71") // NZXT
        || id.contains("vid_0fd9") // Elgato
        || id.contains("vid_07ca") // AVerMedia
}

pub fn resolve_color_convert(
    range: ColorRangeMode,
    matrix: ColorMatrixMode,
    width: u32,
    height: u32,
    display_name: &str,
    device_id: &str,
) -> ColorConvertParams {
    let limited = match range {
        ColorRangeMode::Limited => true,
        ColorRangeMode::Full => false,
        ColorRangeMode::Auto => prefer_limited_range(display_name, device_id),
    };
    let bt709 = match matrix {
        ColorMatrixMode::Bt709 => true,
        ColorMatrixMode::Bt601 => false,
        ColorMatrixMode::Auto => height >= 720 || width >= 1280,
    };
    ColorConvertParams { limited, bt709 }
}

pub fn gpu_mode(format: PixelFormat) -> Option<GpuPixelMode> {
    match format {
        PixelFormat::Bgra8 | PixelFormat::Rgb24 => Some(GpuPixelMode::Bgra),
        PixelFormat::Yuy2 => Some(GpuPixelMode::Yuy2),
        PixelFormat::Nv12 => Some(GpuPixelMode::Nv12),
        PixelFormat::Mjpeg | PixelFormat::Unknown(_) => None,
    }
}

pub fn needs_yuv_convert(format: PixelFormat) -> bool {
    matches!(format, PixelFormat::Nv12 | PixelFormat::Yuy2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nzxt_defaults_to_limited_hd_bt709() {
        assert!(prefer_limited_range(
            "NZXT Signal HD60 Video",
            "mf:\\\\?\\usb#vid_1e71&pid_2701"
        ));
        let p = resolve_color_convert(
            ColorRangeMode::Auto,
            ColorMatrixMode::Auto,
            1920,
            1080,
            "NZXT Signal HD60 Video",
            "mf:usb#vid_1e71",
        );
        assert!(p.limited);
        assert!(p.bt709);
    }

    #[test]
    fn virtual_cam_defaults_to_full() {
        assert!(!prefer_limited_range(
            "S24 (Caméra virtuelle Windows)",
            "mf:\\\\?\\swd#vcamdevapi#abc"
        ));
        let p = resolve_color_convert(
            ColorRangeMode::Auto,
            ColorMatrixMode::Auto,
            1280,
            720,
            "S24 (Caméra virtuelle Windows)",
            "mf:vcamdevapi",
        );
        assert!(!p.limited);
        assert!(p.bt709);
    }

    #[test]
    fn sd_auto_matrix_is_601() {
        let p = resolve_color_convert(
            ColorRangeMode::Limited,
            ColorMatrixMode::Auto,
            720,
            480,
            "capture",
            "mf:usb",
        );
        assert!(p.limited);
        assert!(!p.bt709);
    }

    #[test]
    fn toggle_swaps_limited_full() {
        assert_eq!(
            ColorRangeMode::Limited.toggle_concrete(true),
            ColorRangeMode::Full
        );
        assert_eq!(
            ColorRangeMode::Auto.toggle_concrete(true),
            ColorRangeMode::Full
        );
    }
}
