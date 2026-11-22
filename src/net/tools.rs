//! External tools: the `tools.json` manifest (embedded; also delivered with updates), the
//! single downloader/installer used by the installer (`--install-tools`), the first-run
//! setup, "Repair / re-download" and updates, and the record of installed versions.
//!
//! Install of one component: download to `<base>/downloads/<file>.part` → verify SHA-256 →
//! rename → extract the listed files into a staging folder → move them into place. A failure
//! at any step leaves the previously installed files untouched (no half-installed tools).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::http::{self, NetError, Progress};
use crate::paths::AppPaths;
use crate::pipeline::cancel::CancelToken;

pub const EMBEDDED_MANIFEST: &str = include_str!("../../tools.json");

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FileRule {
    /// Path inside the archive. `*` matches exactly one path segment; a trailing `/` copies
    /// a whole directory (recursively).
    pub from: String,
    /// Destination relative to the tools root (`bin/…`, `models/…`, `licenses/…`).
    pub to: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Component {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub required: bool,
    pub platform: String,
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default = "default_archive")]
    pub archive: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub homepage: String,
    pub files: Vec<FileRule>,
}

fn default_archive() -> String {
    "zip".into()
}

impl Component {
    /// Archive file name (last URL segment).
    pub fn file_name(&self) -> String {
        self.url
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("download.zip")
            .to_string()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ToolsManifest {
    pub schema: u32,
    pub components: Vec<Component>,
}

pub fn current_platform() -> &'static str {
    if cfg!(all(windows, target_arch = "x86_64")) {
        "windows-x64"
    } else if cfg!(windows) {
        "windows-other"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "other"
    }
}

impl ToolsManifest {
    pub fn parse(text: &str) -> Result<ToolsManifest, NetError> {
        let m: ToolsManifest =
            serde_json::from_str(text).map_err(|e| NetError::Invalid(format!("tools.json: {e}")))?;
        for c in &m.components {
            if c.sha256.len() != 64 || !c.sha256.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return Err(NetError::Invalid(format!("tools.json: bad sha256 for {}", c.id)));
            }
            if c.archive != "zip" {
                return Err(NetError::Invalid(format!(
                    "tools.json: unsupported archive type '{}' for {}",
                    c.archive, c.id
                )));
            }
        }
        Ok(m)
    }

    pub fn embedded() -> ToolsManifest {
        Self::parse(EMBEDDED_MANIFEST).expect("embedded tools.json is valid (tested)")
    }

    /// Components available for this platform.
    pub fn for_platform(&self) -> Vec<Component> {
        self.components
            .iter()
            .filter(|c| c.platform == current_platform())
            .cloned()
            .collect()
    }
}

/// `tools-installed.json`: which component versions the downloader installed.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct InstalledTools {
    pub components: BTreeMap<String, String>,
}

impl InstalledTools {
    pub fn load(path: &Path) -> InstalledTools {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let t = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, t)?;
        std::fs::rename(&tmp, path)
    }
}

/// Components whose installed version differs from the manifest (re-download after an
/// update). Components installed manually (not recorded) are left alone.
pub fn outdated(manifest: &[Component], installed: &InstalledTools) -> Vec<String> {
    manifest
        .iter()
        .filter_map(|c| match installed.components.get(&c.id) {
            Some(v) if *v != c.version => Some(c.id.clone()),
            _ => None,
        })
        .collect()
}

/// Does archive entry `entry` match the rule pattern? Returns the path *below* the matched
/// prefix for directory rules (`Some("")` for an exact file match).
pub fn match_rule(pattern: &str, entry: &str) -> Option<String> {
    let entry = entry.replace('\\', "/");
    let dir_rule = pattern.ends_with('/');
    let pat: Vec<&str> = pattern.trim_end_matches('/').split('/').collect();
    let ent: Vec<&str> = entry.trim_end_matches('/').split('/').collect();
    if dir_rule {
        if ent.len() <= pat.len() {
            return None;
        }
    } else if ent.len() != pat.len() {
        return None;
    }
    for (p, e) in pat.iter().zip(ent.iter()) {
        if *p != "*" && !p.eq_ignore_ascii_case(e) {
            return None;
        }
    }
    if dir_rule {
        Some(ent[pat.len()..].join("/"))
    } else {
        Some(String::new())
    }
}

/// Reject `..`, absolute paths and drive letters in archive-relative paths.
fn safe_rel(p: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for seg in p.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." || seg.contains(':') {
            return None;
        }
        out.push(seg);
    }
    Some(out)
}

