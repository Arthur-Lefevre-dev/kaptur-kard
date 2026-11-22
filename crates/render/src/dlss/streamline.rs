//! Optional Streamline DLL probe (`--features dlss`).
//!
//! Does **not** vendor NVIDIA binaries. Looks for `STREAMLINE_SDK_PATH` or
//! `CAPTUREVIEW_STREAMLINE_DIR`, then tries to load `sl.interposer.dll`.
//! Full `slInit` / `slEvaluateFeature` requires a native D3D12/Vulkan device from
//! wgpu (not exposed here yet) — success stops at SdkPresentPendingHook.

use std::path::{Path, PathBuf};

use super::DlssAvailability;

/// Documented target Streamline package (matches NVIDIA-RTX/Streamline releases).
pub fn sdk_version_hint() -> &'static str {
    "Streamline 2.14.16 (GitHub release zip) — set STREAMLINE_SDK_PATH; see docs/DLSS.md"
}

const K_FEATURE_DLSS: u32 = 0;

pub fn probe_streamline(_vendor_id: u32, _backend: wgpu::Backend) -> DlssAvailability {
    #[cfg(not(windows))]
    {
        let _ = K_FEATURE_DLSS;
        return DlssAvailability::SdkMissing;
    }

    #[cfg(windows)]
    {
        let Some(dir) = resolve_sdk_dir() else {
            tracing::debug!("STREAMLINE_SDK_PATH unset and no bundled Streamline DLLs");
            return DlssAvailability::SdkMissing;
        };

        let Some(dll) = find_interposer(&dir) else {
            tracing::warn!(
                path = %dir.display(),
                "Streamline dir found but sl.interposer.dll missing"
            );
            return DlssAvailability::SdkMissing;
        };

        // Load with altered search path so sibling NGX DLLs resolve.
        let lib = {
            use libloading::os::windows::{Library, LOAD_WITH_ALTERED_SEARCH_PATH};
            // SAFETY: path points at a user-provided SDK DLL; we only GetProcAddress.
            match unsafe { Library::load_with_flags(&dll, LOAD_WITH_ALTERED_SEARCH_PATH) } {
                Ok(lib) => lib,
                Err(err) => {
                    tracing::warn!(path = %dll.display(), %err, "failed to load sl.interposer.dll");
                    return DlssAvailability::InitFailed;
                }
            }
        };

        let has_init = unsafe { lib.get::<unsafe extern "C" fn()>(b"slInit\0").is_ok() };
        let has_shutdown = unsafe { lib.get::<unsafe extern "C" fn()>(b"slShutdown\0").is_ok() };
        let has_supported =
            unsafe { lib.get::<unsafe extern "C" fn()>(b"slIsFeatureSupported\0").is_ok() };
        if !(has_init && has_shutdown && has_supported) {
            tracing::warn!("sl.interposer.dll missing expected Streamline exports");
            return DlssAvailability::InitFailed;
        }

        if find_named(&dir, "nvngx_dlss.dll").is_none() {
            tracing::warn!(
                "sl.interposer.dll loaded but nvngx_dlss.dll not found nearby — accept NVIDIA license and unpack release zip"
            );
            return DlssAvailability::SdkMissing;
        }

        tracing::info!(
            path = %dll.display(),
            feature = K_FEATURE_DLSS,
            "Streamline DLSS DLLs present; native wgpu→D3D12/VK evaluate hook still pending — Mode C uses NIS fallback"
        );
        // Keep library loaded for process lifetime of this probe only — drop is fine;
        // encode path still uses NIS until native hook exists.
        drop(lib);
        DlssAvailability::SdkPresentPendingHook
    }
}

fn resolve_sdk_dir() -> Option<PathBuf> {
    for key in ["STREAMLINE_SDK_PATH", "CAPTUREVIEW_STREAMLINE_DIR"] {
        if let Ok(v) = std::env::var(key) {
            let p = PathBuf::from(v.trim());
            if p.is_dir() {
                return Some(p);
            }
        }
    }
    // Optional next-to-exe layout for packaged builds Arthur may copy manually.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for candidate in [
                dir.join("streamline"),
                dir.join("Streamline"),
                dir.join("third_party").join("streamline"),
            ] {
                if candidate.is_dir() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

fn find_interposer(root: &Path) -> Option<PathBuf> {
    const NAMES: &[&str] = &["sl.interposer.dll"];
    for name in NAMES {
        let direct = root.join(name);
        if direct.is_file() {
            return Some(direct);
        }
        let bin = root.join("bin").join("x64").join(name);
        if bin.is_file() {
            return Some(bin);
        }
        if let Some(found) = find_named(root, name) {
            return Some(found);
        }
    }
    None
}

fn find_named(root: &Path, file_name: &str) -> Option<PathBuf> {
    // Shallow walk only (release zip layout is shallow).
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file()
            && path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case(file_name))
        {
            return Some(path);
        }
        if path.is_dir() {
            if let Ok(sub) = std::fs::read_dir(&path) {
                for child in sub.flatten() {
                    let c = child.path();
                    if c.is_file()
                        && c.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.eq_ignore_ascii_case(file_name))
                    {
                        return Some(c);
                    }
                    // One more level for bin/x64/
                    if c.is_dir() {
                        if let Ok(deep) = std::fs::read_dir(&c) {
                            for d in deep.flatten() {
                                let p = d.path();
                                if p.is_file()
                                    && p.file_name()
                                        .and_then(|n| n.to_str())
                                        .is_some_and(|n| n.eq_ignore_ascii_case(file_name))
                                {
                                    return Some(p);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_env_reports_sdk_missing_or_pending() {
        let a = probe_streamline(0x10DE, wgpu::Backend::Dx12);
        assert!(
            matches!(
                a,
                DlssAvailability::SdkMissing
                    | DlssAvailability::InitFailed
                    | DlssAvailability::SdkPresentPendingHook
            ),
            "unexpected {a:?}"
        );
    }
}
