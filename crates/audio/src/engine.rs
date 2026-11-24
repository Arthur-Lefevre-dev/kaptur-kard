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
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::clock::{AudioClock, DriftCorrector};
use crate::device::{
    default_output_id, find_input_device, find_output_device, max_output_channels, AudioError,
    NONE_INPUT_ID,
};
use crate::level::LevelMeter;
use crate::replay_tap::SharedReplayTap;
use crate::resample::{
    fold_to_stereo_interleaved, pack_to_surround51_interleaved, StereoResampler,
};

const TARGET_BUFFER_MS_DEFAULT: f32 = 30.0;
const TARGET_BUFFER_MS_MIN: f32 = 20.0;
const TARGET_BUFFER_MS_MAX: f32 = 40.0;

/// Playback layout preference (actual device may force a fold).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    #[default]
    Stereo,
    Surround51,
}

impl OutputMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stereo => "stereo",
            Self::Surround51 => "surround51",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "surround51" | "5.1" | "51" | "surround" => Self::Surround51,
            _ => Self::Stereo,
        }
    }
}

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
    /// Preferred playback layout.
    pub output_mode: OutputMode,
    /// When true (default), multi-channel capture folds to stereo with full center.
    /// Forced on when the output device cannot open 5.1.
    pub downmix_51_to_stereo: bool,
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
            output_mode: OutputMode::Stereo,
            downmix_51_to_stereo: true,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct AudioStatus {
    pub notice: String,
    pub input_rate: u32,
    pub output_rate: u32,
    /// Capture channel count actually opened (2 preferred when the device allows it).
    pub input_channels: u16,
    /// Channels opened on the playback device (2 or 6).
    pub output_channels: u16,
    /// Max channels the output device reported supporting.
    pub output_device_max_channels: u16,
    /// True when user asked for 5.1 but the WASAPI endpoint is stereo-only.
    pub surround_unavailable: bool,
    /// Effective layout after capability + downmix policy.
    pub effective_output_mode: OutputMode,
    /// Whether the live path is folding multi-ch → stereo.
    pub downmix_active: bool,
    pub running: bool,
}

