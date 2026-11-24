//! Rolling replay buffer + H.264/MJPEG clip save (cheap capture path).

use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use capture::{PixelFormat, PooledBytes, VideoFrame};

use crate::pixel_convert::{
    bgra_to_rgba_thumb, downsample_bgra, downsample_nv12, nv12_to_bgra_scaled, stored_size,
    yuy2_to_bgra,
};
use crate::prefs::app_data_dir;

/// Hard RAM ceiling for buffered frame bytes (~384 MiB — NV12 at up to 1080p).
const MAX_BUFFER_BYTES: usize = 384 * 1024 * 1024;
/// Cap width for stored frames (keep near capture res; avoid heavy downscale that nukes quality).
const MAX_STORE_WIDTH: u32 = 1920;
/// ffmpeg libx264 CRF (lower = higher quality). Was 23 — too soft for preview clips.
const ENCODE_CRF: &str = "18";
/// MJPEG fallback quality (1–100).
const MJPEG_QUALITY: u8 = 95;
/// Default ring sample rate (30 fps). User may choose 30 / 60 / 120 in the Clips tab.
pub const REPLAY_SAMPLE_FPS_DEFAULT: u32 = 30;
pub const REPLAY_SAMPLE_FPS_CHOICES: [u32; 3] = [30, 60, 120];

/// Normalize to an allowed clip sample FPS.
pub fn normalize_sample_fps(fps: u32) -> u32 {
    if REPLAY_SAMPLE_FPS_CHOICES.contains(&fps) {
        fps
    } else if fps == 20 {
        // Legacy pref from v0.3.8 — map to nearest primary choice.
        30
    } else {
        REPLAY_SAMPLE_FPS_DEFAULT
    }
}

fn gap_for_sample_fps(fps: u32) -> Duration {
    let fps = normalize_sample_fps(fps).max(1);
    Duration::from_secs_f64(1.0 / f64::from(fps))
}
/// Toast / list thumbnail max width.
const THUMB_MAX_W: u32 = 160;

/// Hide console windows for helper processes (ffmpeg / where) under `windows_subsystem = "windows"`.
#[cfg(windows)]
fn silence_window(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW)
}