/// Extract the files selected by `rules` into `staging` (relative to the tools root).
/// Returns the destination-relative paths written. Fails if a rule matched nothing.
pub fn extract_selected(
    archive: &Path,
    rules: &[FileRule],
    staging: &Path,
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, NetError> {
    let f = std::fs::File::open(archive)?;
    let mut zip =
        zip::ZipArchive::new(f).map_err(|e| NetError::Invalid(format!("not a valid zip: {e}")))?;
    let mut written = Vec::new();
    let mut hits = vec![0usize; rules.len()];
    for i in 0..zip.len() {
        if cancel.is_cancelled() {
            return Err(NetError::Cancelled);
        }
        let mut entry = zip
            .by_index(i)
            .map_err(|e| NetError::Invalid(format!("zip entry {i}: {e}")))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry
            .name()
            .map(|n| n.to_string())
            .map_err(|e| NetError::Invalid(format!("zip entry name: {e}")))?;
        for (ri, rule) in rules.iter().enumerate() {
            let Some(rest) = match_rule(&rule.from, &name) else {
                continue;
            };
            let to = if rule.from.ends_with('/') {
                format!("{}/{}", rule.to.trim_end_matches('/'), rest)
            } else {
                rule.to.clone()
            };
            let Some(rel) = safe_rel(&to) else {
                return Err(NetError::Invalid(format!("unsafe path in archive: {name}")));
            };
            let dest = staging.join(&rel);
            if let Some(d) = dest.parent() {
                std::fs::create_dir_all(d)?;
            }
            let mut out = std::fs::File::create(&dest)?;
            std::io::copy(&mut entry, &mut out)?;
            written.push(rel);
            hits[ri] += 1;
            break;
        }
    }
    if let Some((ri, _)) = hits.iter().enumerate().find(|(_, n)| **n == 0) {
        return Err(NetError::Invalid(format!(
            "archive does not contain '{}' — wrong file or version?",
            rules[ri].from
        )));
    }
    Ok(written)
}

/// Move staged files into the tools root, replacing existing ones. Each file is renamed
/// individually (same volume, so it is atomic per file).
fn commit_staged(staging: &Path, root: &Path, files: &[PathBuf]) -> Result<(), NetError> {
    for rel in files {
        let src = staging.join(rel);
        let dst = root.join(rel);
        if let Some(d) = dst.parent() {
            std::fs::create_dir_all(d)?;
        }
        if dst.exists() {
            // A running tool keeps its exe locked on Windows: rename it out of the way.
            if std::fs::remove_file(&dst).is_err() {
                let old = dst.with_extension("old");
                let _ = std::fs::remove_file(&old);
                std::fs::rename(&dst, &old)?;
            }
        }
        if std::fs::rename(&src, &dst).is_err() {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// Status of one component in an install run.
#[derive(Clone, Debug, PartialEq)]
pub enum CompStatus {
    Waiting,
    Downloading(ProgressSnap),
    Verifying,
    Extracting,
    Done,
    Skipped,
    Failed(String),
    Cancelled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProgressSnap {
    pub done: u64,
    pub total: Option<u64>,
    pub speed: f64,
}

impl From<Progress> for ProgressSnap {
    fn from(p: Progress) -> Self {
        ProgressSnap {
            done: p.done,
            total: p.total,
            speed: p.speed,
        }
    }
}

/// Where a component's archive comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Download,
    /// An already-downloaded archive (offline install).
    LocalFile(PathBuf),
}

#[derive(Clone, Debug, Default)]
pub struct InstallState {
    pub running: bool,
    pub status: BTreeMap<String, CompStatus>,
    pub finished: bool,
}

/// Background install run shared with the UI.
#[derive(Clone)]
pub struct Installer {
    pub state: Arc<Mutex<InstallState>>,
    pub cancel: CancelToken,
}

impl Installer {
    pub fn snapshot(&self) -> InstallState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn is_running(&self) -> bool {
        self.state.lock().map(|s| s.running).unwrap_or(false)
    }

    /// Install `jobs` sequentially on a background thread.
    pub fn start(
        paths: AppPaths,
        jobs: Vec<(Component, Source)>,
        on_change: Option<egui::Context>,
    ) -> Installer {
        let state = Arc::new(Mutex::new(InstallState {
            running: true,
            status: jobs
                .iter()
                .map(|(c, _)| (c.id.clone(), CompStatus::Waiting))
                .collect(),
            finished: false,
        }));
        let cancel = CancelToken::new();
        let inst = Installer {
            state: state.clone(),
            cancel: cancel.clone(),
        };
        let set = move |id: &str, st: CompStatus| {
            if let Ok(mut s) = state.lock() {
                s.status.insert(id.to_string(), st);
            }
            if let Some(c) = &on_change {
                c.request_repaint();
            }
        };
        let st2 = inst.state.clone();
        let _ = std::thread::Builder::new()
            .name("tools-install".into())
            .spawn(move || {
                for (comp, src) in jobs {
                    if cancel.is_cancelled() {
                        set(&comp.id, CompStatus::Cancelled);
                        continue;
                    }
                    let r = install_one(&paths, &comp, &src, &cancel, &mut |st| set(&comp.id, st));
                    match r {
                        Ok(()) => {
                            tracing::info!("installed {} {}", comp.id, comp.version);
                            set(&comp.id, CompStatus::Done)
                        }
                        Err(NetError::Cancelled) => set(&comp.id, CompStatus::Cancelled),
                        Err(e) => {
                            tracing::error!("installing {} failed: {e}", comp.id);
                            set(&comp.id, CompStatus::Failed(e.to_string()))
                        }
                    }
                }
                if let Ok(mut s) = st2.lock() {
                    s.running = false;
                    s.finished = true;
                }
            });
        inst
    }
}

/// Download (or take the local archive), verify, extract to staging and move into place.
pub fn install_one(
    paths: &AppPaths,
    comp: &Component,
    src: &Source,
    cancel: &CancelToken,
    report: &mut dyn FnMut(CompStatus),
) -> Result<(), NetError> {
    let root = &paths.base;
    std::fs::create_dir_all(root)?;
    let archive = match src {
        Source::Download => {
            let dest = root.join("downloads").join(comp.file_name());
            let have = dest.is_file() && http::verify_sha256(&dest, &comp.sha256).is_ok();
            if !have {
                report(CompStatus::Downloading(ProgressSnap::default()));
                http::download(&comp.url, &dest, Some(&comp.sha256), cancel, &mut |p| {
                    report(CompStatus::Downloading(p.into()))
                })?;
            }
            dest
        }
        Source::LocalFile(p) => {
            report(CompStatus::Verifying);
            http::verify_sha256(p, &comp.sha256)?;
            p.clone()
        }
    };
    report(CompStatus::Extracting);
    let staging = root.join(format!(".staging-{}", comp.id));
    let _ = std::fs::remove_dir_all(&staging);
    let result = extract_selected(&archive, &comp.files, &staging, cancel)
        .and_then(|files| commit_staged(&staging, root, &files));
    let _ = std::fs::remove_dir_all(&staging);
    result?;
    // Downloaded archives are not needed any more.
    if matches!(src, Source::Download) {
        let _ = std::fs::remove_file(&archive);
    }
    let mut state = InstalledTools::load(&paths.tools_state_file);
    state
        .components
        .insert(comp.id.clone(), comp.version.clone());
    state.save(&paths.tools_state_file)?;
    Ok(())
}

/// Offline install: find each component's archive (by its file name) in a folder.
pub fn find_archives_in(dir: &Path, comps: &[Component]) -> Vec<(String, PathBuf)> {
    comps
        .iter()
        .filter_map(|c| {
            let p = dir.join(c.file_name());
            p.is_file().then(|| (c.id.clone(), p))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn embedded_manifest_is_valid() {
        let m = ToolsManifest::embedded();
        assert!(m.components.iter().any(|c| c.id == "ffmpeg" && c.required));
        for c in &m.components {
            assert!(c.url.starts_with("https://"), "{}", c.url);
            assert!(!c.url.contains("/latest/"), "URLs must be pinned: {}", c.url);
            assert!(!c.files.is_empty());
        }
    }

    #[test]
    fn manifest_rejects_bad_hash_and_archive() {
        let bad = r#"{"schema":1,"components":[{"id":"x","name":"x","version":"1","platform":"windows-x64","url":"https://a/b.zip","sha256":"abc","files":[]}]}"#;
        assert!(ToolsManifest::parse(bad).is_err());
        let seven = format!(
            r#"{{"schema":1,"components":[{{"id":"x","name":"x","version":"1","platform":"windows-x64","url":"https://a/b.7z","sha256":"{}","archive":"7z","files":[]}}]}}"#,
            "a".repeat(64)
        );
        assert!(ToolsManifest::parse(&seven).is_err());
    }

    #[test]
    fn rule_matching() {
        assert_eq!(match_rule("*/bin/ffmpeg.exe", "ffmpeg-9.1/bin/ffmpeg.exe"), Some(String::new()));
        assert_eq!(match_rule("*/bin/ffmpeg.exe", "ffmpeg-9.1/bin/ffplay.exe"), None);
        assert_eq!(match_rule("*/bin/ffmpeg.exe", "bin/ffmpeg.exe"), None);
        assert_eq!(match_rule("models/", "models/a.param"), Some("a.param".into()));
        assert_eq!(match_rule("*/rife-v4.6/", "r/rife-v4.6/flownet.bin"), Some("flownet.bin".into()));
        assert_eq!(match_rule("*/rife-v4.6/", "r/rife-v4/flownet.bin"), None);
        assert_eq!(match_rule("models/", "models/"), None);
    }

    #[test]
    fn outdated_only_for_recorded_versions() {
        let m = ToolsManifest::embedded().components;
        let mut st = InstalledTools::default();
        assert!(outdated(&m, &st).is_empty());
        st.components.insert("ffmpeg".into(), "0.0-old".into());
        let ff = m.iter().find(|c| c.id == "ffmpeg").unwrap();
        st.components.insert("rife".into(), m.iter().find(|c| c.id == "rife").unwrap().version.clone());
        assert_eq!(outdated(&m, &st), vec![ff.id.clone()]);
    }

    fn make_zip(path: &Path, files: &[(&str, &[u8])]) {
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in files {
            z.start_file(*name, opts).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn install_from_local_archive_verifies_and_extracts() {
        let d = std::env::temp_dir().join(format!("qe-tools-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let zip_path = d.join("tool.zip");
        make_zip(
            &zip_path,
            &[
                ("pkg-1.0/bin/tool.exe", b"EXE"),
                ("pkg-1.0/bin/other.exe", b"NO"),
                ("pkg-1.0/models/m1/a.param", b"P"),
                ("pkg-1.0/models/m1/a.bin", b"B"),
            ],
        );
        let sha = http::sha256_file(&zip_path).unwrap();
        let comp = Component {
            id: "tool".into(),
            name: "Tool".into(),
            version: "1.0".into(),
            required: false,
            platform: current_platform().into(),
            url: "https://example.invalid/tool.zip".into(),
            sha256: sha.clone(),
            size: 0,
            archive: "zip".into(),
            license: String::new(),
            homepage: String::new(),
            files: vec![
                FileRule { from: "*/bin/tool.exe".into(), to: "bin/tool.exe".into() },
                FileRule { from: "*/models/m1/".into(), to: "models/x/m1/".into() },
            ],
        };
        let paths = AppPaths::from_base(d.join("root"));
        let mut seen = Vec::new();
        install_one(&paths, &comp, &Source::LocalFile(zip_path.clone()), &CancelToken::new(), &mut |s| seen.push(s)).unwrap();
        assert_eq!(std::fs::read(paths.bin.join("tool.exe")).unwrap(), b"EXE");
        assert!(!paths.bin.join("other.exe").exists());
        assert_eq!(std::fs::read(paths.models.join("x/m1/a.bin")).unwrap(), b"B");
        assert!(!d.join("root").join(".staging-tool").exists());
        assert_eq!(InstalledTools::load(&paths.tools_state_file).components.get("tool"), Some(&"1.0".to_string()));
        assert!(seen.contains(&CompStatus::Verifying) && seen.contains(&CompStatus::Extracting));

        // Wrong checksum: nothing is touched.
        std::fs::write(paths.bin.join("tool.exe"), b"KEEP").unwrap();
        let mut bad = comp.clone();
        bad.sha256 = "0".repeat(64);
        let r = install_one(&paths, &bad, &Source::LocalFile(zip_path.clone()), &CancelToken::new(), &mut |_| {});
        assert!(matches!(r, Err(NetError::Checksum { .. })));
        assert_eq!(std::fs::read(paths.bin.join("tool.exe")).unwrap(), b"KEEP");

        // A rule that matches nothing fails before anything is moved.
        let mut missing = comp.clone();
        missing.files.push(FileRule { from: "*/nope.dll".into(), to: "bin/nope.dll".into() });
        let r = install_one(&paths, &missing, &Source::LocalFile(zip_path), &CancelToken::new(), &mut |_| {});
        assert!(matches!(r, Err(NetError::Invalid(_))));
        assert_eq!(std::fs::read(paths.bin.join("tool.exe")).unwrap(), b"KEEP");
        let _ = std::fs::remove_dir_all(&d);
    }
}
