//! CaptureView audio: cpal capture/playback, rubato resample, ringbuf bus.

pub mod associate;
pub mod clock;
pub mod device;
pub mod engine;
pub mod level;
pub mod replay_tap;
pub mod resample;

pub use associate::suggest_input_for_video;
pub use clock::{drift_correction_frames, AudioClock, DriftCorrector};
pub use device::{
    default_output_id, list_inputs, list_outputs, max_output_channels, AudioDeviceInfo, AudioError,
    NONE_INPUT_ID,
};
pub use engine::{AudioEngine, AudioEngineConfig, AudioStatus, OutputMode, VOLUME_MAX};
pub use level::{ChannelPeaks, LevelMeter};
pub use replay_tap::{AudioClipSnapshot, AudioReplayTap, SharedReplayTap};
pub use resample::{fold_to_stereo_interleaved, pack_to_surround51_interleaved, StereoResampler};
