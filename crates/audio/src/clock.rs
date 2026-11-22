//! PTS helpers and soft clock-drift correction from ring fill level.

/// Basic sample-rate clock tag.
#[derive(Clone, Debug, Default)]
pub struct AudioClock {
    pub sample_rate: u32,
}

impl AudioClock {
    pub fn new(sample_rate: u32) -> Self {
        Self { sample_rate }
    }

    pub fn ms_to_frames(&self, ms: f32) -> usize {
        ((self.sample_rate as f32) * ms / 1000.0).round().max(0.0) as usize
    }
}

/// Soft drift corrector: when the ring is too full/empty, skip or pad a few frames.
#[derive(Clone, Debug)]
pub struct DriftCorrector {
    /// Target occupancy of the ring (0..1).
    pub target_fill: f32,
    /// How far from target before correcting (0..1).
    pub deadband: f32,
    /// Max frames to skip/pad per callback.
    pub max_adjust: usize,
}

impl Default for DriftCorrector {
    fn default() -> Self {
        Self {
            target_fill: 0.5,
            deadband: 0.15,
            max_adjust: 4,
        }
    }
}

impl DriftCorrector {
    pub fn correction_frames(&self, fill_ratio: f32) -> i32 {
        drift_correction_frames(
            fill_ratio,
            self.target_fill,
            self.deadband,
            self.max_adjust,
        )
    }
}

/// Pure helper: positive → skip input frames (buffer too full);
/// negative → insert silence frames (buffer too empty).
pub fn drift_correction_frames(
    fill_ratio: f32,
    target: f32,
    deadband: f32,
    max_adjust: usize,
) -> i32 {
    let fill = fill_ratio.clamp(0.0, 1.0);
    let err = fill - target;
    if err.abs() <= deadband {
        return 0;
    }
    let strength = ((err.abs() - deadband) / (1.0 - deadband).max(0.01)).clamp(0.0, 1.0);
    let mag = ((strength * max_adjust as f32).round() as i32).max(1);
    let mag = mag.min(max_adjust as i32);
    if err > 0.0 {
        mag
    } else {
        -mag
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_correction_in_deadband() {
        assert_eq!(drift_correction_frames(0.5, 0.5, 0.15, 4), 0);
        assert_eq!(drift_correction_frames(0.55, 0.5, 0.15, 4), 0);
    }

    #[test]
    fn skips_when_too_full() {
        let c = drift_correction_frames(0.95, 0.5, 0.15, 4);
        assert!(c > 0);
    }

    #[test]
    fn pads_when_too_empty() {
        let c = drift_correction_frames(0.05, 0.5, 0.15, 4);
        assert!(c < 0);
    }
}
