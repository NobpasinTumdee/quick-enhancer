//! The model catalog (`models.json`): which AI models exist, what they need and where their
//! files come from. The catalog is embedded at build time; a newer copy can be fetched from the
//! models release (`update_url`) and is cached as `<base>/models.json`.
//!
//! Download units are ordinary [`Component`]s (same downloader / verifier / installer as
//! `tools.json`). A model lists the package ids it needs; packages may come from this catalog
//! or from `tools.json` (Real-ESRGAN, RIFE).

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

use crate::i18n::{self, Lang};
use crate::net::http::NetError;
use crate::net::tools::{Component, ToolsManifest};

pub const EMBEDDED_CATALOG: &str = include_str!("../../models.json");
/// Catalog schema this build understands. A cached catalog with another schema is ignored.
pub const SCHEMA: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Text {
    pub en: String,
    pub th: String,
}

impl Text {
    pub fn get(&self) -> &str {
        self.in_lang(i18n::lang())
    }
    pub fn in_lang(&self, l: Lang) -> &str {
        match l {
            Lang::Th if !self.th.is_empty() => &self.th,
            _ => &self.en,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Upscale,
    /// 1x restoration (JPEG / noise) at the original size.
    Restore,
    Face,
    Interpolate,
    /// Whole-clip video models (SeedVR2).
    Video,
    /// Support models (face detector); not shown on their own.
    Helper,
}

impl Group {
    pub const SHOWN: [Group; 5] = [
        Group::Upscale,
        Group::Restore,
        Group::Face,
        Group::Interpolate,
        Group::Video,
    ];
    pub fn label_key(self) -> &'static str {
        match self {
            Group::Upscale => "models.group_upscale",
            Group::Restore => "models.group_restore",
            Group::Face => "models.group_face",
            Group::Interpolate => "models.group_interpolate",
            Group::Video => "models.group_video",
            Group::Helper => "models.group_helper",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    NcnnRealesrgan,
    NcnnRealcugan,
    NcnnRife,
    Onnx,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Precision {
    #[serde(rename = "fp32")]
    Fp32,
    #[serde(rename = "fp16")]
    Fp16,
}

/// Pixel value range of an ONNX model's input and output.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    /// 0..1
    #[serde(rename = "01")]
    Unit,
    /// -1..1
    #[serde(rename = "pm1")]
    PlusMinusOne,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OnnxSpec {
    /// Model file relative to the tools root.
    pub file: String,
    /// Tile edge in input pixels.
    pub tile: u32,
    /// The graph only accepts `tile × tile` input (static export); edge tiles are padded.
    pub static_tile: bool,
    pub precision: Precision,
    pub input: String,
    pub output: String,
    pub range: Range,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Cost {
    /// Seconds per input megapixel on the reference PC (RTX-class GPU, DirectML / Vulkan).
    pub s_per_mp: f32,
    /// VRAM used at the default tile size.
    pub vram_mb: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct License {
    pub name: String,
    pub url: String,
    /// Commercial use allowed. `false` → the UI shows a "Non-commercial" badge and asks the
    /// user to accept the license before installing.
    pub commercial: bool,
    /// The license allows us to host the files.
    pub redistribute: bool,
    pub attribution: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ModelEntry {
    pub id: String,
    pub group: Group,
    pub name: Text,
    pub desc: Text,
    pub backend: Backend,
    /// Native scales (1 for restoration / face models).
    pub scales: Vec<u32>,
    /// ncnn model name (`-n`), model folder (`models-se`) or RIFE model folder.
    #[serde(default)]
    pub ncnn_name: Option<String>,
    #[serde(default)]
    pub onnx: Option<OnnxSpec>,
    /// 1 (slowest) .. 5 (fastest).
    pub speed: u8,
    /// 1 .. 5.
    pub quality: u8,
    pub vram_min_mb: u32,
    pub vram_rec_mb: u32,
    #[serde(default)]
    pub cost: Cost,
    pub license: License,
    pub source: String,
    /// Package ids (from this catalog or `tools.json`) that provide the files.
    pub packages: Vec<String>,
    /// Files (relative to the tools root) that must exist for the model to count as installed.
    pub files: Vec<String>,
    /// Content tags: `photo`, `anime`, `video`.
    #[serde(default)]
    pub content: Vec<String>,
    #[serde(default)]
    pub experimental: bool,
    /// Other model ids, or `runtime:onnx` (any ONNX runtime) / `runtime:cuda`.
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub hidden: bool,
}

impl ModelEntry {
    pub fn non_commercial(&self) -> bool {
        !self.license.commercial
    }
    pub fn max_scale(&self) -> u32 {
        self.scales.iter().copied().max().unwrap_or(1)
    }
    pub fn needs_runtime(&self) -> Option<RuntimeNeed> {
        if self.requires.iter().any(|r| r == "runtime:cuda") {
            Some(RuntimeNeed::Cuda)
        } else if self.requires.iter().any(|r| r == "runtime:onnx") || self.backend == Backend::Onnx {
            Some(RuntimeNeed::Any)
        } else {
            None
        }
    }
    /// Required helper models (not runtimes).
    pub fn required_models(&self) -> impl Iterator<Item = &str> {
        self.requires
            .iter()
            .filter(|r| !r.starts_with("runtime:"))
            .map(String::as_str)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeNeed {
    /// DirectML or CUDA.
    Any,
    Cuda,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Catalog {
    pub schema: u32,
    pub catalog_version: String,
    #[serde(default)]
    pub update_url: String,
    pub packages: Vec<Component>,
    pub models: Vec<ModelEntry>,
}

impl Default for Catalog {
    fn default() -> Self {
        Catalog::embedded()
    }
}

/// Package ids of the ONNX runtimes.
pub const RUNTIME_DML: &[&str] = &["ort-dml"];
pub const RUNTIME_CUDA: &[&str] = &["ort-cuda", "cuda-runtime", "cuda-cublas", "cuda-nvrtc", "cuda-cudnn"];

impl Catalog {
    /// Parse and validate. `tools` provides the package ids that live in `tools.json`.
    pub fn parse(text: &str, tools: &ToolsManifest) -> Result<Catalog, NetError> {
        let c: Catalog =
            serde_json::from_str(text).map_err(|e| NetError::Invalid(format!("models.json: {e}")))?;
        c.validate(tools)?;
        Ok(c)
    }

    pub fn embedded() -> Catalog {
        Self::parse(EMBEDDED_CATALOG, &ToolsManifest::embedded())
            .expect("embedded models.json is valid (tested)")
    }

    /// The cached catalog in `<base>/models.json` when it is valid, has our schema and is newer
    /// than the embedded one; otherwise the embedded catalog.
    pub fn load(base: &Path) -> Catalog {
        let embedded = Self::embedded();
        let cached = std::fs::read_to_string(base.join("models.json"))
            .ok()
            .and_then(|t| Self::parse(&t, &ToolsManifest::embedded()).ok());
        match cached {
            Some(c) if c.schema == SCHEMA && version_newer(&c.catalog_version, &embedded.catalog_version) => c,
            _ => embedded,
        }
    }

    /// Fetch the newest catalog from `update_url`; store it when it is valid and newer.
    /// Returns `Ok(Some(version))` when a newer catalog was stored.
    pub fn refresh(&self, base: &Path) -> Result<Option<String>, NetError> {
        if self.update_url.is_empty() {
            return Ok(None);
        }
        let text = crate::net::http::get_text(&self.update_url)?;
        let new = Self::parse(&text, &ToolsManifest::embedded())?;
        if new.schema != SCHEMA || !version_newer(&new.catalog_version, &self.catalog_version) {
            return Ok(None);
        }
        std::fs::create_dir_all(base)?;
        let tmp = base.join("models.json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, base.join("models.json"))?;
        Ok(Some(new.catalog_version))
    }

    pub fn validate(&self, tools: &ToolsManifest) -> Result<(), NetError> {
        let bad = |m: String| Err(NetError::Invalid(format!("models.json: {m}")));
        if self.schema != SCHEMA {
            return bad(format!("unsupported schema {}", self.schema));
        }
        let mut pkg_ids = BTreeSet::new();
        for p in &self.packages {
            p.validate("models.json")?;
            if !pkg_ids.insert(p.id.as_str()) {
                return bad(format!("duplicate package id {}", p.id));
            }
        }
        let tool_ids: BTreeSet<&str> = tools.components.iter().map(|c| c.id.as_str()).collect();
        let mut ids = BTreeSet::new();
        for m in &self.models {
            if !ids.insert(m.id.as_str()) {
                return bad(format!("duplicate model id {}", m.id));
            }
            if m.id.starts_with("custom:") || m.id.contains(char::is_whitespace) {
                return bad(format!("invalid model id {}", m.id));
            }
            if m.scales.is_empty() || m.scales.iter().any(|&s| s == 0 || s > 8) {
                return bad(format!("{}: bad scales", m.id));
            }
            if !(1..=5).contains(&m.speed) || !(1..=5).contains(&m.quality) {
                return bad(format!("{}: speed/quality must be 1..5", m.id));
            }
            if m.vram_rec_mb < m.vram_min_mb {
                return bad(format!("{}: vram_rec_mb < vram_min_mb", m.id));
            }
            if m.packages.is_empty() || m.files.is_empty() {
                return bad(format!("{}: needs packages and files", m.id));
            }
            for p in &m.packages {
                if !pkg_ids.contains(p.as_str()) && !tool_ids.contains(p.as_str()) {
                    return bad(format!("{}: unknown package {p}", m.id));
                }
            }
            if m.files.iter().any(|f| f.contains("..") || f.starts_with('/') || f.contains(':')) {
                return bad(format!("{}: unsafe file path", m.id));
            }
            match (m.backend, &m.onnx) {
                (Backend::Onnx, None) if m.group != Group::Video => {
                    return bad(format!("{}: ONNX model without an onnx spec", m.id));
                }
                (Backend::Onnx, Some(s)) => {
                    if s.tile < 16 || !m.files.contains(&s.file) {
                        return bad(format!("{}: bad onnx spec", m.id));
                    }
                }
                (_, Some(_)) => return bad(format!("{}: onnx spec on an ncnn model", m.id)),
                _ => {}
            }
            if m.backend != Backend::Onnx && m.ncnn_name.is_none() {
                return bad(format!("{}: ncnn model without ncnn_name", m.id));
            }
            if m.backend == Backend::Onnx && m.needs_runtime().is_none() {
                return bad(format!("{}: ONNX model must require a runtime", m.id));
            }
            if !m.license.redistribute
                && m.packages
                    .iter()
                    .filter_map(|p| self.package(p))
                    .any(|p| p.url.contains("/quick-enhancer/releases/"))
            {
                return bad(format!("{}: license forbids hosting, but it is on our release", m.id));
            }
        }
        for m in &self.models {
            for r in &m.requires {
                let ok = matches!(r.as_str(), "runtime:onnx" | "runtime:cuda") || ids.contains(r.as_str());
                if !ok {
                    return bad(format!("{}: unknown requirement {r}", m.id));
                }
            }
        }
        for p in RUNTIME_DML.iter().chain(RUNTIME_CUDA) {
            if !pkg_ids.contains(p) {
                return bad(format!("runtime package {p} missing"));
            }
        }
        Ok(())
    }

    pub fn model(&self, id: &str) -> Option<&ModelEntry> {
        self.models.iter().find(|m| m.id == id)
    }

    pub fn package(&self, id: &str) -> Option<&Component> {
        self.packages.iter().find(|p| p.id == id)
    }

    /// Package from this catalog or from `tools.json`.
    pub fn resolve_package(&self, id: &str, tools: &ToolsManifest) -> Option<Component> {
        self.package(id)
            .cloned()
            .or_else(|| tools.components.iter().find(|c| c.id == id).cloned())
    }

    pub fn in_group(&self, g: Group) -> impl Iterator<Item = &ModelEntry> {
        self.models.iter().filter(move |m| m.group == g && !m.hidden)
    }

    /// Ncnn Real-ESRGAN entry by its `-n` name (used to map existing profiles).
    pub fn by_ncnn_name(&self, backend: Backend, name: &str) -> Option<&ModelEntry> {
        self.models
            .iter()
            .find(|m| m.backend == backend && m.ncnn_name.as_deref() == Some(name))
    }
}

/// Dotted numeric version compare (`2026.11.2` > `2026.11.1`). Non-numeric parts compare as 0.
pub fn version_newer(a: &str, b: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> { s.split('.').map(|p| p.trim().parse().unwrap_or(0)).collect() };
    let (a, b) = (parse(a), parse(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools() -> ToolsManifest {
        ToolsManifest::embedded()
    }

    #[test]
    fn embedded_catalog_is_valid() {
        let c = Catalog::embedded();
        assert_eq!(c.schema, SCHEMA);
        for g in Group::SHOWN {
            assert!(c.in_group(g).next().is_some(), "{g:?} empty");
        }
        // Every model's packages resolve and every file is written by one of its packages.
        let t = tools();
        for m in &c.models {
            let pkgs: Vec<Component> = m
                .packages
                .iter()
                .map(|p| c.resolve_package(p, &t).unwrap())
                .collect();
            for f in &m.files {
                let covered = pkgs.iter().any(|p| {
                    p.files.iter().any(|r| {
                        r.to == *f || (r.to.ends_with('/') && f.starts_with(r.to.as_str()))
                    })
                });
                assert!(covered, "{}: no package writes {f}", m.id);
            }
        }
    }

    #[test]
    fn non_commercial_models_are_flagged() {
        let c = Catalog::embedded();
        let cf = c.model("codeformer").unwrap();
        assert!(cf.non_commercial());
        assert!(!c.model("gfpgan-1.4").unwrap().non_commercial());
        assert!(c.model("hatl-bhi-4x").unwrap().experimental);
        assert!(c.model("seedvr2-3b").unwrap().experimental);
        assert_eq!(c.model("seedvr2-3b").unwrap().needs_runtime(), Some(RuntimeNeed::Cuda));
        assert_eq!(c.model("span-nomosuni-2x").unwrap().needs_runtime(), Some(RuntimeNeed::Any));
        assert_eq!(c.model("realesrgan-x4plus").unwrap().needs_runtime(), None);
    }

    fn mutate(f: impl FnOnce(&mut serde_json::Value)) -> Result<Catalog, NetError> {
        let mut v: serde_json::Value = serde_json::from_str(EMBEDDED_CATALOG).unwrap();
        f(&mut v);
        Catalog::parse(&v.to_string(), &tools())
    }

    #[test]
    fn validation_rejects_broken_catalogs() {
        assert!(mutate(|_| {}).is_ok());
        assert!(mutate(|v| v["schema"] = 99.into()).is_err());
        assert!(mutate(|v| v["models"][0]["packages"][0] = "nope".into()).is_err());
        assert!(mutate(|v| v["models"][0]["requires"] = serde_json::json!(["ghost"])).is_err());
        assert!(mutate(|v| v["models"][0]["scales"] = serde_json::json!([0])).is_err());
        assert!(mutate(|v| v["models"][0]["files"][0] = "../../evil.dll".into()).is_err());
        assert!(mutate(|v| v["packages"][0]["sha256"] = "abc".into()).is_err());
        assert!(mutate(|v| v["packages"][0]["url"] = "http://insecure/x.zip".into()).is_err());
        assert!(mutate(|v| {
            let first = v["models"][0].clone();
            v["models"].as_array_mut().unwrap().push(first);
        })
        .is_err());
        // A non-redistributable model must not point at our release.
        assert!(mutate(|v| {
            for m in v["models"].as_array_mut().unwrap() {
                if m["id"] == "codeformer" {
                    m["license"]["redistribute"] = false.into();
                }
            }
        })
        .is_err());
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(version_newer("2026.11.2", "2026.11.1"));
        assert!(version_newer("2026.12", "2026.11.9"));
        assert!(!version_newer("2026.11.1", "2026.11.1"));
        assert!(!version_newer("2026.9", "2026.11"));
    }

    #[test]
    fn cached_catalog_used_only_when_newer() {
        let dir = std::env::temp_dir().join(format!("qe-cat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(EMBEDDED_CATALOG).unwrap();
        v["catalog_version"] = "9999.1".into();
        std::fs::write(dir.join("models.json"), v.to_string()).unwrap();
        assert_eq!(Catalog::load(&dir).catalog_version, "9999.1");
        v["catalog_version"] = "1.0".into();
        std::fs::write(dir.join("models.json"), v.to_string()).unwrap();
        assert_eq!(Catalog::load(&dir).catalog_version, Catalog::embedded().catalog_version);
        std::fs::write(dir.join("models.json"), "{ broken").unwrap();
        assert_eq!(Catalog::load(&dir).catalog_version, Catalog::embedded().catalog_version);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