#[cfg(not(windows))]
fn silence_window(cmd: &mut Command) -> &mut Command {
    cmd
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StoredFormat {
    Bgra8,
    Nv12,
}

#[derive(Clone)]
struct StoredFrame {
    at: Instant,
    width: u32,
    height: u32,
    format: StoredFormat,
    /// Packed pixels matching `format` (NV12 or BGRA8).
    data: Arc<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct ClipEntry {
    pub name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub is_dir: bool,
    /// First-frame JPEG next to the clip (`clip-….jpg`), when present.
    pub thumb_path: Option<PathBuf>,
}

/// Result of a background clip encode (path + metadata for UI).
#[derive(Clone, Debug)]
pub struct EncodeOutcome {
    pub path: PathBuf,
    pub thumb_path: Option<PathBuf>,
    /// Frame rate written into the container (buffer sample rate, not capture FPS).
    pub encode_fps: f32,
    pub frame_count: usize,
}

pub struct ReplayBuffer {
    enabled: bool,
    seconds: u32,
    frames: VecDeque<StoredFrame>,
    bytes: usize,
    last_push: Instant,
    /// Chosen ring / encode sample rate (20, 30, or 60).
    sample_fps: u32,
    /// Min gap between stored frames — derived from `sample_fps`.
    min_gap: Duration,
}

impl Default for ReplayBuffer {
    fn default() -> Self {
        let sample_fps = REPLAY_SAMPLE_FPS_DEFAULT;
        Self {
            enabled: false,
            seconds: 8,
            frames: VecDeque::new(),
            bytes: 0,
            last_push: Instant::now() - Duration::from_secs(1),
            sample_fps,
            min_gap: gap_for_sample_fps(sample_fps),
        }
    }
}

impl ReplayBuffer {
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            self.clear();
        }
    }

    pub fn set_seconds(&mut self, seconds: u32) {
        self.seconds = seconds.clamp(3, 30);
        self.trim();
    }

    /// Set ring sample / encode FPS (30, 60, or 120). Default 30.
    pub fn set_sample_fps(&mut self, fps: u32) {
        let fps = normalize_sample_fps(fps);
        if self.sample_fps == fps {
            return;
        }
        self.sample_fps = fps;
        self.min_gap = gap_for_sample_fps(fps);
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn seconds(&self) -> u32 {
        self.seconds
    }

    pub fn sample_fps(&self) -> u32 {
        self.sample_fps
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Target sample rate for the ring. Capture source FPS is independent.
    pub fn sample_fps_target(&self) -> f32 {
        self.sample_fps as f32
    }

    /// Measured spacing of frames currently in the ring (encode uses this).
    pub fn buffer_sample_fps(&self) -> f32 {
        if self.frames.len() < 2 {
            return self.sample_fps_target();
        }
        let first = self.frames.front().unwrap();
        let last = self.frames.back().unwrap();
        let dt = last.at.duration_since(first.at);
        if dt.as_secs_f32() <= 0.001 {
            return self.sample_fps_target();
        }
        ((self.frames.len() - 1) as f32 / dt.as_secs_f32()).clamp(10.0, 120.0)
    }

    pub fn time_range(&self) -> Option<(Instant, Instant)> {
        let first = self.frames.front()?;
        let last = self.frames.back()?;
        Some((first.at, last.at))
    }

    pub fn clear(&mut self) {
        self.frames.clear();
        self.bytes = 0;
    }

    pub fn push_frame(&mut self, frame: &VideoFrame) {
        if !self.enabled {
            return;
        }
        if self.last_push.elapsed() < self.min_gap {
            return;
        }
        let Some((fmt, data, w, h)) = store_frame_cheap(frame, MAX_STORE_WIDTH) else {
            return;
        };
        let bytes = data.len();
        if bytes == 0 || bytes > MAX_BUFFER_BYTES / 2 {
            return;
        }
        self.frames.push_back(StoredFrame {
            at: Instant::now(),
            width: w,
            height: h,
            format: fmt,
            data: Arc::new(data),
        });
        self.bytes += bytes;
        self.last_push = Instant::now();
        self.trim();
    }

    fn trim(&mut self) {
        let keep = Duration::from_secs(self.seconds as u64);
        let now = Instant::now();
        while let Some(front) = self.frames.front() {
            let too_old = now.duration_since(front.at) > keep;
            let too_big = self.bytes > MAX_BUFFER_BYTES;
            if !too_old && !too_big {
                break;
            }
            if let Some(f) = self.frames.pop_front() {
                self.bytes = self.bytes.saturating_sub(f.data.len());
            }
        }
    }

    /// Clone buffered frames for a background encode (Arc pixel data — cheap).
    /// Does not block present/capture; caller must encode off the UI thread.
    pub fn snapshot_for_encode(
        &self,
        audio: Option<audio::AudioClipSnapshot>,
    ) -> Result<ReplaySnapshot, String> {
        if self.frames.is_empty() {
            return Err("replay buffer empty".into());
        }
        Ok(ReplaySnapshot {
            frames: self.frames.iter().cloned().collect(),
            audio,
        })
    }

    /// Encode buffered frames to a video file under `%APPDATA%/CaptureView/clips/`.
    /// Prefers ffmpeg H.264 `.mp4`; falls back to in-process MJPEG `.avi`.
    /// Prefer [`Self::snapshot_for_encode`] + [`ReplaySnapshot::encode`] on a worker.
    pub fn save_clip(&self) -> Result<EncodeOutcome, String> {
        self.snapshot_for_encode(None)?.encode()
    }
}

/// Owned ring snapshot for off-thread encode (keeps UI / present free).
pub struct ReplaySnapshot {
    frames: Vec<StoredFrame>,
    audio: Option<audio::AudioClipSnapshot>,
}

impl ReplaySnapshot {
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn encode_fps_estimate(&self) -> f32 {
        estimate_fps_slice(&self.frames).clamp(10.0, 120.0)
    }

    /// First buffered frame as a small RGBA thumb for the clip toast / list.
    pub fn first_frame_thumb_rgba(&self) -> Option<(u32, u32, Vec<u8>)> {
        let frame = self.frames.first()?;
        let bgra = stored_to_bgra(frame)?;
        bgra_to_rgba_thumb(&bgra, frame.width, frame.height, THUMB_MAX_W)
    }

    /// Encode to `%APPDATA%/CaptureView/clips/` — safe to call from a background thread.
    pub fn encode(self) -> Result<EncodeOutcome, String> {
        encode_frames(&self.frames, self.audio.as_ref())
    }
}

fn encode_frames(
    frames: &[StoredFrame],
    audio: Option<&audio::AudioClipSnapshot>,
) -> Result<EncodeOutcome, String> {
    if frames.is_empty() {
        return Err("replay buffer empty".into());
    }
    let root = clips_dir().ok_or_else(|| "no app data dir".to_string())?;
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let w = frames[0].width;
    let h = frames[0].height;
    let fps = estimate_fps_slice(frames).clamp(10.0, 120.0);
    let frame_count = frames.len();

    // Materialize BGRA once (encode path only — not on the capture hot path).
    let mut bgra_frames: Vec<Vec<u8>> = Vec::with_capacity(frames.len());
    for frame in frames {
        let bgra = stored_to_bgra(frame).ok_or_else(|| "frame convert failed".to_string())?;
        bgra_frames.push(bgra);
    }

    let thumb_path = write_clip_thumb(&root, stamp, w, h, &bgra_frames[0]);

    let mp4 = root.join(format!("clip-{stamp}.mp4"));
    if let Some(ffmpeg) = find_ffmpeg() {
        match encode_mp4_ffmpeg(&ffmpeg, &mp4, w, h, fps, &bgra_frames, audio) {
            Ok(()) => {
                return Ok(EncodeOutcome {
                    path: mp4,
                    thumb_path,
                    encode_fps: fps,
                    frame_count,
                });
            }
            Err(e) => {
                tracing::warn!(error = %e, "ffmpeg encode failed; falling back to MJPEG AVI");
                let _ = fs::remove_file(&mp4);
            }
        }
    } else {
        tracing::info!("ffmpeg not found; encoding MJPEG AVI fallback");
    }

    let avi = root.join(format!("clip-{stamp}.avi"));
    encode_mjpeg_avi(&avi, w, h, fps, &bgra_frames)?;
    Ok(EncodeOutcome {
        path: avi,
        thumb_path,
        encode_fps: fps,
        frame_count,
    })
}

fn write_clip_thumb(
    root: &Path,
    stamp: u64,
    w: u32,
    h: u32,
    bgra: &[u8],
) -> Option<PathBuf> {
    let (tw, th, rgba) = bgra_to_rgba_thumb(bgra, w, h, THUMB_MAX_W)?;
    // JPEG has no alpha — encode RGB. Rgba8 encode used to fail silently, so the
    // companion .jpg was missing and the Clips list showed empty thumb slots.
    let rgb: Vec<u8> = rgba
        .chunks_exact(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let path = root.join(format!("clip-{stamp}.jpg"));
    let mut jpeg = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut jpeg);
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, 85);
    enc.encode(&rgb, tw, th, image::ExtendedColorType::Rgb8)
        .ok()?;
    fs::write(&path, jpeg).ok()?;
    Some(path)
}

pub fn clips_dir() -> Option<PathBuf> {
    let mut p = app_data_dir()?;
    p.push("clips");
    Some(p)
}

/// `%APPDATA%\CaptureView\luts` — imported `.cube` files live next to clips prefs.
pub fn luts_dir() -> Option<PathBuf> {
    let mut p = app_data_dir()?;
    p.push("luts");
    Some(p)
}

/// Copy a `.cube` into the app LUT folder; returns the destination path.
pub fn import_cube_lut(src: &Path) -> Result<PathBuf, String> {
    if !src.is_file() {
        return Err("not a file".into());
    }
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext != "cube" {
        return Err("expected .cube".into());
    }
    let root = luts_dir().ok_or_else(|| "no app data dir".to_string())?;
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let name = src
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| "imported.cube".into());
    let dest = root.join(name);
    fs::copy(src, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// List saved clips (files + legacy PNG-sequence folders), newest first.
pub fn list_clips() -> Vec<ClipEntry> {
    let Some(root) = clips_dir() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for ent in entries.flatten() {
        let path = ent.path();
        let name = ent.file_name().to_string_lossy().into_owned();
        let meta = ent.metadata().ok();
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let is_file = meta.as_ref().map(|m| m.is_file()).unwrap_or(false);
        if is_file {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !matches!(ext.as_str(), "mp4" | "avi" | "mkv" | "mov" | "webm") {
                continue;
            }
        } else if is_dir {
            // Legacy PNG sequence folders (clip-<unix>/frame_*.png).
            if !name.starts_with("clip-") {
                continue;
            }
        } else {
            continue;
        }
        let size_bytes = if is_dir {
            dir_size_approx(&path)
        } else {
            meta.as_ref().map(|m| m.len()).unwrap_or(0)
        };
        let modified = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let thumb_path = if is_file {
            companion_thumb_path(&path).filter(|p| p.is_file())
        } else if is_dir {
            // Legacy PNG sequence: use first frame_*.png if present.
            first_png_in_dir(&path)
        } else {
            None
        };
        out.push((
            modified,
            ClipEntry {
                name,
                path,
                size_bytes,
                is_dir,
                thumb_path,
            },
        ));
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.into_iter().map(|(_, e)| e).collect()
}

fn companion_thumb_path(video: &Path) -> Option<PathBuf> {
    let stem = video.file_stem()?.to_str()?;
    let parent = video.parent()?;
    Some(parent.join(format!("{stem}.jpg")))
}

/// Rebuild companion `clip-….jpg` from the video via ffmpeg (list thumbs for legacy clips).
/// Returns the thumb path when a readable JPEG was written.
pub fn backfill_clip_thumb(video: &Path) -> Option<PathBuf> {
    if !video.is_file() {
        return None;
    }
    let thumb = companion_thumb_path(video)?;
    let ffmpeg = find_ffmpeg()?;
    let _ = fs::remove_file(&thumb);
    let mut cmd = Command::new(&ffmpeg);
    silence_window(&mut cmd);
    let status = cmd
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
        ])
        .arg(video)
        .args(["-frames:v", "1", "-q:v", "4"])
        .arg(&thumb)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    if status.success() && thumb.is_file() && thumb.metadata().ok()?.len() > 32 {
        Some(thumb)
    } else {
        let _ = fs::remove_file(&thumb);
        None
    }
}

fn first_png_in_dir(dir: &Path) -> Option<PathBuf> {
    let mut pngs: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("png"))
                .unwrap_or(false)
        })
        .collect();
    pngs.sort();
    pngs.into_iter().next()
}

