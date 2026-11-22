//! Platform capture backends.
//!
//! Step (a) keeps these as API stubs. Media Foundation lands later.

#[cfg(windows)]
pub mod windows_mf;

#[cfg(target_os = "linux")]
pub mod linux_v4l2;

#[cfg(target_os = "macos")]
pub mod macos_avf;
