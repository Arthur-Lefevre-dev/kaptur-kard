//! Dynamic loader for `dlss5g_bridge.dll` (NGX FrameGeneration EvaluateFeature + present queue).

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use libloading::Library;

use super::probe::nvidia_vendor;

/// Outcome of trying to open the FG bridge (mapped to `DlssGAvailability` in `fg.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FgOpenStatus {
    NotNvidia,
    WrongBackend,
    SdkMissing,
    InitFailed,
    Ready,
}

type FnInit = unsafe extern "C" fn(i32, *const u16, *mut i8, i32) -> i32;
type FnProcess = unsafe extern "C" fn(
    *const f32,
    *const f32,
    *mut f32,
    i32,
    i32,
    i32,
    i32,
    *mut i8,
    i32,
) -> i32;
type FnShutdown = unsafe extern "C" fn();
type FnGpuName = unsafe extern "C" fn() -> *const i8;
type FnPresentReady = unsafe extern "C" fn() -> i32;

pub struct FgRuntime {
    _lib: Library,
    process: FnProcess,
    shutdown: FnShutdown,
    present_ready: FnPresentReady,
    pub gpu_name_cache: String,
}

impl FgRuntime {
    pub fn try_open(
        vendor_id: u32,
        name: &str,
        backend: wgpu::Backend,
    ) -> (FgOpenStatus, Option<Self>) {
        if !nvidia_vendor(vendor_id, name) {
            return (FgOpenStatus::NotNvidia, None);
        }
        match backend {
            wgpu::Backend::Dx12 | wgpu::Backend::Vulkan => {}
            _ => return (FgOpenStatus::WrongBackend, None),
        }

        let Some(runtime_dir) = resolve_runtime_dir() else {
            tracing::info!("DLSS-G runtime dir not found");
            return (FgOpenStatus::SdkMissing, None);
        };

        let fg_dll = runtime_dir.join("nvngx_dlssg.dll");
        if !fg_dll.is_file() {
            tracing::info!(
                path = %fg_dll.display(),
                "nvngx_dlssg.dll missing — BYO optional FG (docs/DLSS.md)"
            );
            return (FgOpenStatus::SdkMissing, None);
        }

        let Some(bridge_path) = resolve_bridge_dll(&runtime_dir) else {
            tracing::warn!("dlss5g_bridge.dll not found — run native/dlss5-nr/build_native.ps1");
            return (FgOpenStatus::SdkMissing, None);
        };

        let lib = match unsafe { Library::new(&bridge_path) } {
            Ok(l) => l,
            Err(err) => {
                tracing::warn!(%err, path = %bridge_path.display(), "failed to load dlss5g_bridge.dll");
                return (FgOpenStatus::InitFailed, None);
            }
        };

        let (init_fn, process, shutdown, gpu_name_fn, present_ready) = unsafe {
            let init: libloading::Symbol<FnInit> = match lib.get(b"dlss5g_init\0") {
                Ok(s) => s,
                Err(_) => return (FgOpenStatus::InitFailed, None),
            };
            let process: libloading::Symbol<FnProcess> = match lib.get(b"dlss5g_process\0") {
                Ok(s) => s,
                Err(_) => return (FgOpenStatus::InitFailed, None),
            };
            let shutdown: libloading::Symbol<FnShutdown> = match lib.get(b"dlss5g_shutdown\0") {
                Ok(s) => s,
                Err(_) => return (FgOpenStatus::InitFailed, None),
            };
            let gpu_name: libloading::Symbol<FnGpuName> = match lib.get(b"dlss5g_gpu_name\0") {
                Ok(s) => s,
                Err(_) => return (FgOpenStatus::InitFailed, None),
            };
            let present: libloading::Symbol<FnPresentReady> =
                match lib.get(b"dlss5g_present_queue_ready\0") {
                    Ok(s) => s,
                    Err(_) => return (FgOpenStatus::InitFailed, None),
                };
            (*init, *process, *shutdown, *gpu_name, *present)
        };

        let wide = to_wide(&runtime_dir);
        let mut err = vec![0i8; 4096];
        let ok = unsafe { init_fn(0, wide.as_ptr(), err.as_mut_ptr(), err.len() as i32) };
        if ok == 0 {
            let msg = cbuf_to_string(&err);
            tracing::warn!(%msg, "dlss5g_init failed");
            drop(lib);
            return (FgOpenStatus::InitFailed, None);
        }

        let gpu_name_cache = unsafe {
            let p = gpu_name_fn();
            if p.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        };

        let present_ok = unsafe { present_ready() } != 0;
        tracing::info!(
            gpu = %gpu_name_cache,
            present_queue = present_ok,
            runtime = %runtime_dir.display(),
            "DLSS-G bridge initialized (EvaluateFeature + dedicated present queue)"
        );

        (
            FgOpenStatus::Ready,
            Some(Self {
                _lib: lib,
                process,
                shutdown,
                present_ready,
                gpu_name_cache,
            }),
        )
    }

