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
