use thiserror::Error;

#[derive(Debug, Error)]
pub enum GpuError {
    #[error("wgpu request device failed: {0}")]
    RequestDevice(String),
    #[error("surface error: {0}")]
    Surface(String),
}

/// Shared GPU handles owned by the app render loop.
pub struct GpuContext {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl GpuContext {
    pub async fn new() -> Result<Self, GpuError> {
        // Prefer a single native backend on Windows so we do not load Vulkan + DX12
        // drivers into the process working set.
        #[cfg(windows)]
        let backends = wgpu::Backends::DX12;
        #[cfg(not(windows))]
        let backends = wgpu::Backends::PRIMARY;

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends,
            ..Default::default()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .ok_or_else(|| GpuError::RequestDevice("no suitable GPU adapter".into()))?;

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("captureview-device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                },
                None,
            )
            .await
            .map_err(|e| GpuError::RequestDevice(e.to_string()))?;

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
        })
    }
}