pub fn delete_clip(path: &Path) -> Result<(), String> {
    let Some(root) = clips_dir() else {
        return Err("no clips dir".into());
    };
    let canon_root = root.canonicalize().unwrap_or(root.clone());
    let canon = path.canonicalize().map_err(|e| e.to_string())?;
    if !canon.starts_with(&canon_root) {
        return Err("path outside clips dir".into());
    }
    if let Some(thumb) = companion_thumb_path(&canon) {
        let _ = fs::remove_file(thumb);
    }
    if canon.is_dir() {
        fs::remove_dir_all(&canon).map_err(|e| e.to_string())
    } else {
        fs::remove_file(&canon).map_err(|e| e.to_string())
    }
}

pub fn open_clips_folder() {
    if let Some(dir) = clips_dir() {
        let _ = fs::create_dir_all(&dir);
        crate::export::open_folder(&dir);
    }
}

pub fn play_clip(path: &Path) {
    if path.as_os_str().is_empty() || !path.exists() {
        return;
    }
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        silence_window(&mut cmd);
        let _ = cmd
            .args(["/C", "start", "", &path.to_string_lossy()])
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open").arg(path).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("xdg-open").arg(path).spawn();
    }
}

pub fn reveal_clip(path: &Path) {
    if path.as_os_str().is_empty() || !path.exists() {
        return;
    }
    #[cfg(windows)]
    {
        let _ = Command::new("explorer")
            .arg(format!("/select,{}", path.to_string_lossy()))
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("open").args(["-R", path.to_str().unwrap_or("")]).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(parent) = path.parent() {
            crate::export::open_folder(parent);
        }
    }
}

