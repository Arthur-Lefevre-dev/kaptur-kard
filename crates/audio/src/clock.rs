/// Placeholder PTS / drift helpers for A/V sync (later step).
#[derive(Clone, Debug, Default)]
pub struct AudioClock {
    pub sample_rate: u32,
}

impl AudioClock {
    pub fn new(sample_rate: u32) -> Self {
        Self { sample_rate }
    }
}
