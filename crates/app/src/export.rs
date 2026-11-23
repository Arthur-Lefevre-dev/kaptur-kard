//! Spout (Windows, bundled) / NDI (BYO) export — see docs/EXPORT.md.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ExportPrefs {
    /// Share presented frames via Spout2 when the runtime DLL is available.
    #[serde(default)]
    pub spout_enabled: bool,
    /// Spout sender name (OBS / Resolve Spout input).
    #[serde(default = "default_spout_name")]
    pub spout_name: String,
    /// NDI sender (requires NewTek NDI Runtime — BYO; we never redistribute NDI DLLs).
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
    /// True after at least one successful SendImage this session.
    pub spout_sending: bool,
    /// i18n key for Spout status line.
    pub spout_detail_key: String,
    /// Resolved SpoutLibrary.dll path (may be empty).
    pub spout_dll_path: String,
    pub ndi_available: bool,
    pub ndi_active: bool,
    pub ndi_detail_key: String,
    /// Folder containing Processing.NDI.Lib.x64.dll when found.
    pub ndi_folder: String,
}

pub const NDI_DOWNLOAD_URL: &str = "https://ndi.video/";
pub const DLSS_DOCS_URL: &str =
    "https://github.com/Arthur-Lefevre-dev/kaptur-kard/blob/main/docs/DLSS.md";

/// Probe optional runtimes without linking them (LoadLibrary / path checks).
pub fn probe_export() -> ExportStatus {
    let mut st = ExportStatus::default();
    #[cfg(windows)]
    {
        ensure_spout_extracted();
        if let Some(path) = find_spout_library() {
            st.spout_available = true;
            st.spout_dll_path = path.display().to_string();
            st.spout_detail_key = if path_is_bundled(&path) {
                "export_spout_ok_bundled".into()
            } else {
                "export_spout_ok_external".into()
            };
        } else {
            st.spout_detail_key = "export_spout_missing".into();
        }
        if let Some(folder) = find_ndi_folder() {
            st.ndi_available = true;
            st.ndi_folder = folder.display().to_string();
            st.ndi_detail_key = "export_ndi_ok".into();
        } else {
            st.ndi_detail_key = "export_ndi_missing".into();
        }
    }
    #[cfg(not(windows))]
    {
        st.spout_detail_key = "export_spout_windows_only".into();
        st.ndi_detail_key = "export_ndi_missing".into();
        st.ndi_available = false;
    }
    st
}

fn path_is_bundled(path: &Path) -> bool {
    let s = path.to_string_lossy().to_ascii_lowercase();
    if let Some(exe) = exe_dir() {
        if path.starts_with(&exe) {
            return true;
        }
    }
    if let Some(dir) = appdata_spout_dir() {
        if path.starts_with(&dir) {
            return true;
        }
    }
    s.contains("third_party") || s.contains("native\\spout") || s.contains("native/spout")
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
}

fn appdata_spout_dir() -> Option<PathBuf> {
    crate::prefs::app_data_dir().map(|d| d.join("spout"))
}

/// Bundled SpoutLibrary.dll bytes (Windows release / first-run extract).
#[cfg(windows)]
fn bundled_spout_bytes() -> &'static [u8] {
    // include_bytes! fails the build if missing — keep file committed.
    include_bytes!("../../../third_party/spout/SpoutLibrary.dll")
}

#[cfg(windows)]
fn ensure_spout_extracted() {
    let bytes = bundled_spout_bytes();
    // Prefer next to the executable (release zip layout).
    if let Some(exe_dir) = exe_dir() {
        let dest = exe_dir.join("SpoutLibrary.dll");
        if !dest.is_file() {
            if let Err(e) = std::fs::write(&dest, bytes) {
                tracing::debug!(error = %e, "could not extract SpoutLibrary.dll next to exe");
            }
        }
        let bridge_src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../third_party/spout/spout_bridge.dll");
        let bridge_dest = exe_dir.join("spout_bridge.dll");
        if bridge_src.is_file() && !bridge_dest.is_file() {
            let _ = std::fs::copy(&bridge_src, &bridge_dest);
        }
    }
    // Also keep a copy under %APPDATA%\CaptureView\spout\ for cargo-run layouts.
    if let Some(dir) = appdata_spout_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let dest = dir.join("SpoutLibrary.dll");
        if !dest.is_file() {
            let _ = std::fs::write(&dest, bytes);
        }
        let bridge_src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../third_party/spout/spout_bridge.dll");
        let bridge_dest = dir.join("spout_bridge.dll");
        if bridge_src.is_file() && !bridge_dest.is_file() {
            let _ = std::fs::copy(&bridge_src, &bridge_dest);
        }
    }
}

