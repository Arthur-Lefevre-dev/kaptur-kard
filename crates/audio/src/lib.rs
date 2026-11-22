//! CaptureView audio: cpal capture/playback, rubato resample, ringbuf bus.

pub mod associate;
pub mod clock;
pub mod device;
pub mod engine;
pub mod level;
pub mod resample;

pub use associate::suggest_input_for_video;
pub use clock::{drift_correction_frames, AudioClock, DriftCorrector};
pub use device::{
    default_output_id, list_inputs, list_outputs, AudioDeviceInfo, AudioError, NONE_INPUT_ID,
};
pub use engine::{AudioEngine, AudioEngineConfig, AudioStatus};
pub use level::LevelMeter;
pub use resample::{fold_to_stereo_interleaved, StereoResampler};
