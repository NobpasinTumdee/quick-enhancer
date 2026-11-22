//! `rife-ncnn-vulkan` CLI backend (directory mode with explicit target frame count).

use std::path::{Path, PathBuf};

use super::{EngineCtx, EngineError, Interpolator, count_files};
use crate::util::{ProcError, new_command, run_capture_tick};

pub struct RifeNcnn {
    exe: PathBuf,
    model_dir: PathBuf,
    model_name: String,
    arbitrary_count: bool,
}

impl RifeNcnn {
    pub fn new(
        exe: PathBuf,
        model_dir: PathBuf,
        model_name: String,
        arbitrary_count: bool,
    ) -> Self {
        Self {
            exe,
            model_dir,
            model_name,
            arbitrary_count,
        }
    }
}

impl Interpolator for RifeNcnn {
    fn name(&self) -> String {
        format!("RIFE ncnn ({})", self.model_name)
    }

    fn supports_multiplier(&self, k: u32) -> bool {
        k == 2 || (self.arbitrary_count && k >= 2)
    }

    fn interpolate_dir(
        &self,
        input: &Path,
        output: &Path,
        out_count: usize,
        uhd: bool,
        ctx: &EngineCtx,
    ) -> Result<(), EngineError> {
        if ctx.cancel.is_cancelled() {
            return Err(EngineError::Cancelled);
        }
        std::fs::create_dir_all(output)?;
        let mut cmd = new_command(&self.exe);
        cmd.arg("-i").arg(input);
        cmd.arg("-o").arg(output);
        cmd.args(["-n", &out_count.to_string()]);
        cmd.arg("-m").arg(&self.model_dir);
        cmd.args(["-g", &ctx.gpu_arg()]);
        cmd.args(["-j", &ctx.jobs]);
        cmd.args(["-f", "%08d.png"]);
        if uhd {
            cmd.arg("-u");
        }
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
        if got < out_count {
            return Err(EngineError::MissingOutput {
                engine: "rife-ncnn-vulkan".into(),
                got,
                expected: out_count,
                tail: out.stderr_tail,
            });
        }
        Ok(())
    }
}
