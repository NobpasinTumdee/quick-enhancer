//! FFmpeg/FFprobe integration (subprocesses only) and media type detection.

pub mod encoders;
pub mod ffmpeg;
pub mod probe;

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const IMAGE_EXTS: [&str; 7] = ["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff"];
pub const VIDEO_EXTS: [&str; 5] = ["mp4", "mkv", "mov", "webm", "avi"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    Image,
    Video,
}

/// Detect the media kind from the extension, falling back to sniffing image magic bytes.
pub fn detect_kind(path: &Path) -> Option<MediaKind> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    if let Some(ext) = ext.as_deref() {
        if IMAGE_EXTS.contains(&ext) {
            return Some(MediaKind::Image);
        }
        if VIDEO_EXTS.contains(&ext) {
            return Some(MediaKind::Video);
        }
    }
    // Content sniffing for misnamed files.
    let mut head = [0u8; 16];
    let n = std::fs::File::open(path)
        .and_then(|mut f| std::io::Read::read(&mut f, &mut head))
        .ok()?;
    let head = &head[..n];
    match image::guess_format(head) {
        Ok(
            image::ImageFormat::Png
            | image::ImageFormat::Jpeg
            | image::ImageFormat::WebP
            | image::ImageFormat::Bmp
            | image::ImageFormat::Tiff,
        ) => Some(MediaKind::Image),
        _ => {
            // ISO-BMFF (mp4/mov) "ftyp", Matroska/WebM EBML, AVI RIFF.
            let mp4 = head.len() >= 12 && &head[4..8] == b"ftyp";
            let mkv = head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]);
            let avi = head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"AVI ";
            (mp4 || mkv || avi).then_some(MediaKind::Video)
        }
    }
}

/// Exact frame rate as a fraction (e.g. 30000/1001).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rational {
    pub num: u64,
    pub den: u64,
}

impl Rational {
    pub const fn new(num: u64, den: u64) -> Rational {
        Rational { num, den }
    }

    pub fn parse(s: &str) -> Option<Rational> {
        let s = s.trim();
        let (n, d) = match s.split_once('/') {
            Some((n, d)) => (n.trim().parse::<u64>().ok()?, d.trim().parse::<u64>().ok()?),
            None => {
                let f: f64 = s.parse().ok()?;
                ((f * 1000.0).round() as u64, 1000)
            }
        };
        if n == 0 || d == 0 {
            return None;
        }
        Some(Rational::new(n, d).reduced())
    }

    pub fn reduced(self) -> Rational {
        fn gcd(a: u64, b: u64) -> u64 {
            if b == 0 { a } else { gcd(b, a % b) }
        }
        let g = gcd(self.num, self.den).max(1);
        Rational::new(self.num / g, self.den / g)
    }

    pub fn as_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    pub fn times(self, k: u32) -> Rational {
        Rational::new(self.num * k as u64, self.den).reduced()
    }

    pub fn ffmpeg(self) -> String {
        format!("{}/{}", self.num, self.den)
    }
}

impl std::fmt::Display for Rational {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let v = self.as_f64();
        if (v - v.round()).abs() < 0.005 {
            write!(f, "{}", v.round() as u64)
        } else {
            write!(f, "{v:.3}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rational_parse() {
        assert_eq!(
            Rational::parse("30000/1001"),
            Some(Rational::new(30000, 1001))
        );
        assert_eq!(Rational::parse("50/2"), Some(Rational::new(25, 1)));
        assert_eq!(Rational::parse("0/0"), None);
        assert_eq!(Rational::parse("24"), Some(Rational::new(24, 1)));
        assert_eq!(
            Rational::new(30000, 1001).times(2),
            Rational::new(60000, 1001)
        );
        assert_eq!(format!("{}", Rational::new(30000, 1001)), "29.970");
        assert_eq!(format!("{}", Rational::new(60, 1)), "60");
    }
}
