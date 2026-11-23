//! Spout (Windows) / NDI export — BYO runtime; see docs/EXPORT.md.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ExportPrefs {
    /// Share presented frames via Spout2 when the runtime DLL is available.
    #[serde(default)]
    pub spout_enabled: bool,
    /// Spout sender name (OBS / Resolve Spout input).
    #[serde(default = "default_spout_name")]
    pub spout_name: String,
    /// NDI sender (requires NewTek NDI Runtime — BYO).
    #[serde(default)]
    pub ndi_enabled: bool,
    #[serde(default = "default_ndi_name")]
    pub ndi_name: String,
}

fn default_spout_name() -> String {
    "KapturKard".into()
}

fn default_ndi_name() -> String {
    "KapturKard".into()
}

impl Default for ExportPrefs {
    fn default() -> Self {
        Self {
            spout_enabled: false,
            spout_name: default_spout_name(),
            ndi_enabled: false,
            ndi_name: default_ndi_name(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ExportStatus {
    pub spout_available: bool,
    pub spout_active: bool,
    pub spout_detail: String,
    pub ndi_available: bool,
    pub ndi_active: bool,
    pub ndi_detail: String,
}

/// Probe optional runtimes without linking them (LoadLibrary / path checks).
pub fn probe_export() -> ExportStatus {
    let mut st = ExportStatus::default();
    #[cfg(windows)]
    {
        st.spout_available = spout_dll_present();
        st.spout_detail = if st.spout_available {
            "SpoutLibrary.dll found — enable in Infos to share frames (docs/EXPORT.md)".into()
        } else {
            "Spout2 runtime not found — install Spout and place SpoutLibrary.dll on PATH (docs/EXPORT.md)".into()
        };
        st.ndi_available = ndi_runtime_present();
        st.ndi_detail = if st.ndi_available {
            "NDI Runtime detected — enable in Infos (docs/EXPORT.md)".into()
        } else {
            "NDI Runtime not found — install NewTek NDI Runtime (BYO, docs/EXPORT.md)".into()
        };
    }
    #[cfg(not(windows))]
    {
        st.spout_detail = "Spout is Windows-only.".into();
        st.ndi_detail = "NDI export scaffolding — install NDI Runtime if available for your OS (docs/EXPORT.md).".into();
        st.ndi_available = false;
    }
    st
}

#[cfg(windows)]
fn spout_dll_present() -> bool {
    use std::os::windows::ffi::OsStrExt;
    let names = ["SpoutLibrary.dll", "Spout.dll"];
    for name in names {
        let wide: Vec<u16> = std::ffi::OsStr::new(name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: LoadLibraryW with a NUL-terminated path; we FreeLibrary immediately.
        let h = unsafe { windows_sys_load(&wide) };
        if h != 0 {
            unsafe { windows_sys_free(h) };
            return true;
        }
    }
    false
}

#[cfg(windows)]
fn ndi_runtime_present() -> bool {
    // Common install: %ProgramFiles%\NDI\…\Processing.NDI.Lib.x64.dll
    let candidates = [
        std::env::var_os("NDI_RUNTIME_DIR_V6"),
        std::env::var_os("NDI_RUNTIME_DIR_V5"),
        std::env::var_os("NDI_RUNTIME_DIR"),
    ];
    for c in candidates.into_iter().flatten() {
        let p = std::path::PathBuf::from(c).join("Processing.NDI.Lib.x64.dll");
        if p.is_file() {
            return true;
        }
    }
    if let Ok(pf) = std::env::var("ProgramFiles") {
        let root = std::path::PathBuf::from(pf).join("NDI");
        if root.is_dir() {
            return true;
        }
    }
    false
}

#[cfg(windows)]
unsafe fn windows_sys_load(wide: &[u16]) -> isize {
    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(name: *const u16) -> isize;
    }
    LoadLibraryW(wide.as_ptr())
}

#[cfg(windows)]
unsafe fn windows_sys_free(h: isize) {
    #[link(name = "kernel32")]
    extern "system" {
        fn FreeLibrary(h: isize) -> i32;
    }
    let _ = FreeLibrary(h);
}

/// Lightweight sender state. Full GPU Spout/NDI hooks land when BYO SDKs are present;
/// this keeps UI + prefs coherent and documents the dependency path.
pub struct FrameExport {
    pub prefs: ExportPrefs,
    pub status: ExportStatus,
}

impl Default for FrameExport {
    fn default() -> Self {
        let mut s = Self {
            prefs: ExportPrefs::default(),
            status: probe_export(),
        };
        s.sync_active_flags();
        s
    }
}

impl FrameExport {
    pub fn refresh_probe(&mut self) {
        let spout_on = self.prefs.spout_enabled;
        let ndi_on = self.prefs.ndi_enabled;
        self.status = probe_export();
        self.prefs.spout_enabled = spout_on;
        self.prefs.ndi_enabled = ndi_on;
        self.sync_active_flags();
    }

    pub fn sync_active_flags(&mut self) {
        self.status.spout_active = self.prefs.spout_enabled && self.status.spout_available;
        self.status.ndi_active = self.prefs.ndi_enabled && self.status.ndi_available;
        if self.prefs.spout_enabled && !self.status.spout_available {
            self.status.spout_detail =
                "Spout enabled but DLL missing — install Spout2 (docs/EXPORT.md)".into();
        }
        if self.prefs.ndi_enabled && !self.status.ndi_available {
            self.status.ndi_detail =
                "NDI enabled but Runtime missing — install NDI Runtime (docs/EXPORT.md)".into();
        }
    }

    /// Called each presented frame. Currently a no-op until BYO SDK bridges ship;
    /// keeps the hot path allocation-free.
    pub fn on_frame(&mut self, _width: u32, _height: u32) {
        // Future: Spout SendTexture / NDI video frame from shared handle or BGRA.
    }
}
