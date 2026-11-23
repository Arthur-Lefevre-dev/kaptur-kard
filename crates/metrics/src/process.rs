//! Process / system monitoring samples. Call from the UI thread only (not the capture hot path).

#[derive(Clone, Debug, Default)]
pub struct ProcessSample {
    /// Working set in bytes.
    pub rss_bytes: u64,
    /// CPU usage as a fraction of one logical core (1.0 = 100% of one core).
    pub cpu_one_core: f32,
    /// System-wide average CPU busy fraction across all logical cores (0..=1).
    pub system_cpu_frac: f32,
    /// Per-logical-core busy fraction (0..=1). Empty when unavailable.
    pub per_core_frac: Vec<f32>,
    /// Process affinity bit mask (bit i = logical core i may run the process). Low 64 bits.
    pub affinity_mask: u64,
    /// Logical processor count reported by the OS.
    pub logical_cores: u32,
    /// Physical RAM in use (total − available).
    pub system_ram_used_bytes: u64,
    /// Physical RAM installed.
    pub system_ram_total_bytes: u64,
    /// Process disk read bytes/sec (smoothed over sample interval).
    pub io_read_bps: f64,
    /// Process disk write bytes/sec (smoothed over sample interval).
    pub io_write_bps: f64,
    /// Dedicated GPU (local) video memory in use, when DXGI reports it.
    pub vram_used_bytes: u64,
    /// Dedicated GPU (local) video memory budget, when DXGI reports it.
    pub vram_budget_bytes: u64,
}

#[cfg(windows)]
mod win {
    use super::ProcessSample;
    use std::mem::{size_of, MaybeUninit};
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows::Win32::System::SystemInformation::{
        GlobalMemoryStatusEx, MEMORYSTATUSEX,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetProcessAffinityMask, GetProcessTimes,
    };

    struct CpuState {
        last_kernel: u64,
        last_user: u64,
        last_wall: Instant,
    }

    struct IoState {
        last_read: u64,
        last_write: u64,
        last_wall: Instant,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct SystemProcessorPerformanceInformation {
        idle_time: i64,
        kernel_time: i64,
        user_time: i64,
        dpc_time: i64,
        interrupt_time: i64,
        interrupt_count: u32,
    }

    /// ntdll SystemProcessorPerformanceInformation = 8
    const SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION: i32 = 8;

