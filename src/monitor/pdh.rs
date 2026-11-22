//! Vendor-neutral GPU telemetry via Windows Performance Data Helper counters:
//! `\GPU Engine(*)\Utilization Percentage` and `\GPU Adapter Memory(*)\Dedicated Usage`.

use std::collections::HashMap;

use windows::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhOpenQueryW,
};
use windows::core::{PCWSTR, w};

use super::GpuSample;
use crate::devices::GpuInfo;

pub struct PdhProvider {
    query: PDH_HQUERY,
    engine: PDH_HCOUNTER,
    memory: PDH_HCOUNTER,
}

impl PdhProvider {
    pub fn new() -> Option<PdhProvider> {
        let mut query = PDH_HQUERY::default();
        // SAFETY: plain PDH calls with valid out-pointers; the query is closed in Drop.
        unsafe {
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                return None;
            }
            let mut p = PdhProvider {
                query,
                engine: PDH_HCOUNTER::default(),
                memory: PDH_HCOUNTER::default(),
            };
            if PdhAddEnglishCounterW(
                query,
                w!("\\GPU Engine(*)\\Utilization Percentage"),
                0,
                &mut p.engine,
            ) != 0
            {
                return None; // p dropped → query closed
            }
            if PdhAddEnglishCounterW(
                query,
                w!("\\GPU Adapter Memory(*)\\Dedicated Usage"),
                0,
                &mut p.memory,
            ) != 0
            {
                return None;
            }
            let _ = PdhCollectQueryData(query);
            Some(p)
        }
    }

    fn read_array(counter: PDH_HCOUNTER) -> Vec<(String, f64)> {
        let mut size: u32 = 0;
        let mut count: u32 = 0;
        // SAFETY: first call sizes the buffer, second fills it; the buffer is u64-aligned.
        unsafe {
            let r =
                PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, None);
            if r != PDH_MORE_DATA || size == 0 {
                return Vec::new();
            }
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            if PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                Some(items),
            ) != 0
            {
                return Vec::new();
            }
            let slice = std::slice::from_raw_parts(items, count as usize);
            slice
                .iter()
                .filter(|it| it.FmtValue.CStatus == 0)
                .map(|it| {
                    let name = it
                        .szName
                        .to_string()
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    (name, it.FmtValue.Anonymous.doubleValue)
                })
                .collect()
        }
    }

    pub fn sample(&mut self, gpu: &GpuInfo) -> Option<GpuSample> {
        let luid = gpu.pdh_luid()?;
        // SAFETY: valid query handle.
        if unsafe { PdhCollectQueryData(self.query) } != 0 {
            return None;
        }
        // GPU % = max over engine types of the summed per-process utilisation (like Task Manager).
        let mut per_type: HashMap<String, f64> = HashMap::new();
        for (name, v) in Self::read_array(self.engine) {
            if !name.contains(&luid) {
                continue;
            }
            let ty = name.split("engtype_").nth(1).unwrap_or("other").to_string();
            *per_type.entry(ty).or_default() += v;
        }
        let util = per_type.values().cloned().fold(0.0, f64::max).min(100.0);
        let vram_used: f64 = Self::read_array(self.memory)
            .into_iter()
            .filter(|(n, _)| n.contains(&luid))
            .map(|(_, v)| v)
            .sum();
        Some(GpuSample {
            util: util as f32,
            vram_used: vram_used as u64,
            vram_total: gpu.vram_bytes,
            provider: "PDH",
        })
    }
}

impl Drop for PdhProvider {
    fn drop(&mut self) {
        // SAFETY: query opened in `new`.
        unsafe {
            let _ = PdhCloseQuery(self.query);
        }
    }
}
