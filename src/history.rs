//! Output history (`history.json` in the data dir) and the merged view used by the
//! Outputs page: history entries + a scan of the output folder, de-duplicated by path,
//! with entries whose file no longer exists marked as missing.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::media::{IMAGE_EXTS, VIDEO_EXTS};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Image,
    Video,
}

impl EntryKind {
    pub fn from_path(p: &Path) -> Option<EntryKind> {
        let ext = p.extension()?.to_string_lossy().to_ascii_lowercase();
        if VIDEO_EXTS.contains(&ext.as_str()) {
            Some(EntryKind::Video)
        } else if IMAGE_EXTS.contains(&ext.as_str()) {
            Some(EntryKind::Image)
        } else {
            None
        }
    }
}

/// One successful job.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct HistoryEntry {
    pub output: PathBuf,
    pub source: Option<PathBuf>,
    pub kind: EntryKind,
    /// Unix seconds (UTC).
    pub created: i64,
    pub profile: String,
    pub scale: String,
    /// e.g. "24 → 48 fps"; `None` for images.
    pub fps: Option<String>,
    pub encoder: Option<String>,
    pub width: u32,
    pub height: u32,
    /// Media duration (videos).
    pub duration_secs: Option<f64>,
    pub size_bytes: u64,
    pub processing_secs: f64,
    /// Produced by the Editor page (trim / crop / join / rotate).
    pub edited: bool,
    /// Made with a generative model ("AI-generated details").
    pub ai_generated: bool,
}

impl Default for HistoryEntry {
    fn default() -> Self {
        Self {
            output: PathBuf::new(),
            source: None,
            kind: EntryKind::Image,
            created: 0,
            profile: String::new(),
            scale: String::new(),
            fps: None,
            encoder: None,
            width: 0,
            height: 0,
            duration_secs: None,
            size_bytes: 0,
            processing_secs: 0.0,
            edited: false,
            ai_generated: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct History {
    pub version: u32,
    pub entries: Vec<HistoryEntry>,
}

/// Case-insensitive on Windows, separator-normalised path key for de-duplication.
pub fn path_key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.trim_end_matches('/').to_string();
    if cfg!(windows) { s.to_lowercase() } else { s }
}

impl History {
    pub fn load(path: &Path) -> History {
        let Ok(text) = std::fs::read_to_string(path) else {
            return History::default();
        };
        match serde_json::from_str::<History>(&text) {
            Ok(h) => h,
            Err(e) => {
                // Keep the unreadable file for inspection instead of silently losing it.
                let bak = path.with_extension("json.corrupt");
                let _ = std::fs::rename(path, &bak);
                tracing::warn!("history.json unreadable ({e}); moved to {}", bak.display());
                History::default()
            }
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let text = serde_json::to_string_pretty(&History {
            version: 1,
            entries: self.entries.clone(),
        })
        .map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }

    /// Add (or replace an older entry for the same output path).
    pub fn append(&mut self, e: HistoryEntry) {
        let k = path_key(&e.output);
        self.entries.retain(|x| path_key(&x.output) != k);
        self.entries.push(e);
    }

    pub fn remove(&mut self, output: &Path) -> bool {
        let k = path_key(output);
        let n = self.entries.len();
        self.entries.retain(|x| path_key(&x.output) != k);
        n != self.entries.len()
    }

    /// Load, append, save — used after every successful job.
    pub fn append_to_file(path: &Path, e: HistoryEntry) -> std::io::Result<()> {
        let mut h = History::load(path);
        h.append(e);
        h.save(path)
    }
}

/// A media file found in the output folder.
#[derive(Clone, Debug, PartialEq)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub kind: EntryKind,
    pub size: u64,
    /// Unix seconds.
    pub modified: i64,
}

/// Media files directly inside `dir` (not recursive).
pub fn scan_folder(dir: &Path) -> Vec<ScannedFile> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.flatten()
        .filter_map(|e| {
            let path = e.path();
            let kind = EntryKind::from_path(&path)?;
            let md = e.metadata().ok().filter(|m| m.is_file())?;
            let modified = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            Some(ScannedFile {
                path,
                kind,
                size: md.len(),
                modified,
            })
        })
        .collect()
}

/// One row of the Outputs page.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputRecord {
    pub path: PathBuf,
    pub kind: EntryKind,
    /// Present when the app produced this file (has source + settings).
    pub entry: Option<HistoryEntry>,
    pub exists: bool,
    pub in_folder: bool,
    pub size: u64,
    /// Unix seconds: history date, else file modification time.
    pub date: i64,
}

impl OutputRecord {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    }
}