fn dir_size_approx(path: &Path) -> u64 {
    let Ok(rd) = fs::read_dir(path) else {
        return 0;
    };
    rd.flatten()
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

fn store_frame_cheap(
    frame: &VideoFrame,
    max_w: u32,
) -> Option<(StoredFormat, Vec<u8>, u32, u32)> {
    let (mut dw, mut dh) = stored_size(frame.width, frame.height, max_w);
    let src = frame.data.as_slice();
    match frame.pixel_format {
        // Keep NV12 in the ring — skip expensive YUV→BGRA on the capture path.
        PixelFormat::Nv12 => {
            dw &= !1;
            dh &= !1;
            if dw < 2 || dh < 2 {
                return None;
            }
            let data = downsample_nv12(src, frame.width, frame.height, dw, dh)?;
            Some((StoredFormat::Nv12, data, dw, dh))
        }
        PixelFormat::Bgra8 => {
            let data = downsample_bgra(src, frame.width, frame.height, dw, dh)?;
            Some((StoredFormat::Bgra8, data, dw, dh))
        }
        PixelFormat::Yuy2 => {
            // Convert once to BGRA then downsample (YUY2 is uncommon for MF cards).
            let full = yuy2_to_bgra(src, frame.width, frame.height)?;
            let data = if dw == frame.width && dh == frame.height {
                full
            } else {
                downsample_bgra(&full, frame.width, frame.height, dw, dh)?
            };
            Some((StoredFormat::Bgra8, data, dw, dh))
        }
        _ => None,
    }
}

fn stored_to_bgra(frame: &StoredFrame) -> Option<Vec<u8>> {
    match frame.format {
        StoredFormat::Bgra8 => Some((*frame.data).clone()),
        StoredFormat::Nv12 => {
            nv12_to_bgra_scaled(&frame.data, frame.width, frame.height, frame.width, frame.height)
        }
    }
}

fn estimate_fps_slice(frames: &[StoredFrame]) -> f32 {
    if frames.len() < 2 {
        return REPLAY_SAMPLE_FPS_DEFAULT as f32;
    }
    let dt = frames
        .last()
        .unwrap()
        .at
        .duration_since(frames.first().unwrap().at);
    if dt.as_secs_f32() <= 0.001 {
        return REPLAY_SAMPLE_FPS_DEFAULT as f32;
    }
    (frames.len() - 1) as f32 / dt.as_secs_f32()
}

fn find_ffmpeg() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = which_ffmpeg() {
        return Some(p);
    }
    if let Some(exe) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    {
        candidates.push(exe.join("ffmpeg.exe"));
        candidates.push(exe.join("ffmpeg"));
    }
    candidates.push(PathBuf::from(r"C:\ffmpeg\bin\ffmpeg.exe"));
    candidates.push(PathBuf::from("/usr/bin/ffmpeg"));
    candidates.push(PathBuf::from("/usr/local/bin/ffmpeg"));
    candidates.push(PathBuf::from("/opt/homebrew/bin/ffmpeg"));
    for c in candidates {
        if c.is_file() {
            return Some(c);
        }
    }
    None
}

