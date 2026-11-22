//! Heuristics to associate capture-card video with USB audio inputs.

use crate::device::{AudioDeviceInfo, NONE_INPUT_ID};

/// Extract USB VID/PID from an MF symbolic link / device id.
pub fn extract_vid_pid_from_video_id(video_id: &str) -> (Option<String>, Option<String>) {
    let lower = video_id.to_ascii_lowercase();
    let vid = find_token(&lower, "vid_");
    let pid = find_token(&lower, "pid_");
    (vid, pid)
}

fn find_token(hay: &str, prefix: &str) -> Option<String> {
    let i = hay.find(prefix)?;
    let start = i + prefix.len();
    let end = (start + 4).min(hay.len());
    let s = &hay[start..end];
    if s.len() == 4 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(s.to_string())
    } else {
        None
    }
}

fn name_tokens(s: &str) -> Vec<String> {
    s.to_ascii_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 3)
        .filter(|t| {
            !matches!(
                *t,
                "usb" | "the" | "and" | "audio" | "video" | "device" | "microphone" | "speaker"
            )
        })
        .map(|t| t.to_string())
        .collect()
}

/// Score an audio input against a video device; higher is better. Negative = poor match.
pub fn association_score(
    video_name: &str,
    video_id: &str,
    audio: &AudioDeviceInfo,
) -> i32 {
    if audio.id == NONE_INPUT_ID || !audio.is_input {
        return -1000;
    }
    let mut score = 0i32;
    let (v_vid, v_pid) = extract_vid_pid_from_video_id(video_id);
    if let (Some(ref vv), Some(ref av)) = (&v_vid, &audio.vid) {
        if vv == av {
            score += 50;
            if let (Some(ref vp), Some(ref ap)) = (&v_pid, &audio.pid) {
                if vp == ap {
                    score += 80;
                }
            }
        }
    }
    let v_tokens = name_tokens(video_name);
    let a_tokens = name_tokens(&audio.display_name);
    for t in &v_tokens {
        if a_tokens.iter().any(|a| a == t) {
            score += 8;
        }
    }
    // Capture-card-ish audio product strings.
    let an = audio.display_name.to_ascii_lowercase();
    for kw in ["elgato", "cam link", "avermedia", "magewell", "capture", "hdmi", "digital"] {
        if an.contains(kw) {
            score += 5;
        }
    }
    score
}

/// Pick best matching input index, or `None` if nothing scores positively (caller uses Aucun son).
pub fn suggest_input_for_video(
    video_name: &str,
    video_id: &str,
    inputs: &[AudioDeviceInfo],
) -> Option<usize> {
    inputs
        .iter()
        .enumerate()
        .filter(|(_, d)| d.id != NONE_INPUT_ID)
        .map(|(i, d)| (i, association_score(video_name, video_id, d)))
        .filter(|(_, s)| *s > 0)
        .max_by_key(|(_, s)| *s)
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_vid_pid_from_mf_link() {
        let id = r"mf:\\?\usb#vid_0fd9&pid_0066#5&123#{guid}";
        let (vid, pid) = extract_vid_pid_from_video_id(id);
        assert_eq!(vid.as_deref(), Some("0fd9"));
        assert_eq!(pid.as_deref(), Some("0066"));
    }

    #[test]
    fn prefers_matching_vid_pid() {
        let inputs = vec![
            AudioDeviceInfo::none_input(),
            AudioDeviceInfo {
                id: "in:0:Other Mic".into(),
                display_name: "Other Mic".into(),
                is_input: true,
                vid: Some("1234".into()),
                pid: Some("5678".into()),
            },
            AudioDeviceInfo {
                id: "in:1:Elgato Digital".into(),
                display_name: "Elgato Cam Link Audio (vid_0fd9&pid_0066)".into(),
                is_input: true,
                vid: Some("0fd9".into()),
                pid: Some("0066".into()),
            },
        ];
        let idx = suggest_input_for_video(
            "Elgato Cam Link 4K",
            r"mf:\\?\usb#vid_0fd9&pid_0066#x",
            &inputs,
        );
        assert_eq!(idx, Some(2));
    }
}
