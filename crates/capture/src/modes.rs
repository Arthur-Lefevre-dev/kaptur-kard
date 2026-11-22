//! Pure format-selection helpers (no I/O). Heuristics use discovered caps only.

use crate::device::FormatDesc;
use crate::format::{CaptureMode, PixelFormat};

/// Pick best [`FormatDesc`] for the chosen [`CaptureMode`] among discovered caps.
pub fn select_format(mode: CaptureMode, caps: &[FormatDesc]) -> Option<FormatDesc> {
    match mode {
        CaptureMode::Auto => select_auto(caps),
        CaptureMode::Game => select_game(caps),
        CaptureMode::Quality => select_quality(caps),
        CaptureMode::Economy => select_economy(caps),
    }
}

fn max_fps(desc: &FormatDesc) -> f64 {
    desc.frame_rates
        .iter()
        .map(|r| r.as_f64())
        .fold(0.0_f64, f64::max)
}

fn area(desc: &FormatDesc) -> u64 {
    u64::from(desc.width) * u64::from(desc.height)
}

fn chroma_rank(fmt: PixelFormat) -> u8 {
    match fmt {
        PixelFormat::Bgra8 | PixelFormat::Rgb24 => 3,
        PixelFormat::Yuy2 => 2,
        PixelFormat::Nv12 => 1,
        PixelFormat::Unknown(_) => 0,
    }
}

fn with_best_rate(mut desc: FormatDesc) -> FormatDesc {
    if let Some(best) = desc
        .frame_rates
        .iter()
        .copied()
        .max_by(|a, b| a.as_f64().partial_cmp(&b.as_f64()).unwrap_or(std::cmp::Ordering::Equal))
    {
        desc.frame_rates = vec![best];
    }
    desc
}

/// Prefer ~1080p60 when available, else highest fps then resolution.
pub fn select_auto(caps: &[FormatDesc]) -> Option<FormatDesc> {
    if caps.is_empty() {
        return None;
    }

    let target = caps.iter().find(|c| {
        c.width == 1920
            && c.height == 1080
            && c.frame_rates.iter().any(|r| (r.as_f64() - 60.0).abs() < 0.5)
    });
    if let Some(desc) = target {
        return Some(with_best_rate(desc.clone()));
    }

    select_game(caps)
}

/// Max fps, then resolution.
pub fn select_game(caps: &[FormatDesc]) -> Option<FormatDesc> {
    caps.iter()
        .max_by(|a, b| {
            max_fps(a)
                .partial_cmp(&max_fps(b))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| area(a).cmp(&area(b)))
        })
        .cloned()
        .map(with_best_rate)
}

/// Max resolution, prefer richer chroma.
pub fn select_quality(caps: &[FormatDesc]) -> Option<FormatDesc> {
    caps.iter()
        .max_by(|a, b| {
            area(a)
                .cmp(&area(b))
                .then_with(|| chroma_rank(a.pixel_format).cmp(&chroma_rank(b.pixel_format)))
                .then_with(|| {
                    max_fps(a)
                        .partial_cmp(&max_fps(b))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        })
        .cloned()
        .map(with_best_rate)
}

/// Lower resolution/fps budget: prefer smallest area, then lowest fps.
pub fn select_economy(caps: &[FormatDesc]) -> Option<FormatDesc> {
    caps.iter()
        .min_by(|a, b| {
            area(a)
                .cmp(&area(b))
                .then_with(|| {
                    max_fps(a)
                        .partial_cmp(&max_fps(b))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        })
        .cloned()
        .map(|mut desc| {
            if let Some(slowest) = desc
                .frame_rates
                .iter()
                .copied()
                .min_by(|a, b| a.as_f64().partial_cmp(&b.as_f64()).unwrap_or(std::cmp::Ordering::Equal))
            {
                desc.frame_rates = vec![slowest];
            }
            desc
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{PixelFormat, Rational};

    fn caps() -> Vec<FormatDesc> {
        vec![
            FormatDesc {
                pixel_format: PixelFormat::Nv12,
                width: 1280,
                height: 720,
                frame_rates: vec![Rational::new(30, 1), Rational::new(60, 1)],
            },
            FormatDesc {
                pixel_format: PixelFormat::Yuy2,
                width: 1920,
                height: 1080,
                frame_rates: vec![Rational::new(60, 1)],
            },
            FormatDesc {
                pixel_format: PixelFormat::Bgra8,
                width: 3840,
                height: 2160,
                frame_rates: vec![Rational::new(30, 1)],
            },
        ]
    }

    #[test]
    fn auto_prefers_1080p60() {
        let selected = select_auto(&caps()).expect("caps");
        assert_eq!(selected.width, 1920);
        assert_eq!(selected.height, 1080);
        assert!((selected.frame_rates[0].as_f64() - 60.0).abs() < 0.01);
    }

    #[test]
    fn game_prefers_high_fps() {
        let selected = select_game(&caps()).expect("caps");
        assert!((max_fps(&selected) - 60.0).abs() < 0.01);
    }

    #[test]
    fn quality_prefers_resolution() {
        let selected = select_quality(&caps()).expect("caps");
        assert_eq!(selected.width, 3840);
    }

    #[test]
    fn economy_prefers_small() {
        let selected = select_economy(&caps()).expect("caps");
        assert_eq!(selected.width, 1280);
    }

    #[test]
    fn empty_caps_yield_none() {
        assert!(select_format(CaptureMode::Auto, &[]).is_none());
    }
}