/// Search order: next to exe → APPDATA spout → third_party (dev) → PATH via LoadLibrary.
#[cfg(windows)]
fn find_spout_library() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(exe) = exe_dir() {
        candidates.push(exe.join("SpoutLibrary.dll"));
        candidates.push(exe.join("spout").join("SpoutLibrary.dll"));
    }
    if let Some(dir) = appdata_spout_dir() {
        candidates.push(dir.join("SpoutLibrary.dll"));
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    candidates.push(manifest.join("../../third_party/spout/SpoutLibrary.dll"));
    candidates.push(manifest.join("../../native/spout/bin/SpoutLibrary.dll"));

    for c in &candidates {
        if c.is_file() {
            return Some(c.canonicalize().unwrap_or_else(|_| c.clone()));
        }
    }
    // Last resort: LoadLibrary search path (PATH / system).
    if load_library_probe("SpoutLibrary.dll") || load_library_probe("Spout.dll") {
        return Some(PathBuf::from("SpoutLibrary.dll"));
    }
    None
}

#[cfg(windows)]
fn find_spout_bridge_dir(spout_dll: &Path) -> PathBuf {
    if let Some(parent) = spout_dll.parent() {
        let next = parent.join("spout_bridge.dll");
        if next.is_file() {
            return parent.to_path_buf();
        }
    }
    if let Some(exe) = exe_dir() {
        if exe.join("spout_bridge.dll").is_file() {
            return exe;
        }
    }
    if let Some(dir) = appdata_spout_dir() {
        if dir.join("spout_bridge.dll").is_file() {
            return dir;
        }
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let tp = manifest.join("../../third_party/spout");
    if tp.join("spout_bridge.dll").is_file() {
        return tp;
    }
    let bin = manifest.join("../../native/spout/bin");
    if bin.join("spout_bridge.dll").is_file() {
        return bin;
    }
    spout_dll
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn find_ndi_folder() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let candidates = [
            std::env::var_os("NDI_RUNTIME_DIR_V6"),
            std::env::var_os("NDI_RUNTIME_DIR_V5"),
            std::env::var_os("NDI_RUNTIME_DIR"),
        ];
        for c in candidates.into_iter().flatten() {
            let dir = PathBuf::from(c);
            let dll = dir.join("Processing.NDI.Lib.x64.dll");
            if dll.is_file() {
                return Some(dir);
            }
        }
        if let Ok(pf) = std::env::var("ProgramFiles") {
            let root = PathBuf::from(pf).join("NDI");
            if let Some(found) = find_ndi_dll_under(&root) {
                return Some(found);
            }
        }
        if let Ok(pf) = std::env::var("ProgramFiles(x86)") {
            let root = PathBuf::from(pf).join("NDI");
            if let Some(found) = find_ndi_dll_under(&root) {
                return Some(found);
            }
        }
        None
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(windows)]
fn find_ndi_dll_under(root: &Path) -> Option<PathBuf> {
    if !root.is_dir() {
        return None;
    }
    let direct = root.join("Processing.NDI.Lib.x64.dll");
    if direct.is_file() {
        return Some(root.to_path_buf());
    }
    // Shallow walk (runtime / tools layouts).
    let Ok(entries) = std::fs::read_dir(root) else {
        return Some(root.to_path_buf()); // folder exists → treat as installed
    };
    for ent in entries.flatten() {
        let p = ent.path();
        if p.is_dir() {
            let dll = p.join("Processing.NDI.Lib.x64.dll");
            if dll.is_file() {
                return Some(p);
            }
            // One more level (e.g. NDI\NDI 6 Runtime\v6)
            if let Ok(sub) = std::fs::read_dir(&p) {
                for s in sub.flatten() {
                    let sp = s.path();
                    if sp.is_dir() {
                        let dll = sp.join("Processing.NDI.Lib.x64.dll");
                        if dll.is_file() {
                            return Some(sp);
                        }
                    }
                }
            }
        }
    }
    Some(root.to_path_buf())
}

#[cfg(windows)]
fn load_library_probe(name: &str) -> bool {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = std::ffi::OsStr::new(name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let h = unsafe { windows_sys_load(&wide) };
    if h != 0 {
        unsafe { windows_sys_free(h) };
        true
    } else {
        false
    }
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

pub fn open_url(url: &str) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

pub fn open_folder(path: &Path) {
    if path.as_os_str().is_empty() {
        return;
    }
    let _ = std::fs::create_dir_all(path);
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer")
            .arg(path.as_os_str())
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
}

pub fn dlss_byo_dir() -> Option<PathBuf> {
    crate::prefs::app_data_dir().map(|d| d.join("dlss5-nr"))
}

/// Lightweight sender state with optional Spout CPU SendImage path.
pub struct FrameExport {
    pub prefs: ExportPrefs,
    pub status: ExportStatus,
    #[cfg(windows)]
    spout: Option<SpoutSender>,
}

impl Default for FrameExport {
    fn default() -> Self {
        let mut s = Self {
            prefs: ExportPrefs::default(),
            status: probe_export(),
            #[cfg(windows)]
            spout: None,
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
        #[cfg(windows)]
        {
            if !self.status.spout_available {
                self.spout = None;
            }
        }
    }

    pub fn sync_active_flags(&mut self) {
        self.status.spout_active = self.prefs.spout_enabled && self.status.spout_available;
        self.status.ndi_active = self.prefs.ndi_enabled && self.status.ndi_available;
        if self.prefs.spout_enabled && !self.status.spout_available {
            self.status.spout_detail_key = "export_spout_enabled_missing".into();
        } else if self.status.spout_active && self.status.spout_sending {
            self.status.spout_detail_key = "export_spout_sending".into();
        } else if self.status.spout_active {
            self.status.spout_detail_key = "export_spout_ready".into();
        }
        if self.prefs.ndi_enabled && !self.status.ndi_available {
            self.status.ndi_detail_key = "export_ndi_enabled_missing".into();
        } else if self.status.ndi_active {
            self.status.ndi_detail_key = "export_ndi_scaffold".into();
        }
    }

    /// Ensure Spout sender is live when enabled (lazy init).
    pub fn ensure_spout(&mut self) {
        #[cfg(windows)]
        {
            if !self.prefs.spout_enabled || !self.status.spout_available {
                self.spout = None;
                return;
            }
            if self.spout.is_some() {
                return;
            }
            let dll = PathBuf::from(&self.status.spout_dll_path);
            let dir = find_spout_bridge_dir(&dll);
            match SpoutSender::open(&dir) {
                Ok(s) => {
                    self.spout = Some(s);
                    self.status.spout_detail_key = "export_spout_ready".into();
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Spout bridge init failed");
                    self.status.spout_detail_key = "export_spout_bridge_fail".into();
                    self.status.spout_active = false;
                }
            }
        }
    }

    /// Called each presented capture frame. Spout: CPU BGRA SendImage when format allows.
    /// NDI: probe/prefs only until a BYO SDK bridge is linked.
    pub fn on_frame(
        &mut self,
        width: u32,
        height: u32,
        pixel_format: capture::PixelFormat,
        pixels: &[u8],
    ) {
        #[cfg(windows)]
        {
            if self.prefs.spout_enabled && self.status.spout_available {
                self.ensure_spout();
                if let Some(sender) = self.spout.as_mut() {
                    if matches!(pixel_format, capture::PixelFormat::Bgra8) {
                        let expected = (width as usize).saturating_mul(height as usize) * 4;
                        if pixels.len() >= expected && width > 0 && height > 0 {
                            match sender.send_bgra(&self.prefs.spout_name, pixels, width, height) {
                                Ok(()) => {
                                    if !self.status.spout_sending {
                                        self.status.spout_sending = true;
                                        self.status.spout_detail_key = "export_spout_sending".into();
                                    }
                                }
                                Err(e) => {
                                    tracing::debug!(error = %e, "Spout SendImage failed");
                                    self.status.spout_detail_key = "export_spout_send_fail".into();
                                }
                            }
                        }
                    } else {
                        self.status.spout_detail_key = "export_spout_need_bgra".into();
                    }
                }
            }
        }
        let _ = (width, height, pixel_format, pixels);
        // NDI: intentional no-op until BYO SDK bridge (licensing).
    }
}

#[cfg(windows)]
struct SpoutSender {
    /// Keep the DLL mapped for the function pointers below.
    #[allow(dead_code)]
    lib: libloading::Library,
    bridge: *mut std::ffi::c_void,
    send_bgra: unsafe extern "C" fn(
        *mut std::ffi::c_void,
        *const std::os::raw::c_char,
        *const u8,
        u32,
        u32,
    ) -> i32,
    destroy: unsafe extern "C" fn(*mut std::ffi::c_void),
    last_error: unsafe extern "C" fn(*mut std::ffi::c_void) -> *const std::os::raw::c_char,
}

#[cfg(windows)]
impl SpoutSender {
    fn open(bridge_dir: &Path) -> Result<Self, String> {
        let bridge_path = bridge_dir.join("spout_bridge.dll");
        if !bridge_path.is_file() {
            return Err(format!("spout_bridge.dll missing in {}", bridge_dir.display()));
        }
        // Ensure SpoutLibrary.dll is discoverable next to the bridge.
        let spout = bridge_dir.join("SpoutLibrary.dll");
        if !spout.is_file() {
            return Err(format!(
                "SpoutLibrary.dll missing next to bridge ({})",
                bridge_dir.display()
            ));
        }
        unsafe {
            let lib = libloading::Library::new(&bridge_path)
                .map_err(|e| format!("load spout_bridge: {e}"))?;
            let create: libloading::Symbol<
                unsafe extern "C" fn(*const std::os::raw::c_char) -> *mut std::ffi::c_void,
            > = lib
                .get(b"spout_bridge_create")
                .map_err(|e| format!("spout_bridge_create: {e}"))?;
            let send_bgra: libloading::Symbol<
                unsafe extern "C" fn(
                    *mut std::ffi::c_void,
                    *const std::os::raw::c_char,
                    *const u8,
                    u32,
                    u32,
                ) -> i32,
            > = lib
                .get(b"spout_bridge_send_bgra")
                .map_err(|e| format!("spout_bridge_send_bgra: {e}"))?;
            let destroy: libloading::Symbol<unsafe extern "C" fn(*mut std::ffi::c_void)> = lib
                .get(b"spout_bridge_destroy")
                .map_err(|e| format!("spout_bridge_destroy: {e}"))?;
            let last_error: libloading::Symbol<
                unsafe extern "C" fn(*mut std::ffi::c_void) -> *const std::os::raw::c_char,
            > = lib
                .get(b"spout_bridge_last_error")
                .map_err(|e| format!("spout_bridge_last_error: {e}"))?;

            let dir_c = std::ffi::CString::new(bridge_dir.to_string_lossy().as_bytes())
                .map_err(|_| "bridge dir contains NUL".to_string())?;
            let bridge = create(dir_c.as_ptr());
            if bridge.is_null() {
                return Err("spout_bridge_create returned null".into());
            }
            let err_ptr = last_error(bridge);
            if !err_ptr.is_null() {
                let msg = std::ffi::CStr::from_ptr(err_ptr).to_string_lossy();
                if !msg.is_empty() {
                    destroy(bridge);
                    return Err(msg.into_owned());
                }
            }
            Ok(Self {
                send_bgra: *send_bgra,
                destroy: *destroy,
                last_error: *last_error,
                bridge,
                lib,
            })
        }
    }

    fn send_bgra(&mut self, name: &str, pixels: &[u8], w: u32, h: u32) -> Result<(), String> {
        let name_c = std::ffi::CString::new(name).unwrap_or_default();
        let ok = unsafe {
            (self.send_bgra)(self.bridge, name_c.as_ptr(), pixels.as_ptr(), w, h)
        };
        if ok != 0 {
            Ok(())
        } else {
            let msg = unsafe {
                let p = (self.last_error)(self.bridge);
                if p.is_null() {
                    "SendImage failed".into()
                } else {
                    std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
                }
            };
            Err(msg)
        }
    }
}

#[cfg(windows)]
impl Drop for SpoutSender {
    fn drop(&mut self) {
        unsafe {
            (self.destroy)(self.bridge);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_runs() {
        let st = probe_export();
        assert!(!st.spout_detail_key.is_empty());
    }
}
