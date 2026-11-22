//! NVIDIA telemetry through NVML (`nvml.dll` / `libnvidia-ml.so` loaded at runtime).

use nvml_wrapper::Nvml;

use super::GpuSample;
use crate::devices::GpuInfo;

pub struct NvmlProvider {
    nvml: Nvml,
}

impl NvmlProvider {
    pub fn new() -> Option<NvmlProvider> {
        match Nvml::init() {
            Ok(nvml) => {
                tracing::info!("NVML loaded");
                Some(NvmlProvider { nvml })
            }
            Err(e) => {
                tracing::info!("NVML not available ({e}); GPU stats fall back to PDH");
                None
            }
        }
    }

    pub fn sample(&self, gpu: &GpuInfo) -> Option<GpuSample> {
        let dev = self.nvml.device_by_uuid(gpu.nvml_uuid()).ok().or_else(|| {
            // Fall back to name matching (e.g. when UUIDs differ).
            let n = self.nvml.device_count().ok()?;
            (0..n)
                .filter_map(|i| self.nvml.device_by_index(i).ok())
                .find(|d| d.name().map(|nm| nm == gpu.name).unwrap_or(false))
        })?;
        let util = dev.utilization_rates().ok()?;
        let mem = dev.memory_info().ok()?;
        Some(GpuSample {
            util: util.gpu as f32,
            vram_used: mem.used,
            vram_total: mem.total,
            provider: "NVML",
        })
    }
}
