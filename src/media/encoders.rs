//! Video encoder catalogue, availability detection (listing + 1-frame test encode)
//! and per-encoder quality arguments.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::config::{EncoderChoice, EncoderSpeed, Priority};
use crate::util::{new_command, run_capture};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub enum VideoEncoder {
    H264Nvenc,
    HevcNvenc,
    Av1Nvenc,
    H264Amf,
    HevcAmf,
    Av1Amf,
    H264Qsv,
    HevcQsv,
    Av1Qsv,
    Libx264,
    Libx265,
    LibSvtAv1,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Codec {
    H264,
    Hevc,
    Av1,
}

impl VideoEncoder {
    pub const ALL: [VideoEncoder; 12] = [
        VideoEncoder::H264Nvenc,
        VideoEncoder::HevcNvenc,
        VideoEncoder::Av1Nvenc,
        VideoEncoder::H264Amf,
        VideoEncoder::HevcAmf,
        VideoEncoder::Av1Amf,
        VideoEncoder::H264Qsv,
        VideoEncoder::HevcQsv,
        VideoEncoder::Av1Qsv,
        VideoEncoder::Libx264,
        VideoEncoder::Libx265,
        VideoEncoder::LibSvtAv1,
    ];

    /// Preference order for `Auto`.
    pub const AUTO_ORDER: [VideoEncoder; 9] = [
        VideoEncoder::HevcNvenc,
        VideoEncoder::HevcAmf,
        VideoEncoder::HevcQsv,
        VideoEncoder::H264Nvenc,
        VideoEncoder::H264Amf,
        VideoEncoder::H264Qsv,
        VideoEncoder::Libx265,
        VideoEncoder::Libx264,
        VideoEncoder::LibSvtAv1,
    ];

    pub fn ffmpeg_name(self) -> &'static str {
        match self {
            VideoEncoder::H264Nvenc => "h264_nvenc",
            VideoEncoder::HevcNvenc => "hevc_nvenc",
            VideoEncoder::Av1Nvenc => "av1_nvenc",
            VideoEncoder::H264Amf => "h264_amf",
            VideoEncoder::HevcAmf => "hevc_amf",
            VideoEncoder::Av1Amf => "av1_amf",
            VideoEncoder::H264Qsv => "h264_qsv",
            VideoEncoder::HevcQsv => "hevc_qsv",
            VideoEncoder::Av1Qsv => "av1_qsv",
            VideoEncoder::Libx264 => "libx264",
            VideoEncoder::Libx265 => "libx265",
            VideoEncoder::LibSvtAv1 => "libsvtav1",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            VideoEncoder::H264Nvenc => "NVENC H.264",
            VideoEncoder::HevcNvenc => "NVENC HEVC",
            VideoEncoder::Av1Nvenc => "NVENC AV1",
            VideoEncoder::H264Amf => "AMF H.264",
            VideoEncoder::HevcAmf => "AMF HEVC",
            VideoEncoder::Av1Amf => "AMF AV1",
            VideoEncoder::H264Qsv => "QSV H.264",
            VideoEncoder::HevcQsv => "QSV HEVC",
            VideoEncoder::Av1Qsv => "QSV AV1",
            VideoEncoder::Libx264 => "libx264 (CPU)",
            VideoEncoder::Libx265 => "libx265 (CPU)",
            VideoEncoder::LibSvtAv1 => "SVT-AV1 (CPU)",
        }
    }

    pub fn codec(self) -> Codec {
        match self {
            VideoEncoder::H264Nvenc
            | VideoEncoder::H264Amf
            | VideoEncoder::H264Qsv
            | VideoEncoder::Libx264 => Codec::H264,
            VideoEncoder::HevcNvenc
            | VideoEncoder::HevcAmf
            | VideoEncoder::HevcQsv
            | VideoEncoder::Libx265 => Codec::Hevc,
            VideoEncoder::Av1Nvenc
            | VideoEncoder::Av1Amf
            | VideoEncoder::Av1Qsv
            | VideoEncoder::LibSvtAv1 => Codec::Av1,
        }
    }

    pub fn is_hardware(self) -> bool {
        !matches!(
            self,
            VideoEncoder::Libx264 | VideoEncoder::Libx265 | VideoEncoder::LibSvtAv1
        )
    }

    /// Name of the quality knob shown in the UI.
    pub fn quality_name(self) -> &'static str {
        match self {
            VideoEncoder::H264Nvenc | VideoEncoder::HevcNvenc | VideoEncoder::Av1Nvenc => "CQ",
            VideoEncoder::H264Amf | VideoEncoder::HevcAmf | VideoEncoder::Av1Amf => "QP",
            VideoEncoder::H264Qsv | VideoEncoder::HevcQsv | VideoEncoder::Av1Qsv => {
                "Global quality"
            }
            _ => "CRF",
        }
    }

    /// (default, min, max) for the quality value (lower = better for all of these).
    pub fn quality_range(self) -> (u32, u32, u32) {
        match self {
            VideoEncoder::H264Nvenc => (21, 0, 51),
            VideoEncoder::HevcNvenc => (23, 0, 51),
            VideoEncoder::Av1Nvenc => (28, 0, 63),
            VideoEncoder::H264Amf => (20, 0, 51),
            VideoEncoder::HevcAmf => (22, 0, 51),
            VideoEncoder::Av1Amf => (100, 0, 255),
            VideoEncoder::H264Qsv | VideoEncoder::HevcQsv => (22, 1, 51),
            VideoEncoder::Av1Qsv => (26, 1, 51),
            VideoEncoder::Libx264 => (18, 0, 51),
            VideoEncoder::Libx265 => (21, 0, 51),
            VideoEncoder::LibSvtAv1 => (28, 0, 63),
        }
    }

    /// Codec + rate-control args for `-c:v` (balanced preset, constant quality).
    pub fn args(self, quality: Option<u32>) -> Vec<String> {
        self.args_with(quality, EncoderSpeed::Balanced, None)
    }

    /// Codec args with a speed preset and optional average bitrate (kbit/s).
    pub fn args_with(self, quality: Option<u32>, speed: EncoderSpeed, bitrate_kbps: Option<u32>) -> Vec<String> {
        let mut a = self.quality_args(quality);
        let preset = |fast: &str, bal: &str, best: &str| -> String {
            match speed {
                EncoderSpeed::Fast => fast.into(),
                EncoderSpeed::Balanced => bal.into(),
                EncoderSpeed::Quality => best.into(),
            }
        };
        let (key, value) = match self {
            VideoEncoder::H264Nvenc | VideoEncoder::HevcNvenc | VideoEncoder::Av1Nvenc => ("-preset", preset("p3", "p5", "p7")),
            VideoEncoder::H264Amf | VideoEncoder::HevcAmf | VideoEncoder::Av1Amf => ("-quality", preset("speed", "quality", "quality")),
            VideoEncoder::H264Qsv | VideoEncoder::HevcQsv | VideoEncoder::Av1Qsv => ("-preset", preset("fast", "slower", "veryslow")),
            VideoEncoder::Libx264 | VideoEncoder::Libx265 => ("-preset", preset("fast", "medium", "slow")),
            VideoEncoder::LibSvtAv1 => ("-preset", preset("10", "8", "5")),
        };
        if let Some(i) = a.iter().position(|x| x == key) {
            a[i + 1] = value;
        }
        if let Some(kbps) = bitrate_kbps.filter(|b| *b > 0) {
            // Drop the constant-quality knobs and use average bitrate with a peak cap.
            for k in ["-cq", "-crf", "-global_quality", "-qp_i", "-qp_p", "-qp_b", "-b:v"] {
                while let Some(i) = a.iter().position(|x| x == k) {
                    a.drain(i..i + 2);
                }
            }
            if let Some(i) = a.iter().position(|x| x == "-rc") {
                a[i + 1] = match self {
                    VideoEncoder::H264Amf | VideoEncoder::HevcAmf | VideoEncoder::Av1Amf => "vbr_peak".into(),
                    _ => "vbr".into(),
                };
            }
            let at = a.iter().position(|x| x == "-pix_fmt").unwrap_or(a.len());
            let extra = [
                "-b:v".to_string(),
                format!("{kbps}k"),
                "-maxrate".into(),
                format!("{}k", kbps * 3 / 2),
                "-bufsize".into(),
                format!("{}k", kbps * 2),
            ];
            a.splice(at..at, extra);
        }
        a
    }

    fn quality_args(self, quality: Option<u32>) -> Vec<String> {
        let q = quality.unwrap_or(self.quality_range().0).to_string();
        let mut a: Vec<String> = vec!["-c:v".into(), self.ffmpeg_name().into()];
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        a.extend(match self {
            VideoEncoder::H264Nvenc | VideoEncoder::HevcNvenc | VideoEncoder::Av1Nvenc => {
                s(&["-preset", "p5", "-rc", "vbr", "-cq", &q, "-b:v", "0"])
            }
            VideoEncoder::H264Amf | VideoEncoder::HevcAmf => s(&[
                "-quality", "quality", "-rc", "cqp", "-qp_i", &q, "-qp_p", &q, "-qp_b", &q,
            ]),
            VideoEncoder::Av1Amf => s(&[
                "-quality", "quality", "-rc", "cqp", "-qp_i", &q, "-qp_p", &q,
            ]),
            VideoEncoder::H264Qsv | VideoEncoder::HevcQsv | VideoEncoder::Av1Qsv => {
                s(&["-preset", "slower", "-global_quality", &q])
            }
            VideoEncoder::Libx264 => s(&["-preset", "medium", "-crf", &q]),
            VideoEncoder::Libx265 => s(&[
                "-preset",
                "medium",
                "-crf",
                &q,
                "-x265-params",
                "log-level=error",
            ]),
            VideoEncoder::LibSvtAv1 => s(&["-preset", "8", "-crf", &q]),
        });
        a.push("-pix_fmt".into());
        a.push(self.pix_fmt().into());
        a
    }

    pub fn pix_fmt(self) -> &'static str {
        match self {
            VideoEncoder::H264Qsv | VideoEncoder::HevcQsv | VideoEncoder::Av1Qsv => "nv12",
            _ => "yuv420p",
        }
    }
}

