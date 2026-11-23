//! User-provided models in `<base>/models/custom/`: ncnn `.param` + `.bin` pairs (run with
//! realesrgan-ncnn-vulkan, so they must be ESRGAN-style SR models) and `.onnx` files (run with
//! the ONNX backend, tiled with dynamic shapes). The scale is read from the file name
//! (`4x_Foo`, `Foo_x2`, `2xFoo`); unknown scales are probed by the ONNX backend at load time.
//! The user is responsible for the license of these files.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CustomKind {
    Ncnn,
    Onnx,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CustomModel {
    /// `custom:<file stem>` for ncnn, `custom:<file name>` for ONNX.
    pub id: String,
    pub name: String,
    pub kind: CustomKind,
    /// `.param` (ncnn) or `.onnx` file.
    pub path: PathBuf,
    pub scale: Option<u32>,
}

pub const PREFIX: &str = "custom:";

pub fn custom_dir(models_dir: &Path) -> PathBuf {
    models_dir.join("custom")
}

/// Scan the custom models folder (non-recursive), sorted by name.
pub fn scan(dir: &Path) -> Vec<CustomModel> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()).map(str::to_string) else {
            continue;
        };
        let ext = p
            .extension()
            .and_then(|x| x.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let kind = match ext.as_str() {
            "param" if p.with_extension("bin").is_file() => CustomKind::Ncnn,
            "onnx" => CustomKind::Onnx,
            _ => continue,
        };
        let id = match kind {
            CustomKind::Ncnn => format!("{PREFIX}{stem}"),
            CustomKind::Onnx => format!("{PREFIX}{stem}.onnx"),
        };
        out.push(CustomModel {
            id,
            scale: scale_from_name(&stem),
            name: stem,
            kind,
            path: p,
        });
    }
    out.sort_by_key(|m| m.name.to_lowercase());
    out
}

/// Scale from common community naming: `4x_Name`, `4xName`, `Name_x4`, `Name-2x`, `x2_Name`.
/// Only 1..=8 are accepted; the first match wins.
pub fn scale_from_name(name: &str) -> Option<u32> {
    let b = name.to_ascii_lowercase().into_bytes();
    let is_sep = |i: usize| i >= b.len() || !b[i].is_ascii_alphanumeric();
    for i in 0..b.len() {
        // "<d>x" at a word start
        if b[i].is_ascii_digit()
            && (i == 0 || !b[i - 1].is_ascii_alphanumeric())
            && b.get(i + 1) == Some(&b'x')
            && !b.get(i + 2).is_some_and(u8::is_ascii_digit)
        {
            let d = (b[i] - b'0') as u32;
            if (1..=8).contains(&d) {
                return Some(d);
            }
        }
        // "x<d>" as a whole word
        if b[i] == b'x'
            && (i == 0 || !b[i - 1].is_ascii_alphanumeric())
            && b.get(i + 1).is_some_and(u8::is_ascii_digit)
            && is_sep(i + 2)
        {
            let d = (b[i + 1] - b'0') as u32;
            if (1..=8).contains(&d) {
                return Some(d);
            }
        }
    }
    None
}

pub fn is_custom_id(id: &str) -> bool {
    id.starts_with(PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_from_names() {
        assert_eq!(scale_from_name("4x_UltraSharp"), Some(4));
        assert_eq!(scale_from_name("4xNomos2_otf_esrgan"), Some(4));
        assert_eq!(scale_from_name("2xNomosUni_span"), Some(2));
        assert_eq!(scale_from_name("1x-DeJPG"), Some(1));
        assert_eq!(scale_from_name("RealESRGAN_x4plus"), None, "x4plus is not a whole word");
        assert_eq!(scale_from_name("model_x2"), Some(2));
        assert_eq!(scale_from_name("model-x3-fast"), Some(3));
        assert_eq!(scale_from_name("hdr10x"), None);
        assert_eq!(scale_from_name("16x_big"), None);
        assert_eq!(scale_from_name("plain"), None);
    }

    #[test]
    fn scan_finds_pairs_and_onnx() {
        let dir = std::env::temp_dir().join(format!("qe-custom-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["4x_Remacri.param", "4x_Remacri.bin", "lonely.param", "2x_fast.onnx", "notes.txt"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let found = scan(&dir);
        let ids: Vec<&str> = found.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["custom:2x_fast.onnx", "custom:4x_Remacri"]);
        assert_eq!(found[0].kind, CustomKind::Onnx);
        assert_eq!(found[1].scale, Some(4));
        assert!(scan(&dir.join("missing")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
