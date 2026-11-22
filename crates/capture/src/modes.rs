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
        PixelFormat::Bgra8 | PixelFormat::Rgb24 => 4,
        PixelFormat::Yuy2 => 3,
        PixelFormat::Nv12 => 2,
        PixelFormat::Mjpeg => 1,
        PixelFormat::Unknown(_) => 0,
    }
}

fn is_uncompressed(fmt: PixelFormat) -> bool {
    matches!(
        fmt,
        PixelFormat::Bgra8 | PixelFormat::Rgb24 | PixelFormat::Yuy2 | PixelFormat::Nv12
    )
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

fn near_fps(desc: &FormatDesc, target: f64) -> bool {
    desc.frame_rates.iter().any(|r| (r.as_f64() - target).abs() < 0.6)
}

/// Prefer ~1080p60 uncompressed when available; else best fps then res, preferring raw over MJPEG.
pub fn select_auto(caps: &[FormatDesc]) -> Option<FormatDesc> {
    if caps.is_empty() {
        return None;
    }

    // Ideal: 1080p @ ~60 with uncompressed subtype.
    let ideal = caps
        .iter()
        .filter(|c| c.width == 1920 && c.height == 1080 && near_fps(c, 60.0))
        .max_by(|a, b| {
            is_uncompressed(a.pixel_format)
                .cmp(&is_uncompressed(b.pixel_format))
                .then_with(|| chroma_rank(a.pixel_format).cmp(&chroma_rank(b.pixel_format)))
        });
    if let Some(desc) = ideal {
        return Some(with_best_rate(desc.clone()));
    }

    // Next: any 1080p with highest fps / chroma.
    let any_1080 = caps
        .iter()
        .filter(|c| c.width == 1920 && c.height == 1080)
        .max_by(|a, b| {
            max_fps(a)
                .partial_cmp(&max_fps(b))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| chroma_rank(a.pixel_format).cmp(&chroma_rank(b.pixel_format)))
        });
    if let Some(desc) = any_1080 {
        return Some(with_best_rate(desc.clone()));
    }

    select_game(caps)
}

/// Max fps, then resolution; prefer uncompressed.
pub fn select_game(caps: &[FormatDesc]) -> Option<FormatDesc> {
    caps.iter()
        .max_by(|a, b| {
            max_fps(a)
                .partial_cmp(&max_fps(b))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| area(a).cmp(&area(b)))
                .then_with(|| chroma_rank(a.pixel_format).cmp(&chroma_rank(b.pixel_format)))
        })
        .cloned()
        .map(with_best_rate)
}

/// Max resolution, prefer richer chroma / uncompressed.
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

/// Unique resolutions present in caps (width, height), sorted descending by area.
pub fn unique_resolutions(caps: &[FormatDesc]) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = Vec::new();
    for c in caps {
        let key = (c.width, c.height);
        if !out.contains(&key) {
            out.push(key);
        }
    }
    out.sort_by(|a, b| (b.0 as u64 * b.1 as u64).cmp(&(a.0 as u64 * a.1 as u64)));
    out
}

/// Unique frame rates for a resolution, sorted descending.
pub fn rates_for_resolution(caps: &[FormatDesc], width: u32, height: u32) -> Vec<crate::format::Rational> {
    let mut rates = Vec::new();
    for c in caps.iter().filter(|c| c.width == width && c.height == height) {
        for r in &c.frame_rates {
            if !rates.iter().any(|x: &crate::format::Rational| {
                (x.as_f64() - r.as_f64()).abs() < 0.05
            }) {
                rates.push(*r);
            }
        }
    }
    rates.sort_by(|a, b| b.as_f64().partial_cmp(&a.as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    rates
}

/// Pick a concrete FormatDesc matching resolution + rate, preferring uncompressed.
pub fn pick_format_at(
    caps: &[FormatDesc],
    width: u32,
    height: u32,
    rate: crate::format::Rational,
) -> Option<FormatDesc> {
    let mut candidates: Vec<FormatDesc> = caps
        .iter()
        .filter(|c| {
            c.width == width
                && c.height == height
                && c.frame_rates
                    .iter()
                    .any(|r| (r.as_f64() - rate.as_f64()).abs() < 0.05)
        })
        .cloned()
        .collect();
    for c in &mut candidates {
        c.frame_rates = vec![rate];
    }
    candidates.sort_by(|a, b| chroma_rank(b.pixel_format).cmp(&chroma_rank(a.pixel_format)));
    candidates.into_iter().next()
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
                interlaced: false,
            },
            FormatDesc {
                pixel_format: PixelFormat::Yuy2,
                width: 1920,
                height: 1080,
                frame_rates: vec![Rational::new(60, 1)],
                interlaced: false,
            },
            FormatDesc {
                pixel_format: PixelFormat::Mjpeg,
                width: 1920,
                height: 1080,
                frame_rates: vec![Rational::new(60, 1)],
                interlaced: false,
            },
            FormatDesc {
                pixel_format: PixelFormat::Bgra8,
                width: 3840,
                height: 2160,
                frame_rates: vec![Rational::new(30, 1)],
                interlaced: false,
            },
            FormatDesc {
                pixel_format: PixelFormat::Yuy2,
                width: 720,
                height: 480,
                frame_rates: vec![Rational::new(30000, 1001)],
                interlaced: true,
            },
            FormatDesc {
                pixel_format: PixelFormat::Nv12,
                width: 1920,
                height: 1080,
                frame_rates: vec![Rational::new(30000, 1001)],
                interlaced: true,
            },
        ]
    }

    #[test]
    fn interlaced_modes_are_selectable() {
        let caps = caps();
        let i480 = pick_format_at(&caps, 720, 480, Rational::new(30000, 1001)).expect("480i");
        assert!(i480.interlaced);
        let i1080 = pick_format_at(&caps, 1920, 1080, Rational::new(30000, 1001)).expect("1080i");
        assert!(i1080.interlaced);
        // Auto still prefers progressive 1080p60 when available.
        let auto = select_auto(&caps).expect("auto");
        assert!(!auto.interlaced);
        assert_eq!(auto.height, 1080);
        assert!((auto.frame_rates[0].as_f64() - 60.0).abs() < 0.01);
    }

    #[test]
    fn auto_prefers_uncompressed_1080p60() {
        let selected = select_auto(&caps()).expect("caps");
        assert_eq!(selected.width, 1920);
        assert_eq!(selected.height, 1080);
        assert_eq!(selected.pixel_format, PixelFormat::Yuy2);
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
        // Smallest area among fixtures (includes 720x480 interlaced).
        assert_eq!(selected.width, 720);
        assert_eq!(selected.height, 480);
    }

    #[test]
    fn empty_caps_yield_none() {
        assert!(select_format(CaptureMode::Auto, &[]).is_none());
    }
}
