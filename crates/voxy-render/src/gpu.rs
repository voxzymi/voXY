//! Headless GPU bootstrap (tests, export, tools).
//!
//! The desktop app does not use this: it shares eframe's `wgpu::Device` so the
//! viewport can display compositor output without copying it.

#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error("no suitable GPU adapter: {0}")]
    NoAdapter(#[from] wgpu::RequestAdapterError),
    #[error("could not create GPU device: {0}")]
    Device(#[from] wgpu::RequestDeviceError),
}

/// An adapter, device and queue created without a window.
#[derive(Debug)]
pub struct GpuContext {
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl GpuContext {
    /// Creates a headless context. Honors wgpu's standard environment
    /// variables (`WGPU_BACKEND`, `WGPU_ADAPTER_NAME`, `WGPU_POWER_PREF`).
    pub fn headless() -> Result<Self, GpuError> {
        pollster::block_on(Self::headless_async())
    }

    async fn headless_async() -> Result<Self, GpuError> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None).await?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("voxy-headless"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults()
                    .using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await?;
        Ok(GpuContext {
            adapter,
            device,
            queue,
        })
    }

    /// Human-readable adapter description, e.g. for golden-test reports.
    pub fn adapter_summary(&self) -> String {
        let info = self.adapter.get_info();
        format!(
            "{} ({:?}, driver {} {})",
            info.name, info.backend, info.driver, info.driver_info
        )
    }
}
