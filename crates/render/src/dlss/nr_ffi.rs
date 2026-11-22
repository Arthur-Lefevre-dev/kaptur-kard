//! Dynamic loader for `dlss5nr_bridge.dll` (NGX feature 18 CPU staging).

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use libloading::Library;

use super::probe::nvidia_vendor;
use super::DlssAvailability;

type FnInit = unsafe extern "C" fn(i32, *const u16, *mut i8, i32) -> i32;
type FnProcess = unsafe extern "C" fn(
    *const f32,
    *mut f32,
    i32,
    i32,
    i32,
    i32,
    f32,
    f32,
    f32,
    f32,
    i32,
    i32,
    i32,
    *mut i8,
    i32,
) -> i32;
type FnShutdown = unsafe extern "C" fn();
type FnGpuName = unsafe extern "C" fn() -> *const i8;

pub struct NrRuntime {
    _lib: Library,
    process: FnProcess,
    shutdown: FnShutdown,
    gpu_name_cache: String,
    /// Default / Natural / Cinematic styles (0/1/2) — Natural default.
    style: i32,
    preset: i32,
    intensity: f32,
    temporal: bool,
}

impl NrRuntime {
    pub fn try_open(
        vendor_id: u32,
        name: &str,
        backend: wgpu::Backend,
    ) -> (DlssAvailability, Option<Self>) {
        if !nvidia_vendor(vendor_id, name) {
            return (DlssAvailability::NotNvidia, None);
        }
        match backend {
            wgpu::Backend::Dx12 | wgpu::Backend::Vulkan => {}
            _ => return (DlssAvailability::WrongBackend, None),
        }

        let Some(runtime_dir) = resolve_runtime_dir() else {
            tracing::info!("DLSS5 NR runtime dir not found (CAPTUREVIEW_DLSSNR_DIR / AppData)");
            return (DlssAvailability::SdkMissing, None);
        };

        let nr_dll = runtime_dir.join("nvngx_dlssnr.dll");
        if !nr_dll.is_file() {
            tracing::warn!(
                path = %nr_dll.display(),
                "nvngx_dlssnr.dll missing — obtain legally and place in runtime dir (docs/DLSS.md)"
            );
            return (DlssAvailability::SdkMissing, None);
        }

        let shim = runtime_dir.join("caller").join("nvngx.dll_comfy.dll");
        if !shim.is_file() {
            tracing::warn!(
                path = %shim.display(),
                "caller shim missing — run native/dlss5-nr/build_native.ps1"
            );
            return (DlssAvailability::SdkMissing, None);
        }

        let Some(bridge_path) = resolve_bridge_dll(&runtime_dir) else {
            tracing::warn!("dlss5nr_bridge.dll not found next to runtime or in native/dlss5-nr/bin");
            return (DlssAvailability::SdkMissing, None);
        };

        // SAFETY: bridge is our MIT-built DLL with known C ABI.
        let lib = match unsafe { Library::new(&bridge_path) } {
            Ok(l) => l,
            Err(err) => {
                tracing::warn!(%err, path = %bridge_path.display(), "failed to load dlss5nr_bridge.dll");
                return (DlssAvailability::InitFailed, None);
            }
        };

        let (init_fn, process, shutdown, gpu_name_fn) = unsafe {
            let init: libloading::Symbol<FnInit> = match lib.get(b"dlss5nr_init\0") {
                Ok(s) => s,
                Err(_) => return (DlssAvailability::InitFailed, None),
            };
            let process: libloading::Symbol<FnProcess> = match lib.get(b"dlss5nr_process\0") {
                Ok(s) => s,
                Err(_) => return (DlssAvailability::InitFailed, None),
            };
            let shutdown: libloading::Symbol<FnShutdown> = match lib.get(b"dlss5nr_shutdown\0") {
                Ok(s) => s,
                Err(_) => return (DlssAvailability::InitFailed, None),
            };
            let gpu_name: libloading::Symbol<FnGpuName> = match lib.get(b"dlss5nr_gpu_name\0") {
                Ok(s) => s,
                Err(_) => return (DlssAvailability::InitFailed, None),
            };
            (*init, *process, *shutdown, *gpu_name)
        };

        let wide = to_wide(&runtime_dir);
        let mut err = vec![0i8; 4096];
        let ok = unsafe { init_fn(0, wide.as_ptr(), err.as_mut_ptr(), err.len() as i32) };
        if ok == 0 {
            let msg = cbuf_to_string(&err);
            tracing::warn!(%msg, "dlss5nr_init failed (FeatureNotSupported common on non-RTX-50)");
            // init already called ShutdownUnlocked on failure.
            drop(lib);
            return (DlssAvailability::InitFailed, None);
        }

        let gpu_name_cache = unsafe {
            let p = gpu_name_fn();
            if p.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        };

        tracing::info!(
            gpu = %gpu_name_cache,
            runtime = %runtime_dir.display(),
            "DLSS 5 NR bridge initialized (NGX feature 18)"
        );

        (
            DlssAvailability::Ready,
            Some(Self {
                _lib: lib,
                process,
                shutdown,
                gpu_name_cache,
                style: 1, // Natural
                preset: 0,
                intensity: 1.0,
                temporal: true,
            }),
        )
    }

