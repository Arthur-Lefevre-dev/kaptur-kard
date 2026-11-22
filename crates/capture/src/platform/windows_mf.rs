//! Windows Media Foundation backend (stub for step a).

use crate::source::CaptureError;

/// Placeholder until the real MF enumerator / sample pump is implemented.
pub fn enumerate_devices() -> Result<Vec<crate::device::DeviceInfo>, CaptureError> {
    Err(CaptureError::Unsupported(
        "Media Foundation backend not implemented yet (step a uses MockVideoSource)".into(),
    ))
}
