//! OS-level display capability probes (Windows HDR / DXGI tearing).
//!
//! Surface present modes already expose tearing via wgpu; this module adds
//! whether Windows Advanced Color (HDR) is enabled on any active path.

/// Coarse OS display status for UI / metrics (advisory; re-query on demand).
#[derive(Clone, Debug, Default)]
pub struct OsDisplayCaps {
    /// At least one active path reports advanced color supported.
    pub hdr_supported: bool,
    /// At least one active path has advanced color currently enabled.
    pub hdr_enabled: bool,
    /// DXGI factory reports PRESENT_ALLOW_TEARING (system-wide).
    pub tearing_supported: bool,
}

impl OsDisplayCaps {
    pub fn probe() -> Self {
        #[cfg(windows)]
        {
            windows_probe()
        }
        #[cfg(not(windows))]
        {
            Self::default()
        }
    }
}

#[cfg(windows)]
fn windows_probe() -> OsDisplayCaps {
    let mut caps = OsDisplayCaps {
        tearing_supported: dxgi_tearing_supported(),
        ..Default::default()
    };
    probe_advanced_color(&mut caps);
    caps
}

#[cfg(windows)]
fn dxgi_tearing_supported() -> bool {
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIFactory5, DXGI_FEATURE_PRESENT_ALLOW_TEARING,
    };

    unsafe {
        let factory: windows::core::Result<windows::Win32::Graphics::Dxgi::IDXGIFactory1> =
            CreateDXGIFactory1();
        let Ok(factory) = factory else {
            return false;
        };
        let Ok(factory5) = factory.cast::<IDXGIFactory5>() else {
            return false;
        };
        let mut allow: i32 = 0;
        let ok = factory5.CheckFeatureSupport(
            DXGI_FEATURE_PRESENT_ALLOW_TEARING,
            &mut allow as *mut i32 as *mut _,
            std::mem::size_of_val(&allow) as u32,
        );
        ok.is_ok() && allow != 0
    }
}

#[cfg(windows)]
fn probe_advanced_color(caps: &mut OsDisplayCaps) {
    use windows::Win32::Devices::Display::{
        DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
        DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_HEADER,
        DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
        DISPLAYCONFIG_PATH_TARGET_INFO, QDC_ONLY_ACTIVE_PATHS,
    };
    use windows::Win32::Foundation::ERROR_SUCCESS;

    unsafe {
        let mut path_count = 0u32;
        let mut mode_count = 0u32;
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
            != ERROR_SUCCESS
        {
            return;
        }
        if path_count == 0 {
            return;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            None,
        ) != ERROR_SUCCESS
        {
            return;
        }
        paths.truncate(path_count as usize);

        for path in &paths {
            let target: DISPLAYCONFIG_PATH_TARGET_INFO = path.targetInfo;
            let mut info = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
                    size: std::mem::size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32,
                    adapterId: target.adapterId,
                    id: target.id,
                },
                ..Default::default()
            };
            let hr = DisplayConfigGetDeviceInfo(
                &mut info.header as *mut DISPLAYCONFIG_DEVICE_INFO_HEADER,
            );
            if hr != 0 {
                continue;
            }
            // Bits: 0 = advancedColorSupported, 1 = advancedColorEnabled (MSDN).
            let value = info.Anonymous.value;
            if value & 0x1 != 0 {
                caps.hdr_supported = true;
            }
            if value & 0x2 != 0 {
                caps.hdr_enabled = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_does_not_panic() {
        let _ = OsDisplayCaps::probe();
    }
}