fn which_ffmpeg() -> Result<PathBuf, ()> {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("where");
        silence_window(&mut cmd);
        let out = cmd.arg("ffmpeg").output().map_err(|_| ())?;
        if !out.status.success() {
            return Err(());
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text.lines().next().unwrap_or("").trim();
        if line.is_empty() {
            return Err(());
        }
        let p = PathBuf::from(line);
        if p.is_file() {
            return Ok(p);
        }
        Err(())
    }
    #[cfg(not(windows))]
    {
        let out = Command::new("which")
            .arg("ffmpeg")
            .output()
            .map_err(|_| ())?;
        if !out.status.success() {
            return Err(());
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text.lines().next().unwrap_or("").trim();
        if line.is_empty() {
            return Err(());
        }
        let p = PathBuf::from(line);
        if p.is_file() {
            Ok(p)
        } else {
            Err(())
        }
    }
}

fn write_temp_pcm_f32le(audio: &audio::AudioClipSnapshot) -> Result<PathBuf, String> {
    if audio.samples.is_empty() || audio.sample_rate == 0 {
        return Err("empty audio".into());
    }
    let root = clips_dir().ok_or_else(|| "no app data dir".to_string())?;
    let path = root.join(format!(
        ".clip-audio-{}.f32le",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    let mut bytes = Vec::with_capacity(audio.samples.len() * 4);
    for s in &audio.samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

fn encode_mp4_ffmpeg(
    ffmpeg: &Path,
    out: &Path,
    w: u32,
    h: u32,
    fps: f32,
    frames: &[Vec<u8>],
    audio: Option<&audio::AudioClipSnapshot>,
) -> Result<(), String> {
    let audio_path = audio
        .filter(|a| a.samples.len() >= 64)
        .and_then(|a| write_temp_pcm_f32le(a).ok());
    let audio_rate = audio.map(|a| a.sample_rate).unwrap_or(48_000);

    let mut args: Vec<String> = vec![
        "-y".into(),
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "bgra".into(),
        "-s".into(),
        format!("{w}x{h}"),
        "-r".into(),
        format!("{fps:.3}"),
        "-i".into(),
        "pipe:0".into(),
    ];
    if let Some(ref pcm) = audio_path {
        args.extend([
            "-f".into(),
            "f32le".into(),
            "-ar".into(),
            audio_rate.to_string(),
            "-ac".into(),
            "2".into(),
            "-i".into(),
            pcm.to_string_lossy().into_owned(),
        ]);
    }
    args.extend([
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "faster".into(),
        "-crf".into(),
        ENCODE_CRF.into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
    ]);
    if audio_path.is_some() {
        args.extend([
            "-c:a".into(),
            "aac".into(),
            "-b:a".into(),
            "192k".into(),
            "-shortest".into(),
        ]);
    } else {
        args.push("-an".into());
    }
    args.extend([
        "-movflags".into(),
        "+faststart".into(),
        out.to_str().ok_or("bad out path")?.into(),
    ]);

    let mut cmd = Command::new(ffmpeg);
    silence_window(&mut cmd);
    let mut child = cmd
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn ffmpeg: {e}"))?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| "ffmpeg stdin missing".to_string())?;
        let need = (w as usize).saturating_mul(h as usize).saturating_mul(4);
        for frame in frames {
            if frame.len() < need {
                if let Some(p) = &audio_path {
                    let _ = fs::remove_file(p);
                }
                return Err("bgra frame short".into());
            }
            stdin
                .write_all(&frame[..need])
                .map_err(|e| format!("ffmpeg write: {e}"))?;
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("ffmpeg wait: {e}"))?;
    if let Some(p) = &audio_path {
        let _ = fs::remove_file(p);
    }
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "ffmpeg exit {}: {}",
            output.status,
            err.chars().take(400).collect::<String>()
        ));
    }
    if !out.is_file() {
        return Err("ffmpeg produced no file".into());
    }
    Ok(())
}

/// Minimal MJPEG AVI writer (no external deps) for machines without ffmpeg.
fn encode_mjpeg_avi(
    out: &Path,
    w: u32,
    h: u32,
    fps: f32,
    frames: &[Vec<u8>],
) -> Result<(), String> {
    let mut jpegs: Vec<Vec<u8>> = Vec::with_capacity(frames.len());
    let need = (w as usize).saturating_mul(h as usize).saturating_mul(4);
    for bgra in frames {
        if bgra.len() < need {
            return Err("bgra frame short".into());
        }
        let mut rgba = Vec::with_capacity(need);
        for chunk in bgra[..need].chunks_exact(4) {
            rgba.push(chunk[2]);
            rgba.push(chunk[1]);
            rgba.push(chunk[0]);
            rgba.push(chunk[3]);
        }
        let mut jpeg = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut jpeg);
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, MJPEG_QUALITY);
        enc.encode(&rgba, w, h, image::ExtendedColorType::Rgba8)
            .map_err(|e| e.to_string())?;
        jpegs.push(jpeg);
    }

    let fps_i = fps.round().max(1.0) as u32;
    let mut movi: Vec<u8> = Vec::new();
    let mut idx: Vec<u8> = Vec::new();
    let mut offset = 4u32; // after 'movi'
    for jpeg in &jpegs {
        let chunk_size = jpeg.len() as u32;
        let padded = chunk_size + (chunk_size & 1);
        // idx1 entry
        idx.extend_from_slice(b"00dc");
        idx.extend_from_slice(&0x10u32.to_le_bytes()); // AVIIF_KEYFRAME
        idx.extend_from_slice(&offset.to_le_bytes());
        idx.extend_from_slice(&padded.to_le_bytes());
        // movi chunk
        movi.extend_from_slice(b"00dc");
        movi.extend_from_slice(&chunk_size.to_le_bytes());
        movi.extend_from_slice(jpeg);
        if chunk_size & 1 == 1 {
            movi.push(0);
        }
        offset = offset.saturating_add(8 + padded);
    }

    let frames_n = jpegs.len() as u32;
    let micro_sec = if fps_i > 0 {
        1_000_000u32 / fps_i
    } else {
        50_000
    };

    // Build hdrl (avih + strl)
    let mut hdrl = Vec::new();
    // avih
    let mut avih = Vec::new();
    avih.extend_from_slice(&micro_sec.to_le_bytes());
    avih.extend_from_slice(&0u32.to_le_bytes()); // max bytes/sec
    avih.extend_from_slice(&0u32.to_le_bytes()); // padding
    avih.extend_from_slice(&0x10u32.to_le_bytes()); // flags HASINDEX
    avih.extend_from_slice(&frames_n.to_le_bytes());
    avih.extend_from_slice(&0u32.to_le_bytes()); // initial frames
    avih.extend_from_slice(&1u32.to_le_bytes()); // streams
    avih.extend_from_slice(&0u32.to_le_bytes()); // suggested buffer
    avih.extend_from_slice(&w.to_le_bytes());
    avih.extend_from_slice(&h.to_le_bytes());
    avih.extend_from_slice(&[0u8; 16]);
    push_list(&mut hdrl, b"avih", &avih);

    let mut strl = Vec::new();
    let mut strh = Vec::new();
    strh.extend_from_slice(b"vids");
    strh.extend_from_slice(b"MJPG");
    strh.extend_from_slice(&0u32.to_le_bytes());
    strh.extend_from_slice(&0u16.to_le_bytes());
    strh.extend_from_slice(&0u16.to_le_bytes());
    strh.extend_from_slice(&0u32.to_le_bytes());
    strh.extend_from_slice(&1u32.to_le_bytes()); // scale
    strh.extend_from_slice(&fps_i.to_le_bytes()); // rate
    strh.extend_from_slice(&0u32.to_le_bytes());
    strh.extend_from_slice(&frames_n.to_le_bytes());
    strh.extend_from_slice(&0u32.to_le_bytes());
    strh.extend_from_slice(&0u32.to_le_bytes());
    strh.extend_from_slice(&0u16.to_le_bytes());
    strh.extend_from_slice(&0u16.to_le_bytes());
    strh.extend_from_slice(&w.to_le_bytes()[..2]);
    strh.extend_from_slice(&h.to_le_bytes()[..2]);
    // strh is 56 bytes typically — pad
    while strh.len() < 56 {
        strh.push(0);
    }
    push_chunk(&mut strl, b"strh", &strh);

    let mut strf = Vec::new();
    strf.extend_from_slice(&40u32.to_le_bytes()); // biSize
    strf.extend_from_slice(&(w as i32).to_le_bytes());
    strf.extend_from_slice(&(h as i32).to_le_bytes());
    strf.extend_from_slice(&1u16.to_le_bytes()); // planes
    strf.extend_from_slice(&24u16.to_le_bytes()); // bit count
    strf.extend_from_slice(b"MJPG");
    strf.extend_from_slice(&0u32.to_le_bytes());
    strf.extend_from_slice(&0u32.to_le_bytes());
    strf.extend_from_slice(&0i32.to_le_bytes());
    strf.extend_from_slice(&0i32.to_le_bytes());
    strf.extend_from_slice(&0u32.to_le_bytes());
    strf.extend_from_slice(&0u32.to_le_bytes());
    push_chunk(&mut strl, b"strf", &strf);
    push_list(&mut hdrl, b"strl", &strl);

    let mut riff = Vec::new();
    riff.extend_from_slice(b"RIFF");
    riff.extend_from_slice(&0u32.to_le_bytes()); // placeholder size
    riff.extend_from_slice(b"AVI ");
    push_list(&mut riff, b"hdrl", &hdrl);
    push_list(&mut riff, b"movi", &movi);
    push_chunk(&mut riff, b"idx1", &idx);
    let file_size = (riff.len() - 8) as u32;
    riff[4..8].copy_from_slice(&file_size.to_le_bytes());

    fs::write(out, riff).map_err(|e| e.to_string())?;
    Ok(())
}

