//! Placeholder for an in-process ONNX Runtime backend (DirectML / CUDA execution providers).
//!
//! Implementing it means: load an `.onnx` super-resolution model with the `ort` crate,
//! run tiles of each PNG in `input` (honouring `EngineCtx::tile` and an FP16 toggle), and
//! write results to `output`. Everything else in the pipeline already talks to the
//! [`Upscaler`] trait, so no other module needs to change.

use std::path::Path;

use super::{EngineCtx, EngineError, Upscaler};

pub struct OrtUpscaler {
    pub model_path: std::path::PathBuf,
    pub scale: u32,
    pub fp16: bool,
}

impl Upscaler for OrtUpscaler {
    fn name(&self) -> String {
        "ONNX Runtime (not implemented)".into()
    }

    fn native_scales(&self) -> Vec<u32> {
        vec![self.scale]
    }

    fn supports_cpu(&self) -> bool {
        true
    }

    fn upscale_dir(
        &self,
        _input: &Path,
        _output: &Path,
        _scale: u32,
        _ctx: &EngineCtx,
    ) -> Result<(), EngineError> {
        Err(EngineError::Unavailable(
            "the ONNX Runtime backend is not implemented in this version".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Priority;
    use crate::pipeline::cancel::CancelToken;

    #[test]
    fn stub_reports_unavailable() {
        let u = OrtUpscaler {
            model_path: "x.onnx".into(),
            scale: 4,
            fp16: true,
        };
        let c = CancelToken::new();
        let ctx = EngineCtx {
            cancel: &c,
            gpu: None,
            jobs: "1:2:2".into(),
            tile: 0,
            priority: Priority::Normal,
            progress: None,
        };
        assert!(matches!(
            u.upscale_dir(Path::new("a"), Path::new("b"), 4, &ctx),
            Err(EngineError::Unavailable(_))
        ));
    }
}
