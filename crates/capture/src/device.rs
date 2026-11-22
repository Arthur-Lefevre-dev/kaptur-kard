use crate::format::{PixelFormat, Rational};

/// Opaque, stable-enough id for config keys (platform path / symbolic link / MF activator id).
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct DeviceId(pub String);

impl DeviceId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub display_name: String,
    /// Human-readable backend tag, e.g. "mf", "v4l2", "mock".
    pub backend: &'static str,
    /// Capabilities discovered at enum time (never assumed).
    pub formats: Vec<FormatDesc>,
}

#[derive(Clone, Debug)]
pub struct FormatDesc {
    pub pixel_format: PixelFormat,
    pub width: u32,
    pub height: u32,
    pub frame_rates: Vec<Rational>,
    /// True when the mode is interlaced (480i / 576i / 1080i or MF interlace flag).
    pub interlaced: bool,
}
