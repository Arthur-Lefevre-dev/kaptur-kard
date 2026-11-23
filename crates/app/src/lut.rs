//! Light calibration: brightness / contrast / gamma + optional 1D .cube LUT.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// CPU-side grade applied in the present shaders.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct GradePrefs {
    /// Added to RGB after contrast (-0.5 … 0.5). 0 = neutral.
    #[serde(default)]
    pub brightness: f32,
    /// Multiplier around mid-grey (0.5 … 2.0). 1 = neutral.
    #[serde(default = "default_contrast")]
    pub contrast: f32,
    /// Power curve (0.4 … 2.5). 1 = neutral.
    #[serde(default = "default_gamma")]
    pub gamma: f32,
    /// Mix of optional 1D LUT (0 = curves only, 1 = full LUT).
    #[serde(default)]
    pub lut_mix: f32,
}

fn default_contrast() -> f32 {
    1.0
}

fn default_gamma() -> f32 {
    1.0
}

impl Default for GradePrefs {
    fn default() -> Self {
        Self {
            brightness: 0.0,
            contrast: 1.0,
            gamma: 1.0,
            lut_mix: 0.0,
        }
    }
}

impl GradePrefs {
    pub fn clamp(self) -> Self {
        Self {
            brightness: self.brightness.clamp(-0.5, 0.5),
            contrast: self.contrast.clamp(0.5, 2.0),
            gamma: self.gamma.clamp(0.4, 2.5),
            lut_mix: self.lut_mix.clamp(0.0, 1.0),
        }
    }

    pub fn is_identity(self) -> bool {
        self.brightness.abs() < 0.001
            && (self.contrast - 1.0).abs() < 0.001
            && (self.gamma - 1.0).abs() < 0.001
            && self.lut_mix < 0.001
    }

    /// Uniform vec4 for shaders: brightness, contrast, gamma, lut_mix.
    pub fn as_gpu(self) -> [f32; 4] {
        let g = self.clamp();
        [g.brightness, g.contrast, g.gamma, g.lut_mix]
    }

    pub fn identity_gpu() -> [f32; 4] {
        [0.0, 1.0, 1.0, 0.0]
    }
}

/// Tiny 1D LUT sampled as RGB→RGB (size typically 16–65).
#[derive(Clone, Debug, Default)]
pub struct CubeLut1d {
    pub size: u32,
    /// Interleaved RGB floats, length = size * 3.
    pub table: Vec<f32>,
}

impl CubeLut1d {
    pub fn identity(size: u32) -> Self {
        let size = size.max(2);
        let mut table = Vec::with_capacity((size * 3) as usize);
        for i in 0..size {
            let t = i as f32 / (size - 1) as f32;
            table.push(t);
            table.push(t);
            table.push(t);
        }
        Self { size, table }
    }

    /// Parse a minimal .CUBE (TITLE / LUT_1D_SIZE / LUT_3D_SIZE diagonal sample).
    pub fn load_path(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut size_1d: Option<u32> = None;
        let mut size_3d: Option<u32> = None;
        let mut values: Vec<f32> = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("TITLE") {
                continue;
            }
            if let Some(rest) = line.strip_prefix("LUT_1D_SIZE") {
                size_1d = rest.trim().parse().ok();
                continue;
            }
            if let Some(rest) = line.strip_prefix("LUT_3D_SIZE") {
                size_3d = rest.trim().parse().ok();
                continue;
            }
            if line
                .chars()
                .next()
                .map(|c| c.is_ascii_alphabetic())
                .unwrap_or(false)
            {
                continue;
            }
            for part in line.split_whitespace() {
                if let Ok(v) = part.parse::<f32>() {
                    values.push(v);
                }
            }
        }
        if let Some(n) = size_1d {
            let need = (n * 3) as usize;
            if values.len() < need {
                return Err(format!(".cube 1D expected {need} floats, got {}", values.len()));
            }
            values.truncate(need);
            return Ok(Self {
                size: n,
                table: values,
            });
        }
        if let Some(n) = size_3d {
            // Sample the diagonal of a 3D LUT into a 1D curve (light path).
            let need = (n * n * n * 3) as usize;
            if values.len() < need {
                return Err(format!(".cube 3D expected {need} floats, got {}", values.len()));
            }
            let mut table = Vec::with_capacity((n * 3) as usize);
            for i in 0..n {
                let idx = ((i * n * n) + (i * n) + i) as usize;
                let base = idx * 3;
                table.push(values[base]);
                table.push(values[base + 1]);
                table.push(values[base + 2]);
            }
            return Ok(Self { size: n, table });
        }
        Err("no LUT_1D_SIZE / LUT_3D_SIZE in .cube".into())
    }

    /// Apply 1D LUT to a linear RGB sample (0..1).
    pub fn apply_rgb(&self, r: f32, g: f32, b: f32) -> (f32, f32, f32) {
        if self.size < 2 || self.table.len() < (self.size * 3) as usize {
            return (r, g, b);
        }
        let sample = |channel: f32, offset: usize| {
            let t = channel.clamp(0.0, 1.0) * (self.size - 1) as f32;
            let i0 = t.floor() as usize;
            let i1 = (i0 + 1).min(self.size as usize - 1);
            let f = t - i0 as f32;
            let a = self.table[i0 * 3 + offset];
            let b = self.table[i1 * 3 + offset];
            a + (b - a) * f
        };
        (sample(r, 0), sample(g, 1), sample(b, 2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_1d_cube() {
        let text = "\
TITLE \"test\"
LUT_1D_SIZE 2
0.0 0.0 0.0
1.0 1.0 1.0
";
        let lut = CubeLut1d::parse(text).unwrap();
        assert_eq!(lut.size, 2);
        let (r, g, b) = lut.apply_rgb(0.5, 0.5, 0.5);
        assert!((r - 0.5).abs() < 0.01);
        assert!((g - 0.5).abs() < 0.01);
        assert!((b - 0.5).abs() < 0.01);
    }

    #[test]
    fn identity_grade_gpu() {
        assert_eq!(GradePrefs::default().as_gpu(), [0.0, 1.0, 1.0, 0.0]);
    }
}
