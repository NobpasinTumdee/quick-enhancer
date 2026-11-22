//! Real-time resource monitor running on its own thread (~2 Hz).
//!
//! GPU provider order: NVML (NVIDIA) → PDH (any vendor, Windows) → not available.

mod nvml;
#[cfg(windows)]
mod pdh;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::devices::GpuInfo;

pub const HISTORY: usize = 120; // 60 s at 2 Hz
const INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Default)]
pub struct GpuSample {
    pub util: f32,
    pub vram_used: u64,
    pub vram_total: u64,
    pub provider: &'static str,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub cpu: f32,
    pub ram_used: u64,
    pub ram_total: u64,
    pub gpu: Option<GpuSample>,
    pub gpu_name: Option<String>,
    pub cpu_hist: VecDeque<f32>,
    pub ram_hist: VecDeque<f32>,
    pub gpu_hist: VecDeque<f32>,
    pub vram_hist: VecDeque<f32>,
}

impl Snapshot {
    pub fn free_vram(&self) -> Option<u64> {
        self.gpu
            .as_ref()
            .filter(|g| g.vram_total > 0)
            .map(|g| g.vram_total.saturating_sub(g.vram_used))
    }
}

fn push(h: &mut VecDeque<f32>, v: f32) {
    if h.len() >= HISTORY {
        h.pop_front();
    }
    h.push_back(v);
}

/// Handle shared with the UI and the pipeline.
#[derive(Clone)]
pub struct Monitor {
    pub snapshot: Arc<Mutex<Snapshot>>,
    active_gpu: Arc<Mutex<Option<GpuInfo>>>,
}

impl Monitor {
    pub fn start(repaint: Option<egui::Context>) -> Monitor {
        let m = Monitor {
            snapshot: Arc::new(Mutex::new(Snapshot::default())),
            active_gpu: Arc::new(Mutex::new(None)),
        };
        let m2 = m.clone();
        let _ = std::thread::Builder::new()
            .name("monitor".into())
            .spawn(move || m2.run(repaint));
        m
    }

    pub fn set_active_gpu(&self, gpu: Option<GpuInfo>) {
        if let Ok(mut g) = self.active_gpu.lock() {
            *g = gpu;
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn free_vram(&self) -> Option<u64> {
        self.snapshot.lock().ok()?.free_vram()
    }

    fn run(self, repaint: Option<egui::Context>) {
        use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
        let mut sys = match System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::nothing().with_usage())
                .with_memory(MemoryRefreshKind::nothing().with_ram()),
        ) {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!("sysinfo unavailable: {e}");
                None
            }
        };
        let nvml = nvml::NvmlProvider::new();
        #[cfg(windows)]
        let mut pdh = pdh::PdhProvider::new();
        let mut tick: u64 = 0;
        loop {
            std::thread::sleep(INTERVAL);
            tick += 1;
            let (cpu, ram_used, ram_total) = match sys.as_mut() {
                Some(s) => {
                    s.refresh_cpu_usage();
                    s.refresh_memory();
                    (s.global_cpu_usage(), s.used_memory(), s.total_memory())
                }
                None => (0.0, 0, 0),
            };
            let active = self.active_gpu.lock().ok().and_then(|g| g.clone());
            let mut gpu = None;
            if let Some(a) = &active {
                if a.is_nvidia() {
                    gpu = nvml.as_ref().and_then(|n| n.sample(a));
                }
                #[cfg(windows)]
                if gpu.is_none() {
                    // Re-create the PDH query periodically so new per-process engine
                    // instances are picked up.
                    if tick.is_multiple_of(20) {
                        pdh = pdh::PdhProvider::new();
                    }
                    gpu = pdh.as_mut().and_then(|p| p.sample(a));
                }
            }
            if let Ok(mut s) = self.snapshot.lock() {
                s.cpu = cpu;
                s.ram_used = ram_used;
                s.ram_total = ram_total;
                s.gpu_name = active.as_ref().map(|a| a.name.clone());
                push(&mut s.cpu_hist, cpu);
                push(
                    &mut s.ram_hist,
                    if ram_total > 0 {
                        ram_used as f32 / ram_total as f32 * 100.0
                    } else {
                        0.0
                    },
                );
                let (gu, vu) = gpu
                    .as_ref()
                    .map(|g| {
                        (
                            g.util,
                            if g.vram_total > 0 {
                                g.vram_used as f32 / g.vram_total as f32 * 100.0
                            } else {
                                0.0
                            },
                        )
                    })
                    .unwrap_or((0.0, 0.0));
                push(&mut s.gpu_hist, gu);
                push(&mut s.vram_hist, vu);
                s.gpu = gpu;
            }
            if let Some(ctx) = &repaint {
                ctx.request_repaint();
            }
        }
    }
}
