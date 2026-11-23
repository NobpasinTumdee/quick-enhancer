//! ONNX Runtime loading and execution-provider choice.
//!
//! * One `onnxruntime.dll` per process: the DirectML build (any DX12 GPU) or the CUDA build
//!   (NVIDIA), downloaded by the Model Manager into `<base>/runtime/{dml,cuda}/`. The user's
//!   preference is used when that runtime is installed, otherwise the other one.
//! * Execution providers are tried in order (GPU → CPU). A failure falls back to the next one
//!   and leaves a notice for the UI.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::devices::GpuInfo;
use crate::models::RuntimeKind;
use crate::models::fit::NVIDIA_VENDOR;

/// Execution provider for one session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ep {
    /// DirectML on a DXGI adapter index.
    DirectMl(i32),
    /// CUDA on a CUDA device index.
    Cuda(i32),
    Cpu,
}

impl Ep {
    pub fn label(self) -> String {
        match self {
            Ep::DirectMl(i) => format!("DirectML #{i}"),
            Ep::Cuda(i) => format!("CUDA #{i}"),
            Ep::Cpu => "CPU".into(),
        }
    }
    pub fn is_gpu(self) -> bool {
        self != Ep::Cpu
    }
}

struct Config {
    base: PathBuf,
    pref: RuntimeKind,
    gpus: Vec<GpuInfo>,
}

static CONFIG: Mutex<Option<Config>> = Mutex::new(None);
static LOADED: OnceLock<Result<Loaded, String>> = OnceLock::new();
static NOTICES: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub struct Loaded {
    pub env: ort::environment::Environment,
    pub kind: RuntimeKind,
    pub dir: PathBuf,
}

/// Where the runtimes live, the preferred one and the GPUs (Vulkan order). Call at startup
/// and after installs; the runtime itself is loaded on first use (once per process).
pub fn configure(base: &Path, pref: RuntimeKind, gpus: &[GpuInfo]) {
    if let Ok(mut c) = CONFIG.lock() {
        *c = Some(Config {
            base: base.to_path_buf(),
            pref,
            gpus: gpus.to_vec(),
        });
    }
}

/// Messages for the user (EP fallbacks, runtime switches). Drained by the UI / job log.
pub fn take_notices() -> Vec<String> {
    NOTICES.lock().map(|mut n| std::mem::take(&mut *n)).unwrap_or_default()
}

pub fn notice(msg: String) {
    tracing::warn!("{msg}");
    if let Ok(mut n) = NOTICES.lock()
        && !n.contains(&msg)
    {
        n.push(msg);
    }
}

pub fn runtime_installed(base: &Path, kind: RuntimeKind) -> bool {
    kind.dir(base).join("onnxruntime.dll").is_file() || kind.dir(base).join("libonnxruntime.so").is_file()
}

/// Which runtime to load: the preferred one if installed, else the other.
pub fn choose_kind(pref: RuntimeKind, dml: bool, cuda: bool) -> Option<RuntimeKind> {
    let have = |k| match k {
        RuntimeKind::DirectMl => dml,
        RuntimeKind::Cuda => cuda,
    };
    let other = match pref {
        RuntimeKind::DirectMl => RuntimeKind::Cuda,
        RuntimeKind::Cuda => RuntimeKind::DirectMl,
    };
    [pref, other].into_iter().find(|&k| have(k))
}

/// The loaded runtime (loads it on first call).
pub fn environment() -> Result<&'static Loaded, String> {
    LOADED.get_or_init(load).as_ref().map_err(Clone::clone)
}

/// Kind of the runtime loaded in this process, if any.
pub fn loaded_kind() -> Option<RuntimeKind> {
    LOADED.get().and_then(|r| r.as_ref().ok()).map(|l| l.kind)
}

fn load() -> Result<Loaded, String> {
    let (base, pref) = {
        let c = CONFIG.lock().map_err(|e| e.to_string())?;
        let c = c.as_ref().ok_or("ONNX runtime not configured")?;
        (c.base.clone(), c.pref)
    };
    let kind = choose_kind(
        pref,
        runtime_installed(&base, RuntimeKind::DirectMl),
        runtime_installed(&base, RuntimeKind::Cuda),
    )
    .ok_or_else(|| crate::t!("engine.onnx_unavailable"))?;
    if kind != pref {
        notice(crate::t!("onnx.runtime_other", kind = kind_label(kind)));
    }
    let dir = kind.dir(&base);
    prepare_dll_search(&dir, kind);
    let dll = dir.join(if cfg!(windows) { "onnxruntime.dll" } else { "libonnxruntime.so" });
    let env = ort::init_from(&dll)
        .map_err(|e| crate::t!("onnx.load_failed", error = explain_load_error(&e.to_string())))?
        .with_name("quick-enhancer")
        .build()
        .map_err(|e| crate::t!("onnx.load_failed", error = e))?;
    tracing::info!("ONNX Runtime loaded: {} ({})", dll.display(), kind_label(kind));
    Ok(Loaded { env, kind, dir })
}