    #[link(name = "ntdll")]
    extern "system" {
        fn NtQuerySystemInformation(
            class: i32,
            info: *mut core::ffi::c_void,
            len: u32,
            ret_len: *mut u32,
        ) -> i32;
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
                size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
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

    fn system_ram() -> (u64, u64) {
        unsafe {
            let mut status = MEMORYSTATUSEX {
                dwLength: size_of::<MEMORYSTATUSEX>() as u32,
                ..Default::default()
            };
            if GlobalMemoryStatusEx(&mut status).is_ok() {
                let total = status.ullTotalPhys;
                let avail = status.ullAvailPhys;
                (total.saturating_sub(avail), total)
            } else {
                (0, 0)
            }
        }
    }

    fn affinity_and_cores() -> (u64, u32) {
        unsafe {
            let mut process_mask: usize = 0;
            let mut system_mask: usize = 0;
            if GetProcessAffinityMask(
                GetCurrentProcess(),
                &mut process_mask,
                &mut system_mask,
            )
            .is_ok()
            {
                let cores = system_mask.count_ones().max(1);
                (process_mask as u64, cores)
            } else {
                (u64::MAX, 1)
            }
        }
    }

    fn per_core_and_system_cpu() -> (Vec<f32>, f32) {
        static PREV: OnceLock<Mutex<Option<Vec<SystemProcessorPerformanceInformation>>>> =
            OnceLock::new();
        let lock = PREV.get_or_init(|| Mutex::new(None));

        // Cap at 256 logical cores — enough for consumer / HEDT; avoids huge stack.
        const MAX_CORES: usize = 256;
        let mut buf = [SystemProcessorPerformanceInformation::default(); MAX_CORES];
        let mut ret_len = 0u32;
        let status = unsafe {
            NtQuerySystemInformation(
                SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION,
                buf.as_mut_ptr() as *mut _,
                (size_of::<SystemProcessorPerformanceInformation>() * MAX_CORES) as u32,
                &mut ret_len,
            )
        };
        if status < 0 || ret_len == 0 {
            return (Vec::new(), 0.0);
        }
        let n = (ret_len as usize / size_of::<SystemProcessorPerformanceInformation>())
            .min(MAX_CORES)
            .max(1);
        let current = buf[..n].to_vec();

        let Ok(mut guard) = lock.lock() else {
            return (Vec::new(), 0.0);
        };
        let Some(prev) = guard.as_ref() else {
            *guard = Some(current);
            return (vec![0.0; n], 0.0);
        };
        if prev.len() != n {
            *guard = Some(current);
            return (vec![0.0; n], 0.0);
        }

        let mut fracs = Vec::with_capacity(n);
        let mut sum = 0.0f32;
        for i in 0..n {
            let di = (current[i].idle_time - prev[i].idle_time) as f64;
            let dk = (current[i].kernel_time - prev[i].kernel_time) as f64;
            let du = (current[i].user_time - prev[i].user_time) as f64;
            // Kernel includes idle on Windows; busy = (kernel - idle) + user.
            let total = (dk + du).max(1.0);
            let busy = ((dk - di) + du).max(0.0);
            let frac = (busy / total).clamp(0.0, 1.0) as f32;
            fracs.push(frac);
            sum += frac;
        }
        *guard = Some(current);
        let avg = sum / n as f32;
        (fracs, avg)
    }

    fn process_io_bps() -> (f64, f64) {
        #[repr(C)]
        struct IoCounters {
            read_op: u64,
            write_op: u64,
            other_op: u64,
            read_bytes: u64,
            write_bytes: u64,
            other_bytes: u64,
        }

        #[link(name = "kernel32")]
        extern "system" {
            fn GetProcessIoCounters(process: HANDLE, counters: *mut IoCounters) -> i32;
        }

        static STATE: OnceLock<Mutex<Option<IoState>>> = OnceLock::new();
        let lock = STATE.get_or_init(|| Mutex::new(None));

        let mut counters = IoCounters {
            read_op: 0,
            write_op: 0,
            other_op: 0,
            read_bytes: 0,
            write_bytes: 0,
            other_bytes: 0,
        };
        let ok = unsafe { GetProcessIoCounters(GetCurrentProcess(), &mut counters) };
        if ok == 0 {
            return (0.0, 0.0);
        }
        let now = Instant::now();
        let Ok(mut guard) = lock.lock() else {
            return (0.0, 0.0);
        };
        match guard.as_mut() {
            None => {
                *guard = Some(IoState {
                    last_read: counters.read_bytes,
                    last_write: counters.write_bytes,
                    last_wall: now,
                });
                (0.0, 0.0)
            }
            Some(st) => {
                let wall = now.duration_since(st.last_wall).as_secs_f64();
                let dr = counters.read_bytes.saturating_sub(st.last_read) as f64;
                let dw = counters.write_bytes.saturating_sub(st.last_write) as f64;
                st.last_read = counters.read_bytes;
                st.last_write = counters.write_bytes;
                st.last_wall = now;
                if wall <= 0.0 {
                    (0.0, 0.0)
                } else {
                    (dr / wall, dw / wall)
                }
            }
        }
    }

    fn vram_local() -> (u64, u64) {
        use windows::core::Interface;
        use windows::Win32::Graphics::Dxgi::{
            CreateDXGIFactory1, IDXGIAdapter3, IDXGIFactory1, DXGI_MEMORY_SEGMENT_GROUP_LOCAL,
            DXGI_QUERY_VIDEO_MEMORY_INFO,
        };

        unsafe {
            let factory: windows::core::Result<IDXGIFactory1> = CreateDXGIFactory1();
            let Ok(factory) = factory else {
                return (0, 0);
            };
            // Prefer the first adapter that reports a non-zero local budget (skip Microsoft Basic).
            let mut i = 0u32;
            loop {
                let adapter = match factory.EnumAdapters1(i) {
                    Ok(a) => a,
                    Err(_) => break,
                };
                i += 1;
                let Ok(adapter3) = adapter.cast::<IDXGIAdapter3>() else {
                    continue;
                };
                let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
                if adapter3
                    .QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info)
                    .is_err()
                {
                    continue;
                }
                if info.Budget > 0 {
                    return (info.CurrentUsage, info.Budget);
                }
            }
            (0, 0)
        }
    }

    pub fn sample() -> ProcessSample {
        let (ram_used, ram_total) = system_ram();
        let (affinity_mask, logical_cores) = affinity_and_cores();
        let (per_core_frac, system_cpu_frac) = per_core_and_system_cpu();
        let (io_read_bps, io_write_bps) = process_io_bps();
        let (vram_used_bytes, vram_budget_bytes) = vram_local();
        ProcessSample {
            rss_bytes: rss_bytes(),
            cpu_one_core: cpu_one_core(),
            system_cpu_frac,
            per_core_frac,
            affinity_mask,
            logical_cores,
            system_ram_used_bytes: ram_used,
            system_ram_total_bytes: ram_total,
            io_read_bps,
            io_write_bps,
            vram_used_bytes,
            vram_budget_bytes,
        }
    }
}

#[cfg(windows)]
pub use win::sample;

#[cfg(not(windows))]
pub fn sample() -> ProcessSample {
    ProcessSample::default()
}
