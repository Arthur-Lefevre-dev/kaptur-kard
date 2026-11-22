//! Human-readable labels for resolutions and frame rates.

use crate::device::DeviceInfo;
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

/// True for Windows Virtual Camera / Phone Link style sources (not physical cards).
pub fn is_virtual_camera(display_name: &str, id: &str) -> bool {
    let n = display_name.to_ascii_lowercase();
    let idl = id.to_ascii_lowercase();
    if idl.contains("vcamdevapi") || idl.contains("virtualcamera") {
        return true;
    }
    for kw in [
        "caméra virtuelle",
        "camera virtuelle",
        "virtual camera",
        "virtual cam",
        "obs virtual",
        "phone link",
        "mobile cam",
        "iriun",
        "epoccam",
        "droidcam",
        "ivcam",
        "snap camera",
        "manycam",
        "xsplit vapax",
        "nvidia broadcast",
    ] {
        if n.contains(kw) {
            return true;
        }
    }
    false
}

/// Rank devices so capture cards appear first and virtual cameras last.
///
/// Higher is better. Used for sort + default selection.
pub fn device_capture_score(display_name: &str) -> i32 {
    device_capture_score_for(display_name, "")
}

/// Score using both friendly name and device id (symbolic link / path).
pub fn device_capture_score_for(display_name: &str, id: &str) -> i32 {
    let n = display_name.to_ascii_lowercase();
    let mut score = 0i32;

    if is_virtual_camera(display_name, id) {
        score -= 100;
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
        "4k60",
        "hdmi capture",
        "capture card",
        "capture device",
        "usb3",
        "uvc",
    ] {
        if n.contains(kw) {
            score += 20;
        }
    }
    // Weaker positive signals (also match generic UVC nodes).
    for kw in ["hdmi", "capture", "usb video"] {
        if n.contains(kw) {
            score += 8;
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
        "laptop camera",
        "studio camera",
    ] {
        if n.contains(kw) {
            score -= 15;
        }
    }
    if n.contains("mock") {
        score -= 50;
    }
    score
}

/// Sort key: higher score first, devices with formats before empty, name ascending.
pub fn device_sort_key(d: &DeviceInfo) -> (i32, i32, String) {
    let score = device_capture_score_for(&d.display_name, d.id.as_str());
    let formats_rank = if d.formats.is_empty() { 0 } else { 1 };
    (score, formats_rank, d.display_name.to_ascii_lowercase())
}

/// Index of the preferred non-mock capture device (cards over virtual cams).
pub fn pick_preferred_device_index(devices: &[DeviceInfo]) -> Option<usize> {
    devices
        .iter()
        .enumerate()
        .filter(|(_, d)| d.backend != "mock")
        .max_by(|(ia, a), (ib, b)| {
            let ka = device_sort_key(a);
            let kb = device_sort_key(b);
            // Compare score, then formats, then prefer earlier index on total tie
            // (stable vs max_by_key last-wins picking the virtual cam).
            ka.0
                .cmp(&kb.0)
                .then_with(|| ka.1.cmp(&kb.1))
                .then_with(|| kb.2.cmp(&ka.2)) // reverse name → first alphabetically wins on tie
                .then_with(|| ib.cmp(ia))
        })
        .map(|(i, _)| i)
}

/// ComboBox / list label: keep friendly name, annotate virtual or empty caps.
pub fn device_list_label(d: &DeviceInfo) -> String {
    let mut label = d.display_name.clone();
    if is_virtual_camera(&d.display_name, d.id.as_str()) {
        label.push_str(" · virtual");
    }
    if d.formats.is_empty() && d.backend != "mock" {
        label.push_str(" · no modes");
    }
    if let Some(note) = d.enum_note.as_deref() {
        if !note.is_empty() {
            label.push_str(" · ");
            label.push_str(note);
        }
    }
    label
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{DeviceId, FormatDesc};
    use crate::format::PixelFormat;

    fn info(name: &str, id: &str, formats: usize) -> DeviceInfo {
        let formats = (0..formats)
            .map(|_| FormatDesc {
                pixel_format: PixelFormat::Nv12,
                width: 1920,
                height: 1080,
                frame_rates: vec![Rational::new(60, 1)],
                interlaced: false,
            })
            .collect();
        DeviceInfo {
            id: DeviceId::new(id),
            display_name: name.into(),
            backend: "mf",
            formats,
            enum_note: None,
        }
    }

    #[test]
    fn labels_1080_and_5994() {
        assert!(resolution_label(1920, 1080).contains("Full HD"));
        assert_eq!(display_hz(Rational::new(60000, 1001)), 60);
        assert_eq!(display_hz(Rational::new(30000, 1001)), 30);
    }

    #[test]
    fn ranks_elgato_above_integrated() {
        assert!(
            device_capture_score("Elgato Cam Link 4K") > device_capture_score("Integrated Camera")
        );
    }

    #[test]
    fn ranks_nzxt_above_virtual_s24() {
        let nzxt = "NZXT Signal HD60 Video";
        let s24 = "S24 de arthur (Caméra virtuelle Windows)";
        let s24_id = "mf:\\\\?\\swd#vcamdevapi#abc#{e5323777}";
        assert!(is_virtual_camera(s24, s24_id));
        assert!(!is_virtual_camera(nzxt, "mf:\\\\?\\usb#vid_1e71&pid_2701"));
        assert!(
            device_capture_score_for(nzxt, "mf:usb#vid_1e71")
                > device_capture_score_for(s24, s24_id)
        );
    }

    #[test]
    fn preferred_picks_nzxt_not_last_virtual() {
        let devices = vec![
            info("Logitech StreamCam", "mf:usb#vid_046d", 1),
            info(
                "NZXT Signal HD60 Video",
                "mf:\\\\?\\usb#vid_1e71&pid_2701&mi_00",
                8,
            ),
            info(
                "S24 de arthur (Caméra virtuelle Windows)",
                "mf:\\\\?\\swd#vcamdevapi#deadbeef",
                1,
            ),
        ];
        let idx = pick_preferred_device_index(&devices).expect("pick");
        assert_eq!(devices[idx].display_name, "NZXT Signal HD60 Video");
    }

    #[test]
    fn empty_format_devices_still_listed_label() {
        let d = info("Mystery Card", "mf:usb#vid_1234", 0);
        let label = device_list_label(&d);
        assert!(label.contains("no modes"));
    }
}
