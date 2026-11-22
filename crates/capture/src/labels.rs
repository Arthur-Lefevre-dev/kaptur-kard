//! Human-readable labels for resolutions and frame rates.

use crate::format::Rational;

/// Resolution label, e.g. `1080p (Full HD)`.
pub fn resolution_label(width: u32, height: u32) -> String {
    let tag = match (width, height) {
        (3840, 2160) | (4096, 2160) => Some("4K (UHD)"),
        (2560, 1440) => Some("1440p (QHD)"),
        (1920, 1080) => Some("1080p (Full HD)"),
        (1280, 720) => Some("720p (HD)"),
        (640, 480) => Some("480p"),
        _ => None,
    };
    match tag {
        Some(t) => format!("{height}p · {t}"),
        None => format!("{width}×{height}"),
    }
}

/// Display Hz rounded for NTSC fractions: 59.94 → 60, 29.97 → 30.
pub fn hz_label(rate: Rational) -> String {
    format!("{} Hz", display_hz(rate))
}

pub fn display_hz(rate: Rational) -> u32 {
    let fps = rate.as_f64();
    if (fps - 59.94).abs() < 0.1 || (fps - 60.0).abs() < 0.1 {
        return 60;
    }
    if (fps - 29.97).abs() < 0.1 || (fps - 30.0).abs() < 0.1 {
        return 30;
    }
    if (fps - 23.976).abs() < 0.1 {
        return 24;
    }
    fps.round().max(1.0) as u32
}

/// Rank devices so likely capture cards appear first.
pub fn device_capture_score(display_name: &str) -> i32 {
    let n = display_name.to_ascii_lowercase();
    let mut score = 0i32;
    for kw in [
        "elgato",
        "cam link",
        "avermedia",
        "magewell",
        "hdmi",
        "capture",
        "live gamer",
        "intensity",
        "4k60",
        "uvc",
        "usb video",
        "game capture",
    ] {
        if n.contains(kw) {
            score += 10;
        }
    }
    for kw in [
        "integrated",
        "ir camera",
        "infrared",
        "face time",
        "facetime",
        "webcam",
        "hd camera",
    ] {
        if n.contains(kw) {
            score -= 15;
        }
    }
    if n.contains("mock") {
        score -= 5;
    }
    score
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_1080_and_5994() {
        assert!(resolution_label(1920, 1080).contains("Full HD"));
        assert_eq!(display_hz(Rational::new(60000, 1001)), 60);
        assert_eq!(display_hz(Rational::new(30000, 1001)), 30);
    }

    #[test]
    fn ranks_elgato_above_integrated() {
        assert!(device_capture_score("Elgato Cam Link 4K") > device_capture_score("Integrated Camera"));
    }
}
