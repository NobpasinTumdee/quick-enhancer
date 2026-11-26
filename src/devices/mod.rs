//! Vulkan GPU enumeration (via `ash`, loaded at runtime) and `Auto` device selection.
//!
//! The order of `vkEnumeratePhysicalDevices` is the order ncnn uses for `-g <index>`.

use ash::vk;

use crate::config::ComputeDevice;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GpuKind {
    Discrete,
    Integrated,
    Virtual,
    Cpu,
    Other,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GpuInfo {
    /// ncnn / Vulkan device index.
    pub index: u32,
    pub name: String,
    pub vendor_id: u32,
    pub kind: GpuKind,
    /// Largest DEVICE_LOCAL heap.
    pub vram_bytes: u64,
    pub uuid: [u8; 16],
    pub luid: Option<[u8; 8]>,
    /// Shader float16 supported (ncnn will pick FP16 automatically).
    pub fp16: bool,
}

impl GpuInfo {
    pub fn vendor(&self) -> &'static str {
        match self.vendor_id {
            0x10DE => "NVIDIA",
            0x1002 | 0x1022 => "AMD",
            0x8086 => "Intel",
            0x13B5 => "ARM",
            0x5143 => "Qualcomm",
            _ => "Unknown",
        }
    }

    pub fn is_nvidia(&self) -> bool {
        self.vendor_id == 0x10DE
    }

    /// UUID formatted the way NVML prints it: `GPU-xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`.
    pub fn nvml_uuid(&self) -> String {
        let h: Vec<String> = self.uuid.iter().map(|b| format!("{b:02x}")).collect();
        format!(
            "GPU-{}-{}-{}-{}-{}",
            h[0..4].concat(),
            h[4..6].concat(),
            h[6..8].concat(),
            h[8..10].concat(),
            h[10..16].concat()
        )
    }

    /// LUID formatted as in PDH instance names: `luid_0xHHHHHHHH_0xLLLLLLLL`.
    pub fn pdh_luid(&self) -> Option<String> {
        let l = self.luid?;
        let low = u32::from_le_bytes([l[0], l[1], l[2], l[3]]);
        let high = u32::from_le_bytes([l[4], l[5], l[6], l[7]]);
        Some(format!("luid_0x{high:08x}_0x{low:08x}"))
    }

    pub fn label(&self) -> String {
        format!(
            "[{}] {} ({:.1} GB)",
            self.index,
            self.name,
            self.vram_bytes as f64 / (1u64 << 30) as f64
        )
    }
}

