//! Durable tracing to `%APPDATA%\CaptureView\logs\` (+ stderr when a console exists).
//!
//! Daily rotation via `tracing-appender`. Level: `RUST_LOG` if set, else Infos prefs (default info).
//! Windows release builds hide the console (`windows_subsystem`); file logs remain the support path.
//! Use `--features console` or a debug build to also see stderr live.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::reload;
use tracing_subscriber::EnvFilter;

use crate::prefs::{logs_dir, load_ui_prefs};

/// Kept alive for the process so non-blocking file writes flush.
static FILE_GUARD: OnceLock<WorkerGuard> = OnceLock::new();
static RELOAD: OnceLock<reload::Handle<EnvFilter, tracing_subscriber::Registry>> = OnceLock::new();
static LOG_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

const TAIL_BYTES: u64 = 24 * 1024;
const KEEP_LOG_FILES: usize = 14;

/// Default filter: app level + mute Vulkan loader / OBS-Overwolf layer spam.
fn build_filter(level: &str) -> EnvFilter {
    let directive = format!(
        "{level},wgpu_hal::vulkan::instance=error,wgpu_hal::vulkan::conv=warn,naga=warn,wgpu_core=warn"
    );
    EnvFilter::try_new(&directive).unwrap_or_else(|_| EnvFilter::new(level))
}

/// Install stderr + daily file layers. Safe to call once at process start.
pub fn init() {
    let prefs_level = normalize_level(&load_ui_prefs().log_level);
    let filter = match EnvFilter::try_from_default_env() {
        Ok(f) => f,
        Err(_) => build_filter(prefs_level),
    };

    let (filter_layer, handle) = reload::Layer::new(filter);
    let _ = RELOAD.set(handle);

    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(true)
        .with_ansi(true);

    let mut file_layer = None;
    if let Some(dir) = logs_dir() {
        if let Err(e) = fs::create_dir_all(&dir) {
            eprintln!("CaptureView: could not create log dir {}: {e}", dir.display());
        } else {
            prune_old_logs(&dir, KEEP_LOG_FILES);
            let appender = tracing_appender::rolling::Builder::new()
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .filename_prefix("captureview")
                .filename_suffix("log")
                .build(&dir)
                .unwrap_or_else(|e| {
                    eprintln!("CaptureView: rolling log init failed: {e}");
                    // Fall back to a fixed file in the same dir.
                    tracing_appender::rolling::never(&dir, "captureview.log")
                });
            let path = current_log_path(&dir);
            if let Ok(mut slot) = LOG_PATH.lock() {
                *slot = Some(path);
            }
            let (non_blocking, guard) = tracing_appender::non_blocking(appender);
            let _ = FILE_GUARD.set(guard);
            file_layer = Some(
                fmt::layer()
                    .with_writer(non_blocking)
                    .with_ansi(false)
                    .with_target(true),
            );
        }
    }

    let registry = tracing_subscriber::registry()
        .with(filter_layer)
        .with(stderr_layer);
    if let Some(file) = file_layer {
        registry.with(file).init();
    } else {
        registry.init();
    }

    if let Some(p) = log_file_path() {
        tracing::info!(path = %p.display(), "file logging enabled");
    } else {
        tracing::warn!("file logging unavailable (no app data dir)");
    }
}

/// Update the filter from Infos (ignored when `RUST_LOG` is set at process start —
/// still applies a new filter so the UI control always does something).
pub fn set_level(level: &str) -> bool {
    let Some(handle) = RELOAD.get() else {
        return false;
    };
    let filter = if std::env::var_os("RUST_LOG").is_some() {
        // Prefer live RUST_LOG string if the user set it; else prefs level.
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| build_filter(normalize_level(level)))
    } else {
        build_filter(normalize_level(level))
    };
    match handle.reload(filter) {
        Ok(()) => {
            tracing::info!(level = %normalize_level(level), "log level updated");
            true
        }
        Err(e) => {
            tracing::warn!(error = %e, "log level reload failed");
            false
        }
    }
}

pub fn normalize_level(level: &str) -> &'static str {
    match level.trim().to_ascii_lowercase().as_str() {
        "error" => "error",
        "warn" | "warning" => "warn",
        "debug" => "debug",
        "trace" => "trace",
        _ => "info",
    }
}

pub fn log_file_path() -> Option<PathBuf> {
    LOG_PATH.lock().ok()?.clone().or_else(|| {
        let dir = logs_dir()?;
        Some(current_log_path(&dir))
    })
}

/// Last ~24 KiB of the active log file (for support reports).
pub fn log_tail() -> String {
    let Some(path) = log_file_path() else {
        return String::from("(no log file)");
    };
    match read_tail(&path, TAIL_BYTES) {
        Ok(s) if !s.is_empty() => s,
        Ok(_) => format!("(empty log: {})", path.display()),
        Err(e) => format!("(could not read {}: {e})", path.display()),
    }
}

fn current_log_path(dir: &Path) -> PathBuf {
    // tracing-appender daily naming: `{prefix}.YYYY-MM-DD.{suffix}`
    let date = chrono_like_today();
    dir.join(format!("captureview.{date}.log"))
}

fn chrono_like_today() -> String {
    // Local date YYYY-MM-DD without pulling chrono — enough for path display.
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Approximate UTC date; Windows local offset is fine for support path hint.
    const DAY: u64 = 86_400;
    let days = secs / DAY;
    // Civil from days since 1970-01-01 (Howard Hinnant algorithm).
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn read_tail(path: &Path, max_bytes: u64) -> std::io::Result<String> {
    let mut f = fs::File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(max_bytes);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = String::new();
    f.read_to_string(&mut buf)?;
    if start > 0 {
        // Drop partial first line after a mid-file seek.
        if let Some(i) = buf.find('\n') {
            buf = buf[i + 1..].to_string();
        }
    }
    Ok(buf)
}

fn prune_old_logs(dir: &Path, keep: usize) {
    let mut files: Vec<_> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            e.path()
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("captureview") && n.ends_with(".log"))
                .unwrap_or(false)
        })
        .collect();
    files.sort_by_key(|e| std::cmp::Reverse(e.metadata().and_then(|m| m.modified()).ok()));
    for old in files.into_iter().skip(keep) {
        let _ = fs::remove_file(old.path());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_level_defaults_info() {
        assert_eq!(normalize_level("INFO"), "info");
        assert_eq!(normalize_level("warning"), "warn");
        assert_eq!(normalize_level("nope"), "info");
    }

    #[test]
    fn logs_dir_ends_with_logs() {
        // May be None in weird CI envs without HOME/APPDATA — skip soft.
        if let Some(p) = logs_dir() {
            assert!(p.ends_with("logs"));
            assert!(p
                .components()
                .any(|c| c.as_os_str() == "CaptureView"));
        }
    }

    #[test]
    fn today_format_looks_like_iso() {
        let d = chrono_like_today();
        assert_eq!(d.len(), 10);
        assert_eq!(&d[4..5], "-");
        assert_eq!(&d[7..8], "-");
    }
}
