//! Media metadata: ffprobe JSON for videos, the `image` crate for stills.

use serde::Deserialize;
use std::path::{Path, PathBuf};

use super::{MediaKind, Rational};
use crate::config::Priority;
use crate::util::{ProcError, new_command, run_capture};

#[derive(thiserror::Error, Debug)]
pub enum ProbeError {
    #[error("ffprobe not found (place it in bin/)")]
    NoFfprobe,
    #[error("unsupported file type")]
    Unsupported,
    #[error("{0}")]
    Proc(#[from] ProcError),
    #[error("could not parse ffprobe output: {0}")]
    Json(#[from] serde_json::Error),
    #[error("no video stream found")]
    NoVideo,
    #[error("image error: {0}")]
    Image(#[from] image::ImageError),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug, PartialEq)]
pub struct MediaInfo {
    pub path: PathBuf,
    pub kind: MediaKind,
    pub width: u32,
    pub height: u32,
    /// Average frame rate (videos).
    pub fps: Option<Rational>,
    /// Probable variable frame rate (r_frame_rate differs from avg_frame_rate).
    pub vfr: bool,
    pub duration: Option<f64>,
    /// Frame count estimate at the average fps.
    pub frames: Option<u64>,
    pub video_codec: Option<String>,
    pub audio_codecs: Vec<String>,
    /// "channels@sample_rate" of the first audio stream (join compatibility check).
    pub audio_layout: Option<String>,
    pub subtitle_codecs: Vec<String>,
    pub pix_fmt: Option<String>,
    pub bit_depth: u8,
    pub hdr: bool,
    pub has_alpha: bool,
    pub file_size: u64,
    /// Image container format (images only).
    pub image_format: Option<String>,
}

impl MediaInfo {
    /// "Length 1:23 (83.4 s · 2,001 frames)" for videos; `None` for images.
    pub fn length_label(&self) -> Option<String> {
        if self.kind != crate::media::MediaKind::Video {
            return None;
        }
        let d = self.duration?;
        let time = crate::util::human_duration(d);
        let secs = format!("{d:.1}");
        Some(match self.frames {
            Some(f) => crate::t!(
                "probe.length_frames",
                time = time,
                secs = secs,
                frames = crate::util::thousands(f)
            ),
            None => crate::t!("probe.length", time = time, secs = secs),
        })
    }

    pub fn summary(&self) -> String {
        let mut s = format!("{}x{}", self.width, self.height);
        if let Some(fps) = self.fps {
            s += &format!(" · {fps} fps{}", if self.vfr { " (VFR)" } else { "" });
        }
        if let Some(d) = self.duration {
            s += &format!(" · {}", crate::util::human_duration(d));
        }
        if let Some(c) = &self.video_codec {
            s += &format!(" · {c}");
        }
        if !self.audio_codecs.is_empty() {
            s += &format!(" + {}", self.audio_codecs.join(","));
        }
        if let Some(f) = &self.image_format {
            s += &format!(" · {f}");
        }
        s += &format!(" · {}-bit", self.bit_depth);
        if self.hdr {
            s += " HDR";
        }
        s += &format!(" · {}", crate::util::human_bytes(self.file_size));
        s
    }

    /// 10-bit+/HDR sources are processed as 8-bit SDR in this version.
    pub fn needs_sdr_warning(&self) -> bool {
        self.kind == MediaKind::Video && (self.bit_depth > 8 || self.hdr)
    }
}

// ---- ffprobe JSON --------------------------------------------------------------------

#[derive(Deserialize, Debug, Default)]
struct ProbeJson {
    #[serde(default)]
    streams: Vec<StreamJson>,
    #[serde(default)]
    format: Option<FormatJson>,
}

#[derive(Deserialize, Debug, Default)]
struct StreamJson {
    codec_type: Option<String>,
    codec_name: Option<String>,
    channels: Option<u32>,
    sample_rate: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    pix_fmt: Option<String>,
    r_frame_rate: Option<String>,
    avg_frame_rate: Option<String>,
    duration: Option<String>,
    nb_frames: Option<String>,
    bits_per_raw_sample: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    #[serde(default)]
    disposition: Option<DispositionJson>,
    #[serde(default)]
    side_data_list: Vec<serde_json::Value>,
    #[serde(default)]
    tags: std::collections::HashMap<String, String>,
}

impl StreamJson {
    /// Display rotation in degrees (ffmpeg auto-rotates decoded frames by this).
    fn rotation(&self) -> i64 {
        let from_side = self
            .side_data_list
            .iter()
            .find_map(|v| v.get("rotation")?.as_f64());
        let from_tag = self.tags.get("rotate").and_then(|r| r.parse::<f64>().ok());
        from_side
            .or(from_tag)
            .map(|r| r.round() as i64)
            .unwrap_or(0)
    }
}

#[derive(Deserialize, Debug, Default)]
struct DispositionJson {
    #[serde(default)]
    attached_pic: u8,
}

#[derive(Deserialize, Debug, Default)]
struct FormatJson {
    duration: Option<String>,
    size: Option<String>,
}

fn bit_depth_from_pix_fmt(pix_fmt: &str) -> Option<u8> {
    for (pat, d) in [("16", 16u8), ("14", 14), ("12", 12), ("10", 10), ("9", 9)] {
        if pix_fmt.contains(&format!("p{pat}"))
            || pix_fmt.ends_with(&format!("{pat}le"))
            || pix_fmt.ends_with(&format!("{pat}be"))
        {
            return Some(d);
        }
    }
    None
}

/// Parse `ffprobe -print_format json -show_format -show_streams` output.
pub fn parse_ffprobe_json(
    path: &Path,
    json: &str,
    file_size: u64,
) -> Result<MediaInfo, ProbeError> {
    let p: ProbeJson = serde_json::from_str(json)?;
    let v = p
        .streams
        .iter()
        .find(|s| {
            s.codec_type.as_deref() == Some("video")
                && s.disposition.as_ref().is_none_or(|d| d.attached_pic == 0)
        })
        .ok_or(ProbeError::NoVideo)?;
    let avg = v.avg_frame_rate.as_deref().and_then(Rational::parse);
    let r = v.r_frame_rate.as_deref().and_then(Rational::parse);
    let fps = avg.or(r);
    let vfr = match (avg, r) {
        (Some(a), Some(r)) => {
            let (a, r) = (a.as_f64(), r.as_f64());
            (a - r).abs() / r.max(1e-6) > 0.01
        }
        _ => false,
    };
    let duration = v
        .duration
        .as_deref()
        .and_then(|d| d.parse::<f64>().ok())
        .or_else(|| p.format.as_ref()?.duration.as_deref()?.parse().ok())
        .filter(|d| *d > 0.0);
    let frames = match (duration, fps) {
        (Some(d), Some(f)) => Some((d * f.as_f64()).round() as u64),
        _ => v.nb_frames.as_deref().and_then(|n| n.parse().ok()),
    };
    let pix_fmt = v.pix_fmt.clone();
    let bit_depth = pix_fmt
        .as_deref()
        .and_then(bit_depth_from_pix_fmt)
        .or_else(|| v.bits_per_raw_sample.as_deref()?.parse().ok())
        .unwrap_or(8);
    let hdr = matches!(
        v.color_transfer.as_deref(),
        Some("smpte2084") | Some("arib-std-b67")
    ) || v.color_primaries.as_deref() == Some("bt2020");
    let has_alpha = pix_fmt
        .as_deref()
        .is_some_and(|f| f.starts_with("yuva") || f.contains("rgba") || f.contains("argb"));
    let codecs_of = |t: &str| {
        p.streams
            .iter()
            .filter(|s| s.codec_type.as_deref() == Some(t))
            .filter_map(|s| s.codec_name.clone())
            .collect::<Vec<_>>()
    };
    let file_size = p
        .format
        .as_ref()
        .and_then(|f| f.size.as_deref()?.parse().ok())
        .unwrap_or(file_size);
    // ffmpeg applies the display matrix when decoding, so report display dimensions.
    let (mut width, mut height) = (v.width.unwrap_or(0), v.height.unwrap_or(0));
    if v.rotation().rem_euclid(180) == 90 {
        std::mem::swap(&mut width, &mut height);
    }
    Ok(MediaInfo {
        path: path.to_path_buf(),
        kind: MediaKind::Video,
        width,
        height,
        fps,
        vfr,
        duration,
        frames,
        video_codec: v.codec_name.clone(),
        audio_codecs: codecs_of("audio"),
        audio_layout: p
            .streams
            .iter()
            .find(|s| s.codec_type.as_deref() == Some("audio"))
            .map(|s| format!("{}@{}", s.channels.unwrap_or(0), s.sample_rate.as_deref().unwrap_or("?"))),
        subtitle_codecs: codecs_of("subtitle"),
        pix_fmt,
        bit_depth,
        hdr,
        has_alpha,
        file_size,
        image_format: None,
    })
}

pub fn probe_video(ffprobe: Option<&Path>, path: &Path) -> Result<MediaInfo, ProbeError> {
    let ffprobe = ffprobe.ok_or(ProbeError::NoFfprobe)?;
    let mut cmd = new_command(ffprobe);
    cmd.args([
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
    ])
    .arg(path);
    let out = run_capture(cmd, None, Priority::Normal)?;
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let info = parse_ffprobe_json(path, &String::from_utf8_lossy(&out.stdout), size)?;
    if info.width == 0 || info.height == 0 {
        return Err(ProbeError::NoVideo);
    }
    Ok(info)
}

pub fn probe_image(path: &Path) -> Result<MediaInfo, ProbeError> {
    use image::ImageDecoder;
    let reader = image::ImageReader::open(path)?.with_guessed_format()?;
    let format = reader.format();
    let decoder = reader.into_decoder()?;
    let (w, h) = decoder.dimensions();
    let ct = decoder.color_type();
    let bits = (ct.bits_per_pixel() / ct.channel_count() as u16) as u8;
    Ok(MediaInfo {
        path: path.to_path_buf(),
        kind: MediaKind::Image,
        width: w,
        height: h,
        fps: None,
        vfr: false,
        duration: None,
        frames: None,
        video_codec: None,
        audio_codecs: vec![],
        audio_layout: None,
        subtitle_codecs: vec![],
        pix_fmt: Some(format!("{ct:?}")),
        bit_depth: bits,
        hdr: false,
        has_alpha: ct.has_alpha(),
        file_size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        image_format: format.map(|f| format!("{f:?}").to_uppercase()),
    })
}

pub fn probe(ffprobe: Option<&Path>, path: &Path) -> Result<MediaInfo, ProbeError> {
    match super::detect_kind(path) {
        Some(MediaKind::Image) => probe_image(path),
        Some(MediaKind::Video) => probe_video(ffprobe, path),
        None => Err(ProbeError::Unsupported),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "streams": [
        { "index": 0, "codec_name": "h264", "codec_type": "video", "width": 1920, "height": 1080,
          "pix_fmt": "yuv420p", "r_frame_rate": "30000/1001", "avg_frame_rate": "30000/1001",
          "duration": "10.010000", "nb_frames": "300", "bits_per_raw_sample": "8",
          "disposition": { "default": 1, "attached_pic": 0 } },
        { "index": 1, "codec_name": "aac", "codec_type": "audio", "duration": "10.0" },
        { "index": 2, "codec_name": "subrip", "codec_type": "subtitle" }
      ],
      "format": { "filename": "x.mkv", "duration": "10.010000", "size": "1234567" }
    }"#;

    #[test]
    fn parses_basic_video() {
        let i = parse_ffprobe_json(Path::new("x.mkv"), SAMPLE, 0).unwrap();
        assert_eq!((i.width, i.height), (1920, 1080));
        assert_eq!(i.fps, Some(Rational::new(30000, 1001)));
        assert!(!i.vfr);
        assert_eq!(i.frames, Some(300));
        assert_eq!(i.video_codec.as_deref(), Some("h264"));
        assert_eq!(i.audio_codecs, vec!["aac"]);
        assert_eq!(i.subtitle_codecs, vec!["subrip"]);
        assert_eq!(i.bit_depth, 8);
        assert!(!i.hdr);
        assert_eq!(i.file_size, 1234567);
    }

    #[test]
    fn detects_vfr_hdr_10bit_and_skips_cover_art() {
        let json = r#"{
          "streams": [
            { "codec_name": "mjpeg", "codec_type": "video", "width": 300, "height": 300,
              "avg_frame_rate": "0/0", "r_frame_rate": "90000/1", "disposition": { "attached_pic": 1 } },
            { "codec_name": "hevc", "codec_type": "video", "width": 3840, "height": 2160,
              "pix_fmt": "yuv420p10le", "r_frame_rate": "120/1", "avg_frame_rate": "2997/100",
              "color_transfer": "smpte2084", "color_primaries": "bt2020" }
          ],
          "format": { "duration": "5.0" }
        }"#;
        let i = parse_ffprobe_json(Path::new("x.mp4"), json, 99).unwrap();
        assert_eq!(i.width, 3840);
        assert!(i.vfr);
        assert!(i.hdr);
        assert_eq!(i.bit_depth, 10);
        assert_eq!(i.duration, Some(5.0));
        assert_eq!(i.frames, Some(150));
        assert_eq!(i.file_size, 99);
        assert!(i.needs_sdr_warning());
    }

    #[test]
    fn rotated_video_reports_display_dims() {
        let json = r#"{ "streams": [ { "codec_type": "video", "codec_name": "h264",
            "width": 1920, "height": 1080, "avg_frame_rate": "30/1", "r_frame_rate": "30/1",
            "side_data_list": [ { "side_data_type": "Display Matrix", "rotation": -90 } ] } ] }"#;
        let i = parse_ffprobe_json(Path::new("p.mp4"), json, 0).unwrap();
        assert_eq!((i.width, i.height), (1080, 1920));
    }

    #[test]
    fn no_video_stream_is_error() {
        let json = r#"{ "streams": [ { "codec_type": "audio", "codec_name": "mp3" } ] }"#;
        assert!(matches!(
            parse_ffprobe_json(Path::new("a.mp3"), json, 0),
            Err(ProbeError::NoVideo)
        ));
    }
}