/// Result of startup encoder detection.
#[derive(Clone, Debug, Default)]
pub struct EncoderAvailability {
    pub done: bool,
    /// Listed by `ffmpeg -encoders`.
    pub listed: BTreeSet<VideoEncoder>,
    /// Test-encode result per listed encoder (`Err` = stderr reason).
    pub tested: BTreeMap<VideoEncoder, Result<(), String>>,
}

impl EncoderAvailability {
    pub fn usable(&self, e: VideoEncoder) -> bool {
        matches!(self.tested.get(&e), Some(Ok(())))
    }

    pub fn reason(&self, e: VideoEncoder) -> String {
        if !self.done {
            return crate::t!("encoder.detecting");
        }
        if !self.listed.contains(&e) {
            return crate::t!("encoder.not_in_build");
        }
        match self.tested.get(&e) {
            Some(Err(r)) => crate::t!("encoder.test_failed", reason = r),
            _ => String::new(),
        }
    }

    pub fn auto(&self) -> Option<VideoEncoder> {
        VideoEncoder::AUTO_ORDER
            .into_iter()
            .find(|e| self.usable(*e))
    }

    pub fn resolve(&self, choice: EncoderChoice) -> Option<VideoEncoder> {
        match choice {
            EncoderChoice::Auto => self.auto(),
            EncoderChoice::Specific(e) => Some(e),
        }
    }
}

