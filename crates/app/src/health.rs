//! Capture health: MF drops, stall detection, optional auto-reinit.

use std::time::{Duration, Instant};

use capture::StreamHealth;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HealthPrefs {
    /// Automatically re-open the MF pipeline after a sustained stall.
    #[serde(default)]
    pub auto_reinit: bool,
    /// Seconds without a frame before declaring a stall (2–15).
    #[serde(default = "default_stall_secs")]
    pub stall_secs: u32,
}

fn default_stall_secs() -> u32 {
    3
}

impl Default for HealthPrefs {
    fn default() -> Self {
        Self {
            auto_reinit: false,
            stall_secs: default_stall_secs(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureHealthKind {
    Ok,
    WaitingForSignal,
    DeviceBusy,
    StreamError,
    Stall,
}

#[derive(Clone, Debug, Default)]
pub struct CaptureHealthUi {
    pub kind: CaptureHealthKind,
    pub dropped: u64,
    pub stall_secs: f32,
    pub last_reinit_at: Option<Instant>,
    pub reinit_count: u32,
}

impl Default for CaptureHealthKind {
    fn default() -> Self {
        Self::Ok
    }
}

pub struct HealthMonitor {
    pub prefs: HealthPrefs,
    pub ui: CaptureHealthUi,
    last_frame_at: Instant,
    stall_since: Option<Instant>,
    /// Cooldown so auto-reinit cannot thrash.
    reinit_cooldown: Duration,
}

impl Default for HealthMonitor {
    fn default() -> Self {
        Self {
            prefs: HealthPrefs::default(),
            ui: CaptureHealthUi::default(),
            last_frame_at: Instant::now(),
            stall_since: None,
            reinit_cooldown: Duration::from_secs(8),
        }
    }
}

impl HealthMonitor {
    pub fn note_frame(&mut self) {
        self.last_frame_at = Instant::now();
        self.stall_since = None;
    }

    /// Update derived health. Returns `true` when the caller should re-init capture.
    pub fn tick(
        &mut self,
        stream: StreamHealth,
        dropped: u64,
        capture_restarting: bool,
        device_busy: bool,
    ) -> bool {
        self.ui.dropped = dropped;
        let stall_limit = Duration::from_secs(self.prefs.stall_secs.clamp(2, 15) as u64);

        let kind = if device_busy || matches!(stream, StreamHealth::DeviceBusy) {
            CaptureHealthKind::DeviceBusy
        } else if matches!(stream, StreamHealth::StreamError) {
            CaptureHealthKind::StreamError
        } else if matches!(stream, StreamHealth::WaitingForSignal) {
            CaptureHealthKind::WaitingForSignal
        } else if !capture_restarting && self.last_frame_at.elapsed() > stall_limit {
            if self.stall_since.is_none() {
                self.stall_since = Some(Instant::now());
            }
            CaptureHealthKind::Stall
        } else {
            CaptureHealthKind::Ok
        };

        self.ui.kind = kind;
        self.ui.stall_secs = self
            .stall_since
            .map(|t| t.elapsed().as_secs_f32())
            .unwrap_or(0.0);

        let mut want_reinit = false;
        if self.prefs.auto_reinit && kind == CaptureHealthKind::Stall {
            let cooled = self
                .ui
                .last_reinit_at
                .map(|t| t.elapsed() >= self.reinit_cooldown)
                .unwrap_or(true);
            if cooled {
                want_reinit = true;
                self.ui.last_reinit_at = Some(Instant::now());
                self.ui.reinit_count = self.ui.reinit_count.saturating_add(1);
                // Reset stall clock so we do not immediately re-trigger.
                self.last_frame_at = Instant::now();
                self.stall_since = None;
            }
        }
        want_reinit
    }
}