/// Merge history and a folder scan. Files present in both keep the history details and
/// the scanned size; history entries whose file is gone are kept with `exists = false`.
pub fn merge(
    history: &[HistoryEntry],
    scanned: &[ScannedFile],
    exists: impl Fn(&Path) -> bool,
) -> Vec<OutputRecord> {
    let mut by_key: HashMap<String, OutputRecord> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for e in history {
        let k = path_key(&e.output);
        if !by_key.contains_key(&k) {
            order.push(k.clone());
        }
        by_key.insert(
            k,
            OutputRecord {
                path: e.output.clone(),
                kind: e.kind,
                exists: exists(&e.output),
                in_folder: false,
                size: e.size_bytes,
                date: e.created,
                entry: Some(e.clone()),
            },
        );
    }
    for f in scanned {
        let k = path_key(&f.path);
        match by_key.get_mut(&k) {
            Some(r) => {
                r.exists = true;
                r.in_folder = true;
                r.size = f.size;
            }
            None => {
                order.push(k.clone());
                by_key.insert(
                    k,
                    OutputRecord {
                        path: f.path.clone(),
                        kind: f.kind,
                        entry: None,
                        exists: true,
                        in_folder: true,
                        size: f.size,
                        date: f.modified,
                    },
                );
            }
        }
    }
    order
        .into_iter()
        .filter_map(|k| by_key.remove(&k))
        .collect()
}

/// Local date/time for display.
pub fn format_date(unix: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_opt(unix, 0).single() {
        Some(d) => d.format("%Y-%m-%d %H:%M").to_string(),
        None => "—".into(),
    }
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(p: &str, created: i64) -> HistoryEntry {
        HistoryEntry {
            output: PathBuf::from(p),
            source: Some(PathBuf::from("src.mp4")),
            kind: EntryKind::Video,
            created,
            profile: "Anime".into(),
            size_bytes: 10,
            ..Default::default()
        }
    }

    #[test]
    fn history_roundtrip_and_replace() {
        let dir = std::env::temp_dir().join(format!("qe-hist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("history.json");
        assert!(History::load(&file).entries.is_empty());
        History::append_to_file(&file, entry("C:/out/a.mp4", 1)).unwrap();
        History::append_to_file(&file, entry("C:/out/b.mp4", 2)).unwrap();
        // Same output again (different case / separators on Windows) replaces the old entry.
        let again = if cfg!(windows) { "c:\\OUT\\a.mp4" } else { "C:/out/a.mp4" };
        History::append_to_file(&file, entry(again, 3)).unwrap();
        let h = History::load(&file);
        assert_eq!(h.entries.len(), 2);
        assert_eq!(h.entries.last().unwrap().created, 3);
        let mut h2 = h.clone();
        assert!(h2.remove(Path::new("C:/out/b.mp4")));
        assert_eq!(h2.entries.len(), 1);
        // A corrupt file is moved aside, not fatal.
        std::fs::write(&file, "{ not json").unwrap();
        assert!(History::load(&file).entries.is_empty());
        assert!(dir.join("history.json.corrupt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_dedups_and_marks_missing() {
        let hist = vec![entry("C:/out/a.mp4", 100), entry("C:/out/gone.mp4", 50)];
        let scanned = vec![
            ScannedFile {
                path: PathBuf::from(if cfg!(windows) { "C:\\out\\A.mp4" } else { "C:/out/a.mp4" }),
                kind: EntryKind::Video,
                size: 999,
                modified: 7,
            },
            ScannedFile {
                path: PathBuf::from("C:/out/other.png"),
                kind: EntryKind::Image,
                size: 5,
                modified: 200,
            },
        ];
        let exists = |p: &Path| !p.to_string_lossy().contains("gone");
        let m = merge(&hist, &scanned, exists);
        assert_eq!(m.len(), 3, "{m:#?}");
        let a = m.iter().find(|r| r.name().eq_ignore_ascii_case("a.mp4")).unwrap();
        assert!(a.exists && a.in_folder && a.entry.is_some());
        assert_eq!(a.size, 999, "scanned size wins");
        assert_eq!(a.date, 100, "history date wins");
        let gone = m.iter().find(|r| r.name() == "gone.mp4").unwrap();
        assert!(!gone.exists && gone.entry.is_some());
        let other = m.iter().find(|r| r.name() == "other.png").unwrap();
        assert!(other.exists && other.entry.is_none() && other.kind == EntryKind::Image);
    }

    #[test]
    fn scan_finds_only_media() {
        let dir = std::env::temp_dir().join(format!("qe-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("x.mp4"), b"1234").unwrap();
        std::fs::write(dir.join("y.PNG"), b"12").unwrap();
        std::fs::write(dir.join("notes.txt"), b"1").unwrap();
        let mut s = scan_folder(&dir);
        s.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].kind, EntryKind::Video);
        assert_eq!(s[0].size, 4);
        assert_eq!(s[1].kind, EntryKind::Image);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