/// Parse `ffmpeg -encoders` output into the encoders we know about.
pub fn parse_encoder_list(text: &str) -> BTreeSet<VideoEncoder> {
    let mut set = BTreeSet::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(flags), Some(name)) = (it.next(), it.next()) else {
            continue;
        };
        if !flags.starts_with('V') {
            continue;
        }
        if let Some(e) = VideoEncoder::ALL.iter().find(|e| e.ffmpeg_name() == name) {
            set.insert(*e);
        }
    }
    set
}

/// Encode one black frame with `e` to the null muxer.
pub fn test_encode(ffmpeg: &Path, e: VideoEncoder) -> Result<(), String> {
    let mut cmd = new_command(ffmpeg);
    cmd.args([
        "-hide_banner",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=c=black:s=256x256:r=30:d=0.1",
        "-frames:v",
        "1",
    ]);
    cmd.args(e.args(None));
    cmd.args(["-f", "null", "-"]);
    match run_capture(cmd, None, Priority::Normal) {
        Ok(_) => Ok(()),
        Err(err) => {
            let tail = err.tail().lines().last().unwrap_or("").to_string();
            Err(if tail.is_empty() {
                err.to_string()
            } else {
                tail
            })
        }
    }
}

/// Full detection: list, then test every listed encoder in parallel.
pub fn detect(ffmpeg: Option<&Path>) -> EncoderAvailability {
    let Some(ffmpeg) = ffmpeg else {
        return EncoderAvailability {
            done: true,
            ..Default::default()
        };
    };
    let mut cmd = new_command(ffmpeg);
    cmd.args(["-hide_banner", "-encoders"]);
    let listed = match run_capture(cmd, None, Priority::Normal) {
        Ok(o) => parse_encoder_list(&String::from_utf8_lossy(&o.stdout)),
        Err(e) => {
            tracing::warn!("ffmpeg -encoders failed: {e}");
            BTreeSet::new()
        }
    };
    let tested = std::thread::scope(|s| {
        let handles: Vec<_> = listed
            .iter()
            .map(|&e| (e, s.spawn(move || test_encode(ffmpeg, e))))
            .collect();
        handles
            .into_iter()
            .map(|(e, h)| (e, h.join().unwrap_or_else(|_| Err("panic".into()))))
            .collect::<BTreeMap<_, _>>()
    });
    for (e, r) in &tested {
        match r {
            Ok(()) => tracing::info!("encoder {} usable", e.ffmpeg_name()),
            Err(r) => tracing::info!("encoder {} unusable: {r}", e.ffmpeg_name()),
        }
    }
    EncoderAvailability {
        done: true,
        listed,
        tested,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_list() {
        let text = " V....D libx264              libx264 H.264 / AVC\n V....D h264_nvenc NVIDIA NVENC\n A....D aac AAC\n V..... hevc_qsv x\n";
        let s = parse_encoder_list(text);
        assert!(s.contains(&VideoEncoder::Libx264));
        assert!(s.contains(&VideoEncoder::H264Nvenc));
        assert!(s.contains(&VideoEncoder::HevcQsv));
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn auto_prefers_hw_hevc() {
        let mut a = EncoderAvailability {
            done: true,
            ..Default::default()
        };
        a.tested.insert(VideoEncoder::Libx264, Ok(()));
        a.tested
            .insert(VideoEncoder::HevcNvenc, Err("no device".into()));
        a.tested.insert(VideoEncoder::H264Nvenc, Ok(()));
        assert_eq!(a.auto(), Some(VideoEncoder::H264Nvenc));
    }

    #[test]
    fn quality_args_contain_value() {
        let a = VideoEncoder::Libx264.args(Some(20));
        assert!(a.windows(2).any(|w| w[0] == "-crf" && w[1] == "20"));
        let a = VideoEncoder::HevcNvenc.args(None);
        assert!(a.windows(2).any(|w| w[0] == "-cq" && w[1] == "23"));
    }

    #[test]
    fn speed_and_bitrate_args() {
        let a = VideoEncoder::HevcNvenc.args_with(None, EncoderSpeed::Quality, None).join(" ");
        assert!(a.contains("-preset p7") && a.contains("-cq 23"), "{a}");
        let a = VideoEncoder::HevcNvenc.args_with(None, EncoderSpeed::Fast, Some(8000)).join(" ");
        assert!(a.contains("-preset p3") && a.contains("-b:v 8000k -maxrate 12000k -bufsize 16000k"), "{a}");
        assert!(!a.contains("-cq") && a.ends_with("-pix_fmt yuv420p"), "{a}");
        let a = VideoEncoder::Libx264.args_with(Some(20), EncoderSpeed::Fast, None).join(" ");
        assert!(a.contains("-preset fast -crf 20"), "{a}");
        let a = VideoEncoder::HevcAmf.args_with(None, EncoderSpeed::Balanced, Some(5000)).join(" ");
        assert!(a.contains("-rc vbr_peak") && !a.contains("-qp_i"), "{a}");
        // Default = 2.0 behaviour.
        assert_eq!(VideoEncoder::Libx265.args(None), VideoEncoder::Libx265.args_with(None, EncoderSpeed::Balanced, None));
    }
}