    pub fn gpu_name(&self) -> &str {
        &self.gpu_name_cache
    }

    pub fn process_rgb(
        &mut self,
        rgb_in: &[f32],
        rgb_out: &mut [f32],
        width: u32,
        height: u32,
        reset: bool,
    ) -> Result<(), String> {
        let px = (width as usize).saturating_mul(height as usize);
        if rgb_in.len() < px * 3 || rgb_out.len() < px * 3 {
            return Err("RGB buffer size mismatch".into());
        }
        let mut err = vec![0i8; 4096];
        let ok = unsafe {
            (self.process)(
                rgb_in.as_ptr(),
                rgb_out.as_mut_ptr(),
                width as i32,
                height as i32,
                self.style,
                self.preset,
                self.intensity,
                1.0, // tone
                1.0, // structure
                -1.0, // skin (default)
                0,   // automask
                if reset { 1 } else { 0 },
                if self.temporal { 1 } else { 0 },
                err.as_mut_ptr(),
                err.len() as i32,
            )
        };
        if ok == 0 {
            return Err(cbuf_to_string(&err));
        }
        Ok(())
    }
}

impl Drop for NrRuntime {
    fn drop(&mut self) {
        unsafe {
            (self.shutdown)();
        }
    }
}

fn resolve_runtime_dir() -> Option<PathBuf> {
    for key in ["CAPTUREVIEW_DLSSNR_DIR", "STREAMLINE_SDK_PATH"] {
        if let Ok(v) = std::env::var(key) {
            let p = PathBuf::from(v.trim());
            if p.is_dir() {
                return Some(p);
            }
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        let p = PathBuf::from(appdata).join("CaptureView").join("dlss5-nr");
        if p.is_dir() {
            return Some(p);
        }
    }
    // Repo / worktree layout for developers.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let native_rt = manifest
        .join("..")
        .join("..")
        .join("native")
        .join("dlss5-nr")
        .join("runtime");
    if native_rt.is_dir() {
        return Some(native_rt);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for c in [dir.join("dlss5-nr"), dir.join("runtime")] {
                if c.is_dir() {
                    return Some(c);
                }
            }
        }
    }
    None
}

fn resolve_bridge_dll(runtime_dir: &Path) -> Option<PathBuf> {
    let candidates = [
        runtime_dir.join("dlss5nr_bridge.dll"),
        runtime_dir.join("bin").join("dlss5nr_bridge.dll"),
        runtime_dir
            .parent()
            .map(|p| p.join("bin").join("dlss5nr_bridge.dll"))
            .unwrap_or_default(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("native")
            .join("dlss5-nr")
            .join("bin")
            .join("dlss5nr_bridge.dll"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

fn to_wide(path: &Path) -> Vec<u16> {
    OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn cbuf_to_string(buf: &[i8]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_runtime_is_sdk_missing_or_init() {
        // Without nvngx_dlssnr.dll this should not panic.
        let (a, rt) = NrRuntime::try_open(0x10DE, "NVIDIA GeForce RTX 5090", wgpu::Backend::Dx12);
        assert!(
            matches!(
                a,
                DlssAvailability::SdkMissing
                    | DlssAvailability::InitFailed
                    | DlssAvailability::Ready
            ),
            "unexpected {a:?}"
        );
        let _ = rt;
    }
}