    pub fn gpu_name(&self) -> &str {
        &self.gpu_name_cache
    }

    pub fn present_queue_ready(&self) -> bool {
        unsafe { (self.present_ready)() != 0 }
    }

    /// Generate `generated_count` RGB frames between `prev` and `curr`.
    pub fn process_pair(
        &mut self,
        prev_rgb: &[f32],
        curr_rgb: &[f32],
        out_rgb: &mut [f32],
        width: u32,
        height: u32,
        generated_count: u32,
        reset: bool,
    ) -> Result<u32, String> {
        let px = (width as usize).saturating_mul(height as usize);
        let gen = generated_count.clamp(1, 5) as usize;
        if prev_rgb.len() < px * 3 || curr_rgb.len() < px * 3 || out_rgb.len() < px * 3 * gen {
            return Err("FG RGB buffer size mismatch".into());
        }
        let mut err = vec![0i8; 4096];
        let n = unsafe {
            (self.process)(
                prev_rgb.as_ptr(),
                curr_rgb.as_ptr(),
                out_rgb.as_mut_ptr(),
                width as i32,
                height as i32,
                gen as i32,
                if reset { 1 } else { 0 },
                err.as_mut_ptr(),
                err.len() as i32,
            )
        };
        if n <= 0 {
            return Err(cbuf_to_string(&err));
        }
        Ok(n as u32)
    }
}

impl Drop for FgRuntime {
    fn drop(&mut self) {
        unsafe {
            (self.shutdown)();
        }
    }
}

fn resolve_runtime_dir() -> Option<PathBuf> {
    for key in [
        "CAPTUREVIEW_DLSSG_DIR",
        "CAPTUREVIEW_DLSSNR_DIR",
        "STREAMLINE_SDK_PATH",
    ] {
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
    let native_rt = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("native")
        .join("dlss5-nr")
        .join("runtime");
    if native_rt.is_dir() {
        return Some(native_rt);
    }
    None
}

fn resolve_bridge_dll(runtime_dir: &Path) -> Option<PathBuf> {
    let candidates = [
        runtime_dir.join("dlss5g_bridge.dll"),
        runtime_dir.join("bin").join("dlss5g_bridge.dll"),
        runtime_dir
            .parent()
            .map(|p| p.join("bin").join("dlss5g_bridge.dll"))
            .unwrap_or_default(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("native")
            .join("dlss5-nr")
            .join("bin")
            .join("dlss5g_bridge.dll"),
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
    fn missing_dll_is_graceful() {
        let (a, rt) = FgRuntime::try_open(0x10DE, "NVIDIA GeForce RTX 5090", wgpu::Backend::Dx12);
        assert!(
            matches!(
                a,
                FgOpenStatus::SdkMissing | FgOpenStatus::InitFailed | FgOpenStatus::Ready
            ),
            "unexpected {a:?}"
        );
        let _ = rt;
    }

    #[test]
    fn amd_refused() {
        let (a, rt) = FgRuntime::try_open(0x1002, "AMD Radeon", wgpu::Backend::Dx12);
        assert_eq!(a, FgOpenStatus::NotNvidia);
        assert!(rt.is_none());
    }
}
