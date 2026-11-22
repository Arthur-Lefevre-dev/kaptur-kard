//! Audio capture / playback stubs (wired in a later step).

pub mod clock;
pub mod device;
pub mod resample;

pub use clock::AudioClock;
pub use device::{AudioDeviceInfo, AudioError};
