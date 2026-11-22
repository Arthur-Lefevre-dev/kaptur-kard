use thiserror::Error;

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("audio backend not implemented yet")]
    NotImplemented,
}

#[derive(Clone, Debug)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub display_name: String,
    pub is_input: bool,
}

/// Stub enumeration — real `cpal` wiring comes in step (c).
pub fn list_devices() -> Result<Vec<AudioDeviceInfo>, AudioError> {
    Err(AudioError::NotImplemented)
}
