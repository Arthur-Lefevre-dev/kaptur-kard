//! Platform capture backends.

#[cfg(windows)]
pub mod windows_mf;

#[cfg(all(windows, feature = "directshow"))]
pub mod windows_dshow;

#[cfg(target_os = "linux")]
pub mod linux_v4l2;

#[cfg(target_os = "macos")]
pub mod macos_avf;

/// Cross-platform device enumeration entry point.
///
/// Windows: Media Foundation (primary). Optional DirectShow devices are merged
/// when the `directshow` feature is enabled (currently empty until DS lands).
/// Linux: V4L2. macOS: AVFoundation scaffolding (empty until implemented).
pub fn list_devices() -> Result<Vec<crate::device::DeviceInfo>, crate::source::CaptureError> {
    #[cfg(windows)]
    {
        #[cfg(feature = "directshow")]
        {
            let mut devices = windows_mf::enumerate_devices()?;
            match windows_dshow::enumerate_devices() {
                Ok(mut ds) => devices.append(&mut ds),
                Err(e) => tracing::warn!(error = %e, "DirectShow enumeration failed"),
            }
            return Ok(devices);
        }
        #[cfg(not(feature = "directshow"))]
        {
            return windows_mf::enumerate_devices();
        }
    }
    #[cfg(target_os = "linux")]
    {
        return linux_v4l2::enumerate_devices();
    }
    #[cfg(target_os = "macos")]
    {
        return macos_avf::enumerate_devices();
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        Err(crate::source::CaptureError::Unsupported(
            "video capture not implemented on this OS".into(),
        ))
    }
}