fn push_chunk(buf: &mut Vec<u8>, fourcc: &[u8; 4], data: &[u8]) {
    buf.extend_from_slice(fourcc);
    buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
    buf.extend_from_slice(data);
    if data.len() & 1 == 1 {
        buf.push(0);
    }
}

fn push_list(buf: &mut Vec<u8>, list_type: &[u8; 4], data: &[u8]) {
    buf.extend_from_slice(b"LIST");
    buf.extend_from_slice(&((data.len() + 4) as u32).to_le_bytes());
    buf.extend_from_slice(list_type);
    buf.extend_from_slice(data);
}

/// Helper so callers can stash Arc without importing pool details.
#[allow(dead_code)]
pub fn pooled_len(data: &Arc<PooledBytes>) -> usize {
    data.as_slice().len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel_convert::frame_to_bgra_scaled;
    use capture::PixelFormat;

    #[test]
    fn nv12_fills_replay_when_enabled() {
        let pool = capture::BufferPool::new();
        let mut pooled = pool.take(12);
        {
            let s = pooled.as_mut_slice();
            s[..8].fill(16);
            s[8..].copy_from_slice(&[128, 128, 128, 128]);
        }
        let frame = VideoFrame {
            captured_at: Instant::now(),
            width: 4,
            height: 2,
            pixel_format: PixelFormat::Nv12,
            data: pooled.into_arc(),
        };
        let mut buf = ReplayBuffer::default();
        buf.set_enabled(true);
        buf.push_frame(&frame);
        assert_eq!(buf.frame_count(), 1, "NV12 live frames must enter the ring");
        assert!(frame_to_bgra_scaled(&frame, MAX_STORE_WIDTH).is_some());
        let stored = buf.frames.front().unwrap();
        assert_eq!(stored.format, StoredFormat::Nv12);
    }

    #[test]
    fn save_clip_writes_video() {
        let pool = capture::BufferPool::new();
        // 16×8 NV12 for a tiny encode.
        let w = 16u32;
        let h = 8u32;
        let y = (w * h) as usize;
        let mut buf = ReplayBuffer::default();
        buf.set_enabled(true);
        buf.min_gap = Duration::ZERO;
        for i in 0..5 {
            let mut pooled = pool.take(y + y / 2);
            {
                let s = pooled.as_mut_slice();
                s[..y].fill(16 + (i as u8) * 8);
                s[y..].fill(128);
            }
            let frame = VideoFrame {
                captured_at: Instant::now(),
                width: w,
                height: h,
                pixel_format: PixelFormat::Nv12,
                data: pooled.into_arc(),
            };
            buf.push_frame(&frame);
        }
        assert_eq!(buf.frame_count(), 5);
        // save_clip needs APPDATA — skip if unavailable in CI sandbox.
        if clips_dir().is_none() {
            return;
        }
        let outcome = buf.save_clip().expect("encode");
        assert!(outcome.path.exists());
        let ext = outcome
            .path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        assert!(ext == "mp4" || ext == "avi", "got {ext}");
        let _ = delete_clip(&outcome.path);
    }
}
