//! `realcugan-ncnn-vulkan` CLI backend (directory mode). Real-CUGAN (bilibili) is an anime /
//! line-art model; the ncnn port takes the model folder (`models-se`, `models-pro`) and picks
//! the `up{scale}x-<denoise>` file itself.

use std::path::{Path, PathBuf};

use super::realesrgan_ncnn::looks_like_oom;
use super::{EngineCtx, EngineError, Upscaler, count_files};
use crate::util::{ProcError, new_command, run_capture_tick};

pub struct RealCuganNcnn {
    exe: PathBuf,
    /// `models/realcugan/models-se` (or `models-pro`).
    model_dir: PathBuf,
    scales: Vec<u32>,
    /// `-n`: -1 conservative, 0 no denoise, 1..3 denoise strength (not every level exists
    /// for every scale; see [`RealCuganNcnn::noise_for`]).
    noise: i32,
}

impl RealCuganNcnn {
    pub fn new(exe: PathBuf, model_dir: PathBuf, scales: Vec<u32>, noise: i32) -> Self {
        Self {
            exe,
            model_dir,
            scales,
            noise,
        }
    }

    /// Closest noise level that has a model file for `scale` (the CLI fails otherwise).
    pub fn noise_for(&self, scale: u32) -> i32 {
        let has = |n: i32| {
            let name = match n {
                -1 => "conservative".to_string(),
                0 => "no-denoise".to_string(),
                n => format!("denoise{n}x"),
            };
            self.model_dir.join(format!("up{scale}x-{name}.param")).is_file()
        };
        if has(self.noise) {
            return self.noise;
        }
        // Stronger denoise first (3x exists for every scale), then conservative.
        [3, 2, 1, -1, 0]
            .into_iter()
            .filter(|&n| (self.noise > 0) == (n > 0))
            .chain([-1, 0, 3])
            .find(|&n| has(n))
            .unwrap_or(-1)
    }
}

impl Upscaler for RealCuganNcnn {
    fn name(&self) -> String {
        let variant = self
            .model_dir
            .file_name()
            .map(|n| n.to_string_lossy().trim_start_matches("models-").to_string())
            .unwrap_or_default();
        format!("Real-CUGAN ncnn ({variant})")
    }

    fn native_scales(&self) -> Vec<u32> {
        self.scales.clone()
    }

    fn supports_cpu(&self) -> bool {
        true
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
        if !self.scales.contains(&scale) {
            return Err(EngineError::Unavailable(format!(
                "Real-CUGAN {} has no {scale}x model",
                self.name()
            )));
        }
        std::fs::create_dir_all(output)?;
        let expected = count_files(input);
        let mut cmd = new_command(&self.exe);
        cmd.arg("-i").arg(input);
        cmd.arg("-o").arg(output);
        cmd.args(["-s", &scale.to_string()]);
        cmd.args(["-n", &self.noise_for(scale).to_string()]);
        cmd.args(["-t", &ctx.tile.to_string()]);
        cmd.args(["-g", &ctx.gpu_arg()]);
        cmd.args(["-j", &ctx.jobs]);
        cmd.args(["-f", "png"]);
        cmd.arg("-m").arg(&self.model_dir);
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
        if got < expected || looks_like_oom(&out.stderr_tail) {
            return Err(EngineError::MissingOutput {
                engine: "realcugan-ncnn-vulkan".into(),
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
    use super::*;

    #[test]
    fn noise_falls_back_to_existing_files() {
        let d = std::env::temp_dir().join(format!("qe-cugan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for f in ["up2x-conservative", "up2x-denoise1x", "up2x-denoise3x", "up4x-conservative", "up4x-denoise3x"] {
            std::fs::write(d.join(format!("{f}.param")), b"").unwrap();
        }
        let e = |n| RealCuganNcnn::new(PathBuf::from("x"), d.clone(), vec![2, 4], n);
        assert_eq!(e(-1).noise_for(2), -1);
        assert_eq!(e(1).noise_for(2), 1);
        assert_eq!(e(1).noise_for(4), 3, "denoise1x missing at 4x → strongest available");
        assert_eq!(e(0).noise_for(4), -1, "no-denoise missing → conservative");
        let _ = std::fs::remove_dir_all(&d);
    }
}
