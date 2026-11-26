//! "Make sharp (AI)" for still images: blur analysis, the stage plan (deblur → smart downscale →
//! rebuild detail → target size → faces → light sharpen) and its runner. Video is not touched.

pub mod analysis;
pub mod meta;
pub mod plan;
pub mod region;
pub mod run;
pub mod seedvr2;
