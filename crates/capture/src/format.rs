#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PixelFormat {
    Nv12,
    Yuy2,
    Rgb24,
    Bgra8,
    /// Motion-JPEG from the device; capture path asks MF to decode to NV12.
    Mjpeg,
    /// Extensible: unknown fourcc kept as raw for passthrough diagnostics.
    Unknown(u32),
}

/// Fractional rate: fps = num/den (e.g. 60000/1001).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Rational {
    pub num: u32,
    pub den: u32,
}

impl Rational {
    pub const fn new(num: u32, den: u32) -> Self {
        Self { num, den }
    }

    pub fn as_f64(self) -> f64 {
        self.as_f64_ref()
    }

    pub fn as_f64_ref(&self) -> f64 {
        self.num as f64 / self.den.max(1) as f64
    }

    /// Interval between frames in nanoseconds.
    pub fn frame_period_ns(self) -> u64 {
        let fps = self.as_f64();
        if fps <= 0.0 {
            return 16_666_667;
        }
        (1_000_000_000.0 / fps).round() as u64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CaptureMode {
    Auto,
    /// Prefer lowest latency / higher fps.
    Game,
    /// Prefer resolution / chroma.
    Quality,
    /// Prefer CPU/GPU budget.
    Economy,
}

/// Heuristic for classic SD/HD interlaced modes when the driver omits flags.
/// 480i/576i: SD heights. 1080i: typically reported near 25/29.97/30 (not 50/60p).
pub fn looks_interlaced(width: u32, height: u32, rate: Rational) -> bool {
    let fps = rate.as_f64();
    let near = |t: f64| (fps - t).abs() < 0.6;
    match height {
        480 | 486 | 487 | 512 if width <= 720 => {
            near(29.97) || near(30.0) || near(59.94) || near(60.0)
        }
        576 | 608 if width <= 720 => near(25.0) || near(50.0),
        1080 if width >= 1280 => near(25.0) || near(29.97) || near(30.0),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_480i_576i_1080i() {
        assert!(looks_interlaced(720, 480, Rational::new(30000, 1001)));
        assert!(looks_interlaced(1920, 1080, Rational::new(30000, 1001)));
        assert!(looks_interlaced(720, 576, Rational::new(25, 1)));
        assert!(!looks_interlaced(1920, 1080, Rational::new(60, 1)));
        assert!(!looks_interlaced(1280, 720, Rational::new(60, 1)));
        assert!(!looks_interlaced(1920, 1080, Rational::new(24, 1)));
    }
}