pub fn kind_label(k: RuntimeKind) -> &'static str {
    match k {
        RuntimeKind::DirectMl => "DirectML",
        RuntimeKind::Cuda => "CUDA",
    }
}

/// A missing Visual C++ runtime is the most common load failure on clean Windows installs.
fn explain_load_error(e: &str) -> String {
    let l = e.to_ascii_lowercase();
    if l.contains("126") || l.contains("module could not be found") || l.contains("msvcp140") {
        format!("{e}. {}", crate::t!("onnx.vcredist_hint"))
    } else {
        e.to_string()
    }
}

/// Let Windows find the runtime's dependencies (DirectML.dll, provider DLLs, CUDA / cuDNN)
/// in the runtime folder, and load ours before the older copy in System32.
fn prepare_dll_search(dir: &Path, kind: RuntimeKind) {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::LibraryLoader::{AddDllDirectory, LoadLibraryW, SetDllDirectoryW};
        use windows::core::HSTRING;
        let h = HSTRING::from(dir.as_os_str());
        if let Err(e) = SetDllDirectoryW(&h) {
            tracing::warn!("SetDllDirectoryW: {e}");
        }
        let _ = AddDllDirectory(&h);
        // The app-local Visual C++ runtime first, so onnxruntime.dll binds to it even on PCs
        // without the VC++ Redistributable.
        let vc: &[&str] = &["vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll"];
        let preload: &[&str] = match kind {
            RuntimeKind::DirectMl => &["DirectML.dll"],
            RuntimeKind::Cuda => &["cudart64_13.dll", "cublasLt64_13.dll", "cublas64_13.dll", "cudnn64_9.dll"],
        };
        for name in vc.iter().chain(preload) {
            let p = dir.join(name);
            if p.is_file()
                && let Err(e) = LoadLibraryW(&HSTRING::from(p.as_os_str()))
            {
                tracing::warn!("preloading {}: {e}", p.display());
            }
        }
        tracing::info!("C++ runtime in use: {}", vc_runtime_in_use().unwrap_or_else(|| "none".into()));
    }
    #[cfg(not(windows))]
    {
        let _ = (dir, kind);
    }
}

/// Full path of the `msvcp140.dll` loaded in this process (ours or the system's), if any.
#[cfg(windows)]
pub fn vc_runtime_in_use() -> Option<String> {
    use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
    unsafe {
        let m = GetModuleHandleW(windows::core::w!("msvcp140.dll")).ok()?;
        let mut buf = [0u16; 1024];
        let n = GetModuleFileNameW(Some(m), &mut buf) as usize;
        (n > 0).then(|| String::from_utf16_lossy(&buf[..n]))
    }
}

/// Execution providers to try for `gpu` (Vulkan index; `None` = CPU only), best first.
pub fn ep_chain(kind: RuntimeKind, gpu: Option<&GpuInfo>, gpus: &[GpuInfo]) -> Vec<Ep> {
    let Some(g) = gpu else { return vec![Ep::Cpu] };
    match kind {
        RuntimeKind::DirectMl => {
            let idx = g.luid.and_then(dxgi_index_for_luid).unwrap_or(0);
            vec![Ep::DirectMl(idx as i32), Ep::Cpu]
        }
        RuntimeKind::Cuda if g.vendor_id == NVIDIA_VENDOR => {
            // CUDA numbers NVIDIA GPUs only; keep their relative (PCI) order from Vulkan.
            let idx = gpus
                .iter()
                .filter(|o| o.vendor_id == NVIDIA_VENDOR)
                .position(|o| o.index == g.index)
                .unwrap_or(0);
            vec![Ep::Cuda(idx as i32), Ep::Cpu]
        }
        RuntimeKind::Cuda => {
            notice(crate::t!("onnx.cuda_not_nvidia", gpu = &g.name));
            vec![Ep::Cpu]
        }
    }
}

