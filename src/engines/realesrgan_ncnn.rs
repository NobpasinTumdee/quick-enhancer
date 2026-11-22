//! `realesrgan-ncnn-vulkan` CLI backend (directory mode).

use std::path::{Path, PathBuf};

use super::{EngineCtx, EngineError, Upscaler, count_files};
use crate::util::{ProcError, new_command, run_capture_tick};

pub struct RealEsrganNcnn {
    exe: PathBuf,
    models_dir: PathBuf,
    /// `-n` model name (e.g. `realesrgan-x4plus`, `realesr-animevideov3`).
    model: String,
    scales: Vec<u32>,
}

impl RealEsrganNcnn {
    pub fn new(exe: PathBuf, models_dir: PathBuf, model: String, scales: Vec<u32>) -> Self {
        Self {
            exe,
            models_dir,
            model,
            scales,
        }
    }
}

/// stderr patterns that indicate a VRAM / device problem (→ retry with smaller tiles).
pub fn looks_like_oom(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("vkallocatememory")
        || s.contains("out of memory")
        || s.contains("out_of_device_memory")
        || s.contains("vkqueuesubmit failed")
        || s.contains("vk_error")
}

impl Upscaler for RealEsrganNcnn {
    fn name(&self) -> String {
        format!("Real-ESRGAN ncnn ({})", self.model)
    }

    fn native_scales(&self) -> Vec<u32> {
        self.scales.clone()
    }

    fn supports_cpu(&self) -> bool {
        // The official realesrgan-ncnn-vulkan builds reject `-g -1` ("invalid gpu device").
        false
    }

    fn upscale_dir(
        &self,
        input: &Path,
        output: &Path,
        scale: u32,
        ctx: &EngineCtx,
    ) -> Result<(), EngineError> {
        if ctx.cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        std::fs::create_dir_all(output)?;
        let expected = count_files(input);
        let mut cmd = new_command(&self.exe);
        cmd.arg("-i").arg(input);
        cmd.arg("-o").arg(output);
        cmd.args(["-n", &self.model, "-s", &scale.to_string()]);
        cmd.args(["-t", &ctx.tile.to_string()]);
        cmd.args(["-g", &ctx.gpu_arg()]);
        cmd.args(["-j", &ctx.jobs]);
        cmd.args(["-f", "png"]);
        cmd.arg("-m").arg(&self.models_dir);
        // ncnn tools resolve relative resources against the cwd.
        if let Some(dir) = self.exe.parent() {
            cmd.current_dir(dir);
        }
        let mut tick = || {
            if let Some(p) = ctx.progress {
                p(count_files(output));
            }
        };
        let out = run_capture_tick(cmd, Some(ctx.cancel), ctx.priority, &mut tick).map_err(
            |e| match e {
                ProcError::Cancelled => EngineError::Cancelled,
                e => EngineError::Proc(e),
            },
        )?;
        let got = count_files(output);
        if got < expected {
            return Err(EngineError::MissingOutput {
                engine: "realesrgan-ncnn-vulkan".into(),
                got,
                expected,
                tail: out.stderr_tail,
            });
        }
        if looks_like_oom(&out.stderr_tail) {
            // Output files exist but ncnn reported memory trouble — they may be black/garbage.
            return Err(EngineError::MissingOutput {
                engine: "realesrgan-ncnn-vulkan (memory error)".into(),
                got,
                expected,
                tail: out.stderr_tail,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn oom_patterns() {
        assert!(super::looks_like_oom("vkAllocateMemory failed -2"));
        assert!(!super::looks_like_oom("0.00%\n100.00%"));
    }
}