/// Enumerate Vulkan physical devices. Returns `Err` with a readable reason if Vulkan
/// is unavailable (the app then runs CPU-only).
pub fn enumerate() -> Result<Vec<GpuInfo>, String> {
    // SAFETY: loading the Vulkan loader library; failure is handled.
    let entry =
        unsafe { ash::Entry::load() }.map_err(|e| format!("Vulkan loader not found: {e}"))?;
    let app = vk::ApplicationInfo::default()
        .application_name(c"quick-enhancer")
        .api_version(vk::make_api_version(0, 1, 1, 0));
    let ci = vk::InstanceCreateInfo::default().application_info(&app);
    // SAFETY: valid create info; instance destroyed below.
    let instance = unsafe { entry.create_instance(&ci, None) }
        .map_err(|e| format!("vkCreateInstance failed: {e}"))?;
    let result = (|| {
        // SAFETY: instance is valid.
        let pds = unsafe { instance.enumerate_physical_devices() }
            .map_err(|e| format!("vkEnumeratePhysicalDevices failed: {e}"))?;
        let mut out = Vec::new();
        for (i, pd) in pds.into_iter().enumerate() {
            // SAFETY: pd comes from this instance.
            let props = unsafe { instance.get_physical_device_properties(pd) };
            let mut id = vk::PhysicalDeviceIDProperties::default();
            let api11 = props.api_version >= vk::make_api_version(0, 1, 1, 0);
            if api11 {
                let mut p2 = vk::PhysicalDeviceProperties2::default().push_next(&mut id);
                // SAFETY: Vulkan 1.1 core entry point, device supports 1.1.
                unsafe { instance.get_physical_device_properties2(pd, &mut p2) };
            }
            let mut f16 = vk::PhysicalDeviceShaderFloat16Int8Features::default();
            let fp16 = if props.api_version >= vk::make_api_version(0, 1, 2, 0) {
                let mut f2 = vk::PhysicalDeviceFeatures2::default().push_next(&mut f16);
                // SAFETY: core 1.1 entry point; struct is core in 1.2.
                unsafe { instance.get_physical_device_features2(pd, &mut f2) };
                f16.shader_float16 == vk::TRUE
            } else {
                false
            };
            // SAFETY: pd valid.
            let mem = unsafe { instance.get_physical_device_memory_properties(pd) };
            let vram_bytes = mem.memory_heaps[..mem.memory_heap_count as usize]
                .iter()
                .filter(|h| h.flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL))
                .map(|h| h.size)
                .max()
                .unwrap_or(0);
            let name = props
                .device_name_as_c_str()
                .map(|c| c.to_string_lossy().to_string())
                .unwrap_or_else(|_| format!("GPU {i}"));
            let kind = match props.device_type {
                vk::PhysicalDeviceType::DISCRETE_GPU => GpuKind::Discrete,
                vk::PhysicalDeviceType::INTEGRATED_GPU => GpuKind::Integrated,
                vk::PhysicalDeviceType::VIRTUAL_GPU => GpuKind::Virtual,
                vk::PhysicalDeviceType::CPU => GpuKind::Cpu,
                _ => GpuKind::Other,
            };
            out.push(GpuInfo {
                index: i as u32,
                name,
                vendor_id: props.vendor_id,
                kind,
                vram_bytes,
                uuid: id.device_uuid,
                luid: (api11 && id.device_luid_valid == vk::TRUE).then_some(id.device_luid),
                fp16,
            });
        }
        Ok(out)
    })();
    // SAFETY: no child objects were created.
    unsafe { instance.destroy_instance(None) };
    result
}

/// `Auto` = discrete GPU with the most VRAM, else integrated, else CPU (`None`).
pub fn auto_select(gpus: &[GpuInfo]) -> Option<&GpuInfo> {
    let best = |kind: GpuKind| {
        gpus.iter()
            .filter(|g| g.kind == kind)
            .max_by_key(|g| g.vram_bytes)
    };
    best(GpuKind::Discrete)
        .or_else(|| best(GpuKind::Integrated))
        .or_else(|| best(GpuKind::Virtual))
}

/// Resolve a compute setting to a GPU (or `None` for CPU).
pub fn resolve(setting: ComputeDevice, gpus: &[GpuInfo]) -> Option<&GpuInfo> {
    match setting {
        ComputeDevice::Auto => auto_select(gpus),
        ComputeDevice::Gpu(i) => gpus
            .iter()
            .find(|g| g.index == i)
            .or_else(|| auto_select(gpus)),
        ComputeDevice::Cpu => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu(index: u32, kind: GpuKind, gb: u64) -> GpuInfo {
        GpuInfo {
            index,
            name: format!("g{index}"),
            vendor_id: 0x10DE,
            kind,
            vram_bytes: gb << 30,
            uuid: [0; 16],
            luid: None,
            fp16: true,
        }
    }

    #[test]
    fn auto_prefers_discrete_with_most_vram() {
        let g = vec![
            gpu(0, GpuKind::Integrated, 2),
            gpu(1, GpuKind::Discrete, 8),
            gpu(2, GpuKind::Discrete, 16),
        ];
        assert_eq!(auto_select(&g).unwrap().index, 2);
        let g = vec![gpu(0, GpuKind::Integrated, 2), gpu(1, GpuKind::Cpu, 0)];
        assert_eq!(auto_select(&g).unwrap().index, 0);
        let g = vec![gpu(0, GpuKind::Cpu, 0)];
        assert!(auto_select(&g).is_none());
        assert!(resolve(ComputeDevice::Cpu, &g).is_none());
    }

    #[test]
    fn id_formatting() {
        let mut g = gpu(0, GpuKind::Discrete, 16);
        g.uuid = [
            0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab,
            0xcd, 0xef,
        ];
        assert_eq!(g.nvml_uuid(), "GPU-12345678-9abc-def0-0123-456789abcdef");
        g.luid = Some([0xb5, 0xd1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(g.pdh_luid().unwrap(), "luid_0x00000000_0x0000d1b5");
    }
}