/// Live monitor volume ceiling (300%). Soft-clip on boost so quiet cutscene dialogue can be lifted.
pub const VOLUME_MAX: f32 = 3.0;

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
    /// Optional replay PCM tap (clip mux). Pre-volume, post fold/resample.
    replay_tap: Option<SharedReplayTap>,
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
            replay_tap: None,
            _input_stream: None,
            _output_stream: None,
            _ring_keep: None,
        }
    }

    /// Attach a shared replay PCM tap (app owns the Arc; engine clones into the input callback).
    pub fn set_replay_tap(&mut self, tap: Option<SharedReplayTap>) {
        self.replay_tap = tap;
    }

    pub fn replay_tap(&self) -> Option<&SharedReplayTap> {
        self.replay_tap.as_ref()
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
        // Live monitor boost (quiet cutscene dialogue on some HDMI mixes); soft-clip on output.
        let v = volume.clamp(0.0, VOLUME_MAX);
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
        config.volume = config.volume.clamp(0.0, VOLUME_MAX);
        self.config = config.clone();
        self.shared
            .volume
            .store((config.volume * 1000.0).round() as u32, Ordering::Relaxed);
        self.shared.muted.store(config.muted, Ordering::Relaxed);
        self.status.notice.clear();
        self.status.surround_unavailable = false;
        self.status.downmix_active = false;
        self.status.output_channels = 0;
        self.status.output_device_max_channels = 0;
        self.status.effective_output_mode = OutputMode::Stereo;

        if config.input_id == NONE_INPUT_ID {
            self.status.running = false;
            self.status.input_rate = 0;
            self.status.input_channels = 0;
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

        let out_max = max_output_channels(&output_dev);
        self.status.output_device_max_channels = out_max;

        // Resolve effective layout: 5.1 only when requested, not downmixed, and device can do ≥6.
        let want_surround = config.output_mode == OutputMode::Surround51
            && !config.downmix_51_to_stereo;
        let (effective_mode, surround_unavailable) = if want_surround && out_max >= 6 {
            (OutputMode::Surround51, false)
        } else if config.output_mode == OutputMode::Surround51 && out_max < 6 {
            (OutputMode::Stereo, true)
        } else {
            (OutputMode::Stereo, false)
        };
        self.status.surround_unavailable = surround_unavailable;
        self.status.effective_output_mode = effective_mode;
        let out_channels: u16 = match effective_mode {
            OutputMode::Surround51 => 6,
            OutputMode::Stereo => 2,
        };
        // Prefer stereo capture when the live path will fold anyway (HD60 / stereo speakers).
        let prefer_stereo_in = effective_mode == OutputMode::Stereo || config.downmix_51_to_stereo;

        // If configured output vanished mid-check, try default once more after open config fails.
        let (in_cfg, in_format) = pick_input_config(&input_dev, prefer_stereo_in)?;
        let out_cfg = match pick_output_config(&output_dev, out_channels) {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "output config failed; trying default");
                self.status.notice =
                    "Périphérique audio de sortie introuvable, retour au défaut système.".into();
                output_dev = find_output_device("default")?;
                output_id = default_output_id().unwrap_or_else(|| "default".into());
                let out_max = max_output_channels(&output_dev);
                self.status.output_device_max_channels = out_max;
                if out_channels == 6 && out_max < 6 {
                    self.status.surround_unavailable = true;
                    self.status.effective_output_mode = OutputMode::Stereo;
                    pick_output_config(&output_dev, 2)?
                } else {
                    pick_output_config(&output_dev, out_channels)?
                }
            }
        };
        let out_channels = out_cfg.channels;
        let effective_mode = if out_channels >= 6 {
            OutputMode::Surround51
        } else {
            OutputMode::Stereo
        };
        self.status.effective_output_mode = effective_mode;
        self.status.output_channels = out_channels;

        self.config.output_id = output_id;
        let in_rate = in_cfg.sample_rate.0;
        let mut out_cfg = out_cfg;
        let mut out_rate = out_cfg.sample_rate.0;
        let in_channels = in_cfg.channels as usize;
        self.status.input_rate = in_rate;
        self.status.output_rate = out_rate;
        self.status.input_channels = in_cfg.channels;

        let mut use_surround_path =
            effective_mode == OutputMode::Surround51 && out_channels >= 6 && in_rate == out_rate;
        // Surround + rate mismatch: soft fall back to stereo fold (rubato path is stereo).
        if effective_mode == OutputMode::Surround51 && !use_surround_path {
            warn!(
                in_rate,
                out_rate, "5.1 output needs matching sample rates or 6ch device; folding to stereo"
            );
            if self.status.notice.is_empty() {
                self.status.notice =
                    "5.1 indisponible (périphérique ou fréquences) — repli stéréo.".into();
            }
            self.status.surround_unavailable = true;
            self.status.effective_output_mode = OutputMode::Stereo;
            out_cfg = pick_output_config(&output_dev, 2)?;
            out_rate = out_cfg.sample_rate.0;
            self.status.output_rate = out_rate;
            self.status.output_channels = 2;
            use_surround_path = false;
        }

        let path_channels = if use_surround_path { 6 } else { 2 };
        self.status.downmix_active = !use_surround_path && in_channels > 2;
        if !use_surround_path {
            self.status.output_channels = 2;
            self.status.effective_output_mode = OutputMode::Stereo;
        }

        let delay_frames = AudioClock::new(out_rate).ms_to_frames(config.delay_ms);
        let buffer_frames = AudioClock::new(out_rate).ms_to_frames(config.buffer_ms);
        self.shared
            .delay_frames
            .store(delay_frames as u32, Ordering::Relaxed);
        self.shared
            .target_buffer_frames
            .store(buffer_frames as u32, Ordering::Relaxed);

        // Capacity: ~500ms delay + 100ms slack + buffer, in path_channels samples.
        let capacity_frames = AudioClock::new(out_rate).ms_to_frames(600.0) + buffer_frames * 2;
        let capacity_samples = capacity_frames * path_channels;
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
        let tap_cb = self.replay_tap.clone();

        let input_stream = if use_surround_path {
            build_input_stream(
                &input_dev,
                &in_cfg,
                in_format,
                move |data: &[f32]| {
                    meter_in.observe_interleaved_channels(data, in_channels);
                    let surround = pack_to_surround51_interleaved(data, in_channels);
                    // Replay tap stays stereo (ITU fold) for clip mux.
                    if let Some(tap) = tap_cb.as_ref() {
                        let stereo = fold_to_stereo_interleaved(data, in_channels);
                        tap.lock().push_stereo(&stereo);
                    }
                    let mut prod = prod_cb.lock();
                    let _ = prod.push_slice(&surround);
                },
            )?
        } else {
            build_input_stream(
                &input_dev,
                &in_cfg,
                in_format,
                move |data: &[f32]| {
                    meter_in.observe_interleaved_channels(data, in_channels);
                    let stereo = fold_to_stereo_interleaved(data, in_channels);
                    let mut pending = pending_cb.lock();
                    pending.extend_from_slice(&stereo);
                    let mut resampler = resampler_cb.lock();
                    let chunk_samples = resampler.chunk_frames() * 2;
                    while pending.len() >= chunk_samples || resampler.is_identity() {
                        if resampler.is_identity() {
                            if let Some(tap) = tap_cb.as_ref() {
                                tap.lock().push_stereo(&pending);
                            }
                            let mut prod = prod_cb.lock();
                            let _ = prod.push_slice(&pending);
                            pending.clear();
                            break;
                        }
                        let chunk: Vec<f32> = pending.drain(..chunk_samples).collect();
                        match resampler.process_interleaved(&chunk) {
                            Ok(out) if !out.is_empty() => {
                                if let Some(tap) = tap_cb.as_ref() {
                                    tap.lock().push_stereo(&out);
                                }
                                let mut prod = prod_cb.lock();
                                let _ = prod.push_slice(&out);
                            }
                            Ok(_) => {}
                            Err(e) => warn!(error = %e, "resample failed"),
                        }
                    }
                },
            )?
        };

        let shared = Arc::clone(&self.shared);
        let meter_out = Arc::clone(&self.meter);
        let cons_cb = Arc::clone(&cons);
        let drift = DriftCorrector::default();
        let capacity_samples_u = capacity_samples;
        let play_channels = if use_surround_path { 6u16 } else { 2u16 };

        let output_stream = build_output_stream(
            &output_dev,
            &out_cfg,
            play_channels,
            move |out: &mut [f32]| {
                out.fill(0.0);
                let mut cons = cons_cb.lock();
                let occupied = cons.occupied_len();
                let fill = occupied as f32 / capacity_samples_u as f32;
                let ch = play_channels as usize;
                let delay_needed = shared.delay_frames.load(Ordering::Relaxed) as usize * ch;
                if occupied < delay_needed {
                    return; // wait to build delay / prebuffer
                }
                let target = shared.target_buffer_frames.load(Ordering::Relaxed) as usize * ch;
                let _ = target; // informational; delay gate is primary
                let corr = drift.correction_frames(fill);
                if corr > 0 {
                    // Skip frames (drop samples).
                    let skip = (corr as usize) * ch;
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
                if muted {
                    out.fill(0.0);
                } else if (vol - 1.0).abs() > 0.001 {
                    // Soft clip when boosting (>1) so quiet dialogue can be lifted without hard square.
                    // Milder than raw tanh(x*vol) at 200–300% — keeps speech intelligible.
                    for s in out.iter_mut() {
                        *s = apply_monitor_gain(*s, vol);
                    }
                }
                if ch == 2 {
                    meter_out.observe_interleaved_stereo(out);
                } else {
                    meter_out.observe_interleaved_channels(out, ch);
                }
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
            in_channels = in_cfg.channels,
            out_channels = play_channels,
            effective = ?self.status.effective_output_mode,
            downmix = self.status.downmix_active,
            surround_unavailable = self.status.surround_unavailable,
            delay_ms = config.delay_ms,
            buffer_ms = config.buffer_ms,
            volume = config.volume,
            "audio engine started (live monitor path)"
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

/// Live monitor gain with soft ceiling. At unity, caller skips this path.
fn apply_monitor_gain(sample: f32, vol: f32) -> f32 {
    let v = sample * vol;
    if vol <= 1.0 {
        return v.clamp(-1.0, 1.0);
    }
    // Soft-clip: linear-ish for quiet speech, gentle knee for loud SFX after boost.
    // Scale tanh so |x|<<1 stays close to x*vol (intelligible dialogue lift).
    let drive = 1.15;
    (v * drive).tanh() / drive.tanh()
}

fn pick_input_config(
    dev: &cpal::Device,
    prefer_stereo: bool,
) -> Result<(StreamConfig, SampleFormat), AudioError> {
    let default = dev
        .default_input_config()
        .map_err(|e| AudioError::Config(e.to_string()))?;

    // Prefer stereo when the live path will fold — some capture cards expose 6/8ch WASAPI
    // endpoints even for stereo HDMI, which used to apply headroom ducking on silent surrounds.
    let chosen = if !prefer_stereo || default.channels() == 2 {
        default
    } else {
        let stereo = dev.supported_input_configs().ok().and_then(|configs| {
            let def_rate = default.sample_rate();
            let mut at_rate = None;
            let mut any = None;
            for range in configs {
                if range.channels() != 2 {
                    continue;
                }
                if def_rate >= range.min_sample_rate() && def_rate <= range.max_sample_rate() {
                    at_rate = Some(range.with_sample_rate(def_rate));
                    break;
                }
                if any.is_none() {
                    any = Some(range.with_sample_rate(range.max_sample_rate()));
                }
            }
            at_rate.or(any)
        });
        match stereo {
            Some(c) => {
                info!(
                    from_ch = default.channels(),
                    to_ch = c.channels(),
                    rate = c.sample_rate().0,
                    "audio input: preferring stereo over multi-channel default (live monitor)"
                );
                c
            }
            None => {
                info!(
                    channels = default.channels(),
                    "audio input: multi-channel only; folding to stereo with center preserved"
                );
                default
            }
        }
    };

    Ok((
        StreamConfig {
            channels: chosen.channels(),
            sample_rate: chosen.sample_rate(),
            buffer_size: cpal::BufferSize::Default,
        },
        chosen.sample_format(),
    ))
}

fn pick_output_config(dev: &cpal::Device, want_channels: u16) -> Result<StreamConfig, AudioError> {
    let default = dev
        .default_output_config()
        .map_err(|e| AudioError::Config(e.to_string()))?;
    let def_rate = default.sample_rate();

    if want_channels <= 2 {
        return Ok(StreamConfig {
            channels: 2,
            sample_rate: def_rate,
            buffer_size: cpal::BufferSize::Default,
        });
    }

    // Prefer a supported 6ch (or ≥6) config at the default rate.
    let surround = dev.supported_output_configs().ok().and_then(|configs| {
        let mut at_rate = None;
        let mut any = None;
        for range in configs {
            if range.channels() < 6 {
                continue;
            }
            if def_rate >= range.min_sample_rate() && def_rate <= range.max_sample_rate() {
                at_rate = Some(range.with_sample_rate(def_rate));
                break;
            }
            if any.is_none() {
                any = Some(range.with_sample_rate(range.max_sample_rate()));
            }
        }
        at_rate.or(any)
    });

    if let Some(c) = surround {
        return Ok(StreamConfig {
            channels: 6.min(c.channels()),
            sample_rate: c.sample_rate(),
            buffer_size: cpal::BufferSize::Default,
        });
    }

    // Fall back to stereo when 5.1 is unavailable.
    Ok(StreamConfig {
        channels: 2,
        sample_rate: def_rate,
        buffer_size: cpal::BufferSize::Default,
    })
}

fn build_input_stream<F>(
    device: &cpal::Device,
    config: &StreamConfig,
    sample_format: SampleFormat,
    mut on_data: F,
) -> Result<Stream, AudioError>
where
    F: FnMut(&[f32]) + Send + 'static,
{
    let err_fn = |e| warn!(error = %e, "input stream error");
    // Use the format from the chosen config (may differ from device default when we prefer stereo).
    match sample_format {
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
    channels: u16,
    mut on_data: F,
) -> Result<Stream, AudioError>
where
    F: FnMut(&mut [f32]) + Send + 'static,
{
    let err_fn = |e| warn!(error = %e, "output stream error");
    let supported = device
        .default_output_config()
        .map_err(|e| AudioError::Config(e.to_string()))?;
    // Match device sample format; channel count comes from the resolved layout.
    match supported.sample_format() {
        SampleFormat::F32 => {
            let mut cfg = config.clone();
            cfg.channels = channels;
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
            cfg.channels = channels;
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
            cfg.channels = channels;
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
