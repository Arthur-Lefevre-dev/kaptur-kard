//! Capture → ringbuf → (delay / drift / resample) → playback engine.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Sample, SampleFormat, Stream, StreamConfig};
use parking_lot::Mutex;
use ringbuf::{
    traits::{Consumer, Observer, Producer, Split},
    HeapRb,
};
use tracing::{info, warn};

use crate::clock::{AudioClock, DriftCorrector};
use crate::device::{
    default_output_id, find_input_device, find_output_device, AudioError, NONE_INPUT_ID,
};
use crate::level::LevelMeter;
use crate::resample::{fold_to_stereo_interleaved, StereoResampler};

const TARGET_BUFFER_MS_DEFAULT: f32 = 30.0;
const TARGET_BUFFER_MS_MIN: f32 = 20.0;
const TARGET_BUFFER_MS_MAX: f32 = 40.0;

#[derive(Clone, Debug)]
pub struct AudioEngineConfig {
    pub input_id: String,
    pub output_id: String,
    pub volume: f32,
    pub muted: bool,
    /// Extra delay applied to audio relative to video (0..=500 ms).
    pub delay_ms: f32,
    /// Ring target latency before delay (20..=40 ms).
    pub buffer_ms: f32,
}

impl Default for AudioEngineConfig {
    fn default() -> Self {
        Self {
            input_id: NONE_INPUT_ID.into(),
            output_id: default_output_id().unwrap_or_else(|| "default".into()),
            volume: 1.0,
            muted: false,
            delay_ms: 0.0,
            buffer_ms: TARGET_BUFFER_MS_DEFAULT,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct AudioStatus {
    pub notice: String,
    pub input_rate: u32,
    pub output_rate: u32,
    pub running: bool,
}

struct SharedState {
    volume: AtomicU32, // milli
    muted: AtomicBool,
    delay_frames: AtomicU32,
    target_buffer_frames: AtomicU32,
}

impl SharedState {
    fn volume_f32(&self) -> f32 {
        self.volume.load(Ordering::Relaxed) as f32 / 1000.0
    }
}

pub struct AudioEngine {
    config: AudioEngineConfig,
    status: AudioStatus,
    meter: Arc<LevelMeter>,
    shared: Arc<SharedState>,
    _input_stream: Option<Stream>,
    _output_stream: Option<Stream>,
    /// Kept alive so producers/consumers stay valid — held inside streams actually.
    _ring_keep: Option<Arc<()>>,
}

impl AudioEngine {
    pub fn new() -> Self {
        Self {
            config: AudioEngineConfig::default(),
            status: AudioStatus::default(),
            meter: LevelMeter::new(),
            shared: Arc::new(SharedState {
                volume: AtomicU32::new(1000),
                muted: AtomicBool::new(false),
                delay_frames: AtomicU32::new(0),
                target_buffer_frames: AtomicU32::new(0),
            }),
            _input_stream: None,
            _output_stream: None,
            _ring_keep: None,
        }
    }

    pub fn meter(&self) -> Arc<LevelMeter> {
        Arc::clone(&self.meter)
    }

    pub fn status(&self) -> &AudioStatus {
        &self.status
    }

    pub fn config(&self) -> &AudioEngineConfig {
        &self.config
    }

    pub fn set_volume(&mut self, volume: f32) {
        let v = volume.clamp(0.0, 1.5);
        self.config.volume = v;
        self.shared
            .volume
            .store((v * 1000.0).round() as u32, Ordering::Relaxed);
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.config.muted = muted;
        self.shared.muted.store(muted, Ordering::Relaxed);
    }

    pub fn set_delay_ms(&mut self, delay_ms: f32) {
        let d = delay_ms.clamp(0.0, 500.0);
        self.config.delay_ms = d;
        let rate = self.status.output_rate.max(1);
        let frames = AudioClock::new(rate).ms_to_frames(d) as u32;
        self.shared.delay_frames.store(frames, Ordering::Relaxed);
    }

    pub fn set_buffer_ms(&mut self, buffer_ms: f32) {
        let b = buffer_ms.clamp(TARGET_BUFFER_MS_MIN, TARGET_BUFFER_MS_MAX);
        self.config.buffer_ms = b;
        let rate = self.status.output_rate.max(1);
        let frames = AudioClock::new(rate).ms_to_frames(b) as u32;
        self.shared
            .target_buffer_frames
            .store(frames, Ordering::Relaxed);
    }

    pub fn stop(&mut self) {
        self._input_stream = None;
        self._output_stream = None;
        self.status.running = false;
    }

    /// Restart streams with the given device ids. On output failure, falls back to system default.
    pub fn start(&mut self, mut config: AudioEngineConfig) -> Result<(), AudioError> {
        self.stop();
        config.buffer_ms = config
            .buffer_ms
            .clamp(TARGET_BUFFER_MS_MIN, TARGET_BUFFER_MS_MAX);
        config.delay_ms = config.delay_ms.clamp(0.0, 500.0);
        config.volume = config.volume.clamp(0.0, 1.5);
        self.config = config.clone();
        self.shared
            .volume
            .store((config.volume * 1000.0).round() as u32, Ordering::Relaxed);
        self.shared.muted.store(config.muted, Ordering::Relaxed);
        self.status.notice.clear();

        if config.input_id == NONE_INPUT_ID {
            self.status.running = false;
            self.status.input_rate = 0;
            info!("audio input: Aucun son");
            // Still open output? Not needed without input — keep silent.
            return Ok(());
        }

        let input_dev = find_input_device(&config.input_id)?;
        let (mut output_dev, mut output_id) =
            match find_output_device(&config.output_id) {
                Ok(d) => (d, config.output_id.clone()),
                Err(e) => {
                    warn!(error = %e, "output device missing; falling back to default");
                    self.status.notice =
                        "Périphérique audio de sortie introuvable, retour au défaut système."
                            .into();
                    let d = find_output_device("default")?;
                    let id = default_output_id().unwrap_or_else(|| "default".into());
                    (d, id)
                }
            };

        // If configured output vanished mid-check, try default once more after open config fails.
        let in_cfg = pick_input_config(&input_dev)?;
        let out_cfg = match pick_output_config(&output_dev) {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "output config failed; trying default");
                self.status.notice =
                    "Périphérique audio de sortie introuvable, retour au défaut système.".into();
                output_dev = find_output_device("default")?;
                output_id = default_output_id().unwrap_or_else(|| "default".into());
                pick_output_config(&output_dev)?
            }
        };

        self.config.output_id = output_id;
        let in_rate = in_cfg.sample_rate.0;
        let out_rate = out_cfg.sample_rate.0;
        let in_channels = in_cfg.channels as usize;
        self.status.input_rate = in_rate;
        self.status.output_rate = out_rate;

        let delay_frames = AudioClock::new(out_rate).ms_to_frames(config.delay_ms);
        let buffer_frames = AudioClock::new(out_rate).ms_to_frames(config.buffer_ms);
        self.shared
            .delay_frames
            .store(delay_frames as u32, Ordering::Relaxed);
        self.shared
            .target_buffer_frames
            .store(buffer_frames as u32, Ordering::Relaxed);

        // Capacity: ~500ms delay + 100ms slack + buffer, in stereo samples.
        let capacity_frames = AudioClock::new(out_rate).ms_to_frames(600.0) + buffer_frames * 2;
        let capacity_samples = capacity_frames * 2;
        let rb = HeapRb::<f32>::new(capacity_samples.max(1024));
        let (prod, cons) = rb.split();
        let prod = Arc::new(Mutex::new(prod));
        let cons = Arc::new(Mutex::new(cons));

        let chunk = 256usize;
        let resampler = StereoResampler::new(in_rate, out_rate, chunk)?;
        let resampler = Arc::new(Mutex::new(resampler));
        let pending_in = Arc::new(Mutex::new(Vec::<f32>::new()));

        let meter_in = Arc::clone(&self.meter);
        let prod_cb = Arc::clone(&prod);
        let resampler_cb = Arc::clone(&resampler);
        let pending_cb = Arc::clone(&pending_in);

        let input_stream = build_input_stream(
            &input_dev,
            &in_cfg,
            in_channels,
            move |data: &[f32]| {
                let stereo = fold_to_stereo_interleaved(data, in_channels);
                meter_in.observe_interleaved_stereo(&stereo);
                let mut pending = pending_cb.lock();
                pending.extend_from_slice(&stereo);
                let mut resampler = resampler_cb.lock();
                let chunk_samples = resampler.chunk_frames() * 2;
                while pending.len() >= chunk_samples || resampler.is_identity() {
                    if resampler.is_identity() {
                        let mut prod = prod_cb.lock();
                        let _ = prod.push_slice(&pending);
                        pending.clear();
                        break;
                    }
                    let chunk: Vec<f32> = pending.drain(..chunk_samples).collect();
                    match resampler.process_interleaved(&chunk) {
                        Ok(out) if !out.is_empty() => {
                            let mut prod = prod_cb.lock();
                            let _ = prod.push_slice(&out);
                        }
                        Ok(_) => {}
                        Err(e) => warn!(error = %e, "resample failed"),
                    }
                }
            },
        )?;

        let shared = Arc::clone(&self.shared);
        let meter_out = Arc::clone(&self.meter);
        let cons_cb = Arc::clone(&cons);
        let drift = DriftCorrector::default();
        let capacity_samples_u = capacity_samples;

        let output_stream = build_output_stream(
            &output_dev,
            &out_cfg,
            move |out: &mut [f32]| {
                out.fill(0.0);
                let mut cons = cons_cb.lock();
                let occupied = cons.occupied_len();
                let fill = occupied as f32 / capacity_samples_u as f32;
                let delay_needed = shared.delay_frames.load(Ordering::Relaxed) as usize * 2;
                if occupied < delay_needed {
                    return; // wait to build delay / prebuffer
                }
                let target = shared.target_buffer_frames.load(Ordering::Relaxed) as usize * 2;
                let _ = target; // informational; delay gate is primary
                let corr = drift.correction_frames(fill);
                if corr > 0 {
                    // Skip frames (drop samples).
                    let skip = (corr as usize) * 2;
                    let mut discard = vec![0.0f32; skip.min(occupied)];
                    let _ = cons.pop_slice(&mut discard);
                }

                let got = cons.pop_slice(out);
                if corr < 0 {
                    // Underrun pad already zero-filled; leave remaining.
                    let _ = corr;
                }
                if got < out.len() {
                    // underrun silence for the rest
                }

                let muted = shared.muted.load(Ordering::Relaxed);
                let vol = if muted { 0.0 } else { shared.volume_f32() };
                if (vol - 1.0).abs() > 0.001 {
                    for s in out.iter_mut() {
                        *s = (*s * vol).clamp(-1.0, 1.0);
                    }
                } else if muted {
                    out.fill(0.0);
                }
                meter_out.observe_interleaved_stereo(out);
            },
        )?;

        input_stream
            .play()
            .map_err(|e| AudioError::Stream(e.to_string()))?;
        output_stream
            .play()
            .map_err(|e| AudioError::Stream(e.to_string()))?;

        self._input_stream = Some(input_stream);
        self._output_stream = Some(output_stream);
        self.status.running = true;
        info!(
            in_rate,
            out_rate,
            delay_ms = config.delay_ms,
            buffer_ms = config.buffer_ms,
            "audio engine started"
        );
        // Keep streams warm briefly so first callback can allocate.
        std::thread::sleep(Duration::from_millis(1));
        Ok(())
    }
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn pick_input_config(dev: &cpal::Device) -> Result<StreamConfig, AudioError> {
    let cfg = dev
        .default_input_config()
        .map_err(|e| AudioError::Config(e.to_string()))?;
    Ok(StreamConfig {
        channels: cfg.channels(),
        sample_rate: cfg.sample_rate(),
        buffer_size: cpal::BufferSize::Default,
    })
}

fn pick_output_config(dev: &cpal::Device) -> Result<StreamConfig, AudioError> {
    let cfg = dev
        .default_output_config()
        .map_err(|e| AudioError::Config(e.to_string()))?;
    Ok(StreamConfig {
        channels: 2,
        sample_rate: cfg.sample_rate(),
        buffer_size: cpal::BufferSize::Default,
    })
}

fn build_input_stream<F>(
    device: &cpal::Device,
    config: &StreamConfig,
    _channels: usize,
    mut on_data: F,
) -> Result<Stream, AudioError>
where
    F: FnMut(&[f32]) + Send + 'static,
{
    let err_fn = |e| warn!(error = %e, "input stream error");
    let supported = device
        .default_input_config()
        .map_err(|e| AudioError::Config(e.to_string()))?;
    match supported.sample_format() {
        SampleFormat::F32 => device
            .build_input_stream(
                config,
                move |data: &[f32], _| on_data(data),
                err_fn,
                None,
            )
            .map_err(|e| AudioError::Stream(e.to_string())),
        SampleFormat::I16 => device
            .build_input_stream(
                config,
                move |data: &[i16], _| {
                    let conv: Vec<f32> = data.iter().map(|s| (*s).to_sample::<f32>()).collect();
                    on_data(&conv);
                },
                err_fn,
                None,
            )
            .map_err(|e| AudioError::Stream(e.to_string())),
        SampleFormat::U16 => device
            .build_input_stream(
                config,
                move |data: &[u16], _| {
                    let conv: Vec<f32> = data.iter().map(|s| (*s).to_sample::<f32>()).collect();
                    on_data(&conv);
                },
                err_fn,
                None,
            )
            .map_err(|e| AudioError::Stream(e.to_string())),
        other => Err(AudioError::Config(format!(
            "unsupported input sample format: {other:?}"
        ))),
    }
}

fn build_output_stream<F>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut on_data: F,
) -> Result<Stream, AudioError>
where
    F: FnMut(&mut [f32]) + Send + 'static,
{
    let err_fn = |e| warn!(error = %e, "output stream error");
    let supported = device
        .default_output_config()
        .map_err(|e| AudioError::Config(e.to_string()))?;
    // We request stereo f32-ish config; match device format.
    match supported.sample_format() {
        SampleFormat::F32 => {
            let mut cfg = config.clone();
            cfg.channels = 2;
            device
                .build_output_stream(
                    &cfg,
                    move |data: &mut [f32], _| on_data(data),
                    err_fn,
                    None,
                )
                .map_err(|e| AudioError::Stream(e.to_string()))
        }
        SampleFormat::I16 => {
            let mut cfg = config.clone();
            cfg.channels = 2;
            device
                .build_output_stream(
                    &cfg,
                    move |data: &mut [i16], _| {
                        let mut tmp = vec![0.0f32; data.len()];
                        on_data(&mut tmp);
                        for (d, s) in data.iter_mut().zip(tmp.iter()) {
                            *d = s.to_sample::<i16>();
                        }
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| AudioError::Stream(e.to_string()))
        }
        SampleFormat::U16 => {
            let mut cfg = config.clone();
            cfg.channels = 2;
            device
                .build_output_stream(
                    &cfg,
                    move |data: &mut [u16], _| {
                        let mut tmp = vec![0.0f32; data.len()];
                        on_data(&mut tmp);
                        for (d, s) in data.iter_mut().zip(tmp.iter()) {
                            *d = s.to_sample::<u16>();
                        }
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| AudioError::Stream(e.to_string()))
        }
        other => Err(AudioError::Config(format!(
            "unsupported output sample format: {other:?}"
        ))),
    }
}

