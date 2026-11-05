//! One wgpu device for the whole process (Metal on macOS), created on first
//! use with the adapter's own limits (the wgpu defaults cap buffers at
//! 256 MiB, far below what the M4 Max can hold).

use anyhow::{Context, Result, anyhow};
use std::sync::OnceLock;

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub info: wgpu::AdapterInfo,
    pub limits: wgpu::Limits,
    /// half-precision storage arrays (`enable f16;`) are available
    pub f16: bool,
}

static GPU: OnceLock<std::result::Result<Gpu, String>> = OnceLock::new();

/// The shared GPU context (created on the first call).
pub fn gpu() -> Result<&'static Gpu> {
    GPU.get_or_init(|| create().map_err(|e| format!("{e:#}"))).as_ref().map_err(|e| anyhow!("no usable GPU: {e}"))
}

fn create() -> Result<Gpu> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .context("no GPU adapter found")?;
    let limits = adapter.limits();
    let f16 = adapter.features().contains(wgpu::Features::SHADER_F16);
    let mut features = wgpu::Features::empty();
    if f16 {
        features |= wgpu::Features::SHADER_F16;
    }
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("simlab-cfd"),
        required_features: features,
        required_limits: limits.clone(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .context("could not open the GPU")?;
    let info = adapter.get_info();
    Ok(Gpu { device, queue, info, limits, f16 })
}

/// One plain line about the GPU, e.g. "Apple M4 Max · Metal · 38.9 GB max buffer".
pub fn gpu_info() -> Result<String> {
    let g = gpu()?;
    let backend = match g.info.backend {
        wgpu::Backend::Metal => "Metal".to_string(),
        b => format!("{b:?}"),
    };
    Ok(format!("{} · {} · {:.1} GB max buffer", g.info.name, backend, g.limits.max_buffer_size as f64 / 1e9))
}

impl Gpu {
    /// Largest single storage buffer we create. Kept under 4 GiB so every
    /// index and byte offset fits 32 bits whatever the shader compiler does.
    pub fn max_buffer(&self) -> u64 {
        self.limits.max_buffer_size.min(self.limits.max_storage_buffer_binding_size).min((1u64 << 32) - 256)
    }
}
