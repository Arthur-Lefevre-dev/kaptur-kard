//! Pure helpers for frame-time percentiles (no atomics).

/// Return the value at `pct` (0..=100) of a sorted ascending copy of `samples`.
/// Empty input yields 0.
pub fn percentile_u64(samples: &[u64], pct: u8) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let pct = pct.min(100) as usize;
    if pct == 0 {
        return sorted[0];
    }
    if pct >= 100 {
        return *sorted.last().unwrap();
    }
    // Nearest-rank: index = ceil(p/100 * N) - 1
    let n = sorted.len();
    let rank = ((pct * n) + 99) / 100; // ceil(pct/100 * n)
    let idx = rank.saturating_sub(1).min(n - 1);
    sorted[idx]
}

/// Convert a frame time in nanoseconds to frames-per-second.
pub fn fps_from_frame_ns(frame_ns: u64) -> f32 {
    if frame_ns == 0 {
        return 0.0;
    }
    1_000_000_000.0 / frame_ns as f32
}

/// 1% low FPS: average of the slowest 1% of frame times → FPS.
pub fn one_percent_low_fps(frame_times_ns: &[u64]) -> f32 {
    if frame_times_ns.is_empty() {
        return 0.0;
    }
    let mut sorted = frame_times_ns.to_vec();
    sorted.sort_unstable();
    let worst = ((sorted.len() as f32) * 0.01).ceil() as usize;
    let worst = worst.max(1).min(sorted.len());
    let start = sorted.len() - worst;
    let avg = sorted[start..].iter().sum::<u64>() / worst as u64;
    fps_from_frame_ns(avg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_empty_is_zero() {
        assert_eq!(percentile_u64(&[], 50), 0);
    }

    #[test]
    fn percentile_single() {
        assert_eq!(percentile_u64(&[10], 0), 10);
        assert_eq!(percentile_u64(&[10], 50), 10);
        assert_eq!(percentile_u64(&[10], 100), 10);
    }

    #[test]
    fn percentile_even_spread() {
        let samples: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile_u64(&samples, 1), 1);
        assert_eq!(percentile_u64(&samples, 50), 50);
        assert_eq!(percentile_u64(&samples, 99), 99);
        assert_eq!(percentile_u64(&samples, 100), 100);
    }

    #[test]
    fn one_percent_low_from_mixed_times() {
        // Mostly 16.6ms frames, one slow 50ms frame → 1% low near 20 fps.
        let mut times = vec![16_666_667u64; 99];
        times.push(50_000_000);
        let low = one_percent_low_fps(&times);
        assert!(low > 15.0 && low < 25.0, "got {low}");
    }

    #[test]
    fn fps_from_frame_ns_60hz() {
        let fps = fps_from_frame_ns(16_666_667);
        assert!((fps - 60.0).abs() < 0.1);
    }
}
