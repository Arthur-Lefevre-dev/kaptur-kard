//! Channel fold + rubato stereo resampler.

use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

use crate::device::AudioError;

/// Downmix arbitrary channel counts to interleaved stereo f32.
pub fn fold_to_stereo_interleaved(input: &[f32], channels: usize) -> Vec<f32> {
    if channels == 0 || input.is_empty() {
        return Vec::new();
    }
    let frames = input.len() / channels;
    let mut out = Vec::with_capacity(frames * 2);
    match channels {
        1 => {
            for f in 0..frames {
                let s = input[f];
                out.push(s);
                out.push(s);
            }
        }
        2 => out.extend_from_slice(&input[..frames * 2]),
        n => {
            for f in 0..frames {
                let base = f * n;
                // ITU-ish-ish simple downmix: FL, FR, optional FC/LFE/BL/BR…
                let fl = input[base];
                let fr = if n > 1 { input[base + 1] } else { fl };
                let fc = if n > 2 { input[base + 2] * 0.707 } else { 0.0 };
                let bl = if n > 4 { input[base + 4] * 0.5 } else { 0.0 };
                let br = if n > 5 { input[base + 5] * 0.5 } else { 0.0 };
                out.push((fl + fc + bl).clamp(-1.0, 1.0));
                out.push((fr + fc + br).clamp(-1.0, 1.0));
            }
        }
    }
    out
}

/// Stereo resampler (input rate → output rate). Identity when rates match.
pub struct StereoResampler {
    inner: Option<SincFixedIn<f32>>,
    chunk_frames: usize,
    ratio: f64,
}

impl StereoResampler {
    pub fn new(input_rate: u32, output_rate: u32, chunk_frames: usize) -> Result<Self, AudioError> {
        if input_rate == 0 || output_rate == 0 {
            return Err(AudioError::Config("zero sample rate".into()));
        }
        let ratio = output_rate as f64 / input_rate as f64;
        if (ratio - 1.0).abs() < 0.000_5 {
            return Ok(Self {
                inner: None,
                chunk_frames,
                ratio: 1.0,
            });
        }
        let params = SincInterpolationParameters {
            sinc_len: 64,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 128,
            window: WindowFunction::BlackmanHarris2,
        };
        let inner = SincFixedIn::<f32>::new(ratio, 2.0, params, chunk_frames, 2)
            .map_err(|e| AudioError::Config(format!("rubato: {e}")))?;
        Ok(Self {
            inner: Some(inner),
            chunk_frames,
            ratio,
        })
    }

    pub fn ratio(&self) -> f64 {
        self.ratio
    }

    pub fn chunk_frames(&self) -> usize {
        self.chunk_frames
    }

    pub fn is_identity(&self) -> bool {
        self.inner.is_none()
    }

    /// Process interleaved stereo; returns interleaved stereo at output rate.
    pub fn process_interleaved(&mut self, input: &[f32]) -> Result<Vec<f32>, AudioError> {
        let Some(resampler) = self.inner.as_mut() else {
            return Ok(input.to_vec());
        };
        if input.len() < self.chunk_frames * 2 {
            // Not enough for a full chunk — return empty (caller should buffer).
            return Ok(Vec::new());
        }
        let frames = (input.len() / 2).min(self.chunk_frames);
        let mut left = Vec::with_capacity(frames);
        let mut right = Vec::with_capacity(frames);
        for i in 0..frames {
            left.push(input[i * 2]);
            right.push(input[i * 2 + 1]);
        }
        let waves = resampler
            .process(&[&left, &right], None)
            .map_err(|e| AudioError::Config(format!("resample: {e}")))?;
        let out_frames = waves[0].len();
        let mut out = Vec::with_capacity(out_frames * 2);
        for i in 0..out_frames {
            out.push(waves[0][i]);
            out.push(waves[1][i]);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_folds_to_stereo() {
        let mono = vec![0.5f32, -0.25];
        let st = fold_to_stereo_interleaved(&mono, 1);
        assert_eq!(st, vec![0.5, 0.5, -0.25, -0.25]);
    }

    #[test]
    fn identity_resampler_passthrough() {
        let mut r = StereoResampler::new(48_000, 48_000, 64).unwrap();
        assert!(r.is_identity());
        let v = vec![0.1f32, -0.1, 0.2, -0.2];
        assert_eq!(r.process_interleaved(&v).unwrap(), v);
    }

    #[test]
    fn resample_441_to_48_produces_output() {
        let chunk = 512;
        let mut r = StereoResampler::new(44_100, 48_000, chunk).unwrap();
        assert!(!r.is_identity());
        let mut input = vec![0.0f32; chunk * 2];
        for i in 0..chunk {
            let t = i as f32 / 44100.0;
            let s = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.2;
            input[i * 2] = s;
            input[i * 2 + 1] = s;
        }
        let out = r.process_interleaved(&input).unwrap();
        assert!(out.len() > chunk); // 48k should yield more frames than 44.1k chunk
        assert_eq!(out.len() % 2, 0);
    }
}
