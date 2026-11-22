//! Process CPU / RAM sampling. Call from the UI thread only (not the capture hot path).

#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessSample {
    /// Working set in bytes.
    pub rss_bytes: u64,
    /// CPU usage as a fraction of one logical core (1.0 = 100% of one core).
    pub cpu_one_core: f32,
}

#[cfg(windows)]
mod win {
    use super::ProcessSample;
    use std::mem::MaybeUninit;
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    struct CpuState {
        last_kernel: u64,
        last_user: u64,
        last_wall: Instant,
    }

    fn filetime_to_u64(ft: windows::Win32::Foundation::FILETIME) -> u64 {
        ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
    }

    fn rss_bytes() -> u64 {
        unsafe {
            let handle = GetCurrentProcess();
            let mut pmc = MaybeUninit::<PROCESS_MEMORY_COUNTERS>::zeroed();
            if GetProcessMemoryInfo(
                handle,
                pmc.as_mut_ptr(),
                std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            )
            .is_ok()
            {
                pmc.assume_init().WorkingSetSize as u64
            } else {
                0
            }
        }
    }

    fn cpu_one_core() -> f32 {
        // Mutex only for rare UI sampling — never on capture/present atomics path.
        static STATE: OnceLock<Mutex<Option<CpuState>>> = OnceLock::new();
        let lock = STATE.get_or_init(|| Mutex::new(None));

        let mut creation = Default::default();
        let mut exit = Default::default();
        let mut kernel = Default::default();
        let mut user = Default::default();
        unsafe {
            if GetProcessTimes(
                GetCurrentProcess(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            )
            .is_err()
            {
                return 0.0;
            }
        }
        let k = filetime_to_u64(kernel);
        let u = filetime_to_u64(user);
        let now = Instant::now();

        let Ok(mut guard) = lock.lock() else {
            return 0.0;
        };
        match guard.as_mut() {
            None => {
                *guard = Some(CpuState {
                    last_kernel: k,
                    last_user: u,
                    last_wall: now,
                });
                0.0
            }
            Some(st) => {
                let dk = k.saturating_sub(st.last_kernel);
                let du = u.saturating_sub(st.last_user);
                let wall = now.duration_since(st.last_wall).as_secs_f64();
                st.last_kernel = k;
                st.last_user = u;
                st.last_wall = now;
                if wall <= 0.0 {
                    0.0
                } else {
                    // FILETIME ticks are 100 ns.
                    let cpu_secs = (dk + du) as f64 * 100e-9;
                    (cpu_secs / wall) as f32
                }
            }
        }
    }

    pub fn sample() -> ProcessSample {
        ProcessSample {
            rss_bytes: rss_bytes(),
            cpu_one_core: cpu_one_core(),
        }
    }
}

#[cfg(windows)]
pub use win::sample;

#[cfg(not(windows))]
pub fn sample() -> ProcessSample {
    ProcessSample::default()
}