/// GPU info for a Vulkan index from the configured list.
pub fn gpu_info(vulkan_index: Option<u32>) -> Option<GpuInfo> {
    let i = vulkan_index?;
    let c = CONFIG.lock().ok()?;
    c.as_ref()?.gpus.iter().find(|g| g.index == i).cloned()
}

pub fn gpus() -> Vec<GpuInfo> {
    CONFIG
        .lock()
        .ok()
        .and_then(|c| c.as_ref().map(|c| c.gpus.clone()))
        .unwrap_or_default()
}

/// DXGI adapter index (DirectML device id) of the adapter with this LUID.
#[cfg(windows)]
pub fn dxgi_index_for_luid(luid: [u8; 8]) -> Option<u32> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    let low = u32::from_le_bytes([luid[0], luid[1], luid[2], luid[3]]);
    let high = i32::from_le_bytes([luid[4], luid[5], luid[6], luid[7]]);
    unsafe {
        let f: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let mut i = 0;
        while let Ok(a) = f.EnumAdapters1(i) {
            if let Ok(d) = a.GetDesc1()
                && d.AdapterLuid.LowPart == low
                && d.AdapterLuid.HighPart == high
            {
                return Some(i);
            }
            i += 1;
        }
    }
    None
}

#[cfg(not(windows))]
pub fn dxgi_index_for_luid(_luid: [u8; 8]) -> Option<u32> {
    None
}

/// Does an ONNX Runtime error mean "out of (video) memory"?
pub fn is_oom(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("out of memory")
        || m.contains("outofmemory")
        || m.contains("e_outofmemory")
        || m.contains("8007000e")
        || m.contains("887a0005") // DXGI_ERROR_DEVICE_REMOVED (often a VRAM exhaustion / TDR)
        || m.contains("887a0006") // DXGI_ERROR_DEVICE_HUNG
        || m.contains("bad allocation")
        || m.contains("bad_alloc")
        || m.contains("cudnn_status_alloc_failed")
        || m.contains("cuda_error_out_of_memory")
        || m.contains("failed to allocate memory")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::GpuKind;

    fn gpu(index: u32, vendor: u32) -> GpuInfo {
        GpuInfo {
            index,
            name: format!("gpu{index}"),
            vendor_id: vendor,
            kind: GpuKind::Discrete,
            vram_bytes: 8 << 30,
            uuid: [0; 16],
            luid: None,
            fp16: true,
        }
    }

    #[test]
    fn runtime_choice_prefers_then_falls_back() {
        use RuntimeKind::*;
        assert_eq!(choose_kind(DirectMl, true, true), Some(DirectMl));
        assert_eq!(choose_kind(Cuda, true, true), Some(Cuda));
        assert_eq!(choose_kind(Cuda, true, false), Some(DirectMl));
        assert_eq!(choose_kind(DirectMl, false, true), Some(Cuda));
        assert_eq!(choose_kind(DirectMl, false, false), None);
    }

    #[test]
    fn ep_fallback_order() {
        let amd = gpu(0, 0x1002);
        let nv = gpu(1, NVIDIA_VENDOR);
        let all = vec![amd.clone(), nv.clone()];
        // No LUID → adapter 0.
        assert_eq!(ep_chain(RuntimeKind::DirectMl, Some(&amd), &all), vec![Ep::DirectMl(0), Ep::Cpu]);
        assert_eq!(ep_chain(RuntimeKind::DirectMl, None, &all), vec![Ep::Cpu]);
        // CUDA counts NVIDIA cards only.
        assert_eq!(ep_chain(RuntimeKind::Cuda, Some(&nv), &all), vec![Ep::Cuda(0), Ep::Cpu]);
        // CUDA runtime on a non-NVIDIA GPU → CPU (with a notice).
        assert_eq!(ep_chain(RuntimeKind::Cuda, Some(&amd), &all), vec![Ep::Cpu]);
        assert!(take_notices().iter().any(|n| n.contains("gpu0")));
        assert!(Ep::DirectMl(0).is_gpu() && !Ep::Cpu.is_gpu());
    }

    #[test]
    fn oom_messages() {
        assert!(is_oom("Non-zero status code returned while running Conv node. 887A0005"));
        assert!(is_oom("CUDA failure 2: out of memory"));
        assert!(is_oom("bad allocation"));
        assert!(!is_oom("Invalid input name: foo"));
    }
}
