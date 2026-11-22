//! State behind the Outputs page: background rescans (history + output folder), a
//! thumbnail worker with an on-disk cache (`<data>/thumbs`), and per-file metadata.
//! Nothing here blocks the UI thread.

use crossbeam_channel::{Receiver, Sender, unbounded};
use egui::TextureHandle;
use image::RgbImage;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::history::{self, EntryKind, History, OutputRecord};
use crate::media::probe::MediaInfo;
use crate::paths::{AppPaths, ToolSet};

pub const THUMB_SIZE: u32 = 256;

pub enum Thumb {
    Pending,
    Ready(TextureHandle),
    Failed,
}

struct ThumbDone {
    key: String,
    image: Option<RgbImage>,
    info: Option<MediaInfo>,
}

pub struct OutputsState {
    pub records: Vec<OutputRecord>,
    pub selected: Option<PathBuf>,
    pub search: String,
    /// Folder that was scanned (`None` = output setting is "next to source").
    pub folder: Option<PathBuf>,
    pub scanning: bool,
    pub loaded_once: bool,
    pub confirm_delete: Option<PathBuf>,
    pub thumbs: HashMap<String, Thumb>,
    /// Probed metadata of existing files (filled by the thumbnail worker).
    pub info: HashMap<String, MediaInfo>,
    scan_rx: Option<Receiver<Vec<OutputRecord>>>,
    thumb_tx: Sender<(String, PathBuf)>,
    thumb_rx: Receiver<ThumbDone>,
}

/// Cache file for a thumbnail: depends on path, size and mtime so edits invalidate it.
pub fn thumb_cache_file(dir: &Path, path: &Path) -> PathBuf {
    let md = std::fs::metadata(path).ok();
    let size = md.as_ref().map(|m| m.len()).unwrap_or(0);
    let mtime = md
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut h = Sha256::new();
    h.update(history::path_key(path).as_bytes());
    h.update(size.to_le_bytes());
    h.update(mtime.to_le_bytes());
    let digest = h.finalize();
    let name: String = digest[..12].iter().map(|b| format!("{b:02x}")).collect();
    dir.join(format!("{name}.png"))
}

fn make_thumb(tools: &ToolSet, path: &Path) -> (Option<RgbImage>, Option<MediaInfo>) {
    let info = crate::media::probe::probe(tools.ffprobe.as_deref(), path).ok();
    let img = match EntryKind::from_path(path) {
        Some(EntryKind::Image) => image::open(path)
            .ok()
            .map(|i| i.thumbnail(THUMB_SIZE, THUMB_SIZE).into_rgb8()),
        Some(EntryKind::Video) => (|| {
            let ff = tools.ffmpeg.as_ref()?;
            let i = info.as_ref()?;
            let (w, h) = crate::engines::resize::fit_within(i.width, i.height, THUMB_SIZE, THUMB_SIZE);
            let t = i.duration.map(|d| (d * 0.1).min(5.0)).unwrap_or(0.0);
            crate::media::ffmpeg::extract_frame(ff, path, t, w.max(2), h.max(2), true, None).ok()
        })(),
        None => None,
    };
    (img, info)
}

impl OutputsState {
    pub fn new(paths: &AppPaths, tools: ToolSet, ctx: egui::Context) -> OutputsState {
        let (thumb_tx, rx) = unbounded::<(String, PathBuf)>();
        let (done_tx, thumb_rx) = unbounded();
        let cache_dir = paths.thumbs.clone();
        let tools = Arc::new(tools);
        let _ = std::thread::Builder::new()
            .name("outputs-thumbs".into())
            .spawn(move || {
                let _ = std::fs::create_dir_all(&cache_dir);
                for (key, path) in rx {
                    let cache = thumb_cache_file(&cache_dir, &path);
                    let cached = image::open(&cache).ok().map(|i| i.into_rgb8());
                    let (image, info) = match cached {
                        Some(img) => (
                            Some(img),
                            crate::media::probe::probe(tools.ffprobe.as_deref(), &path).ok(),
                        ),
                        None => {
                            let (img, info) = make_thumb(&tools, &path);
                            if let Some(i) = &img {
                                let _ = i.save(&cache);
                            }
                            (img, info)
                        }
                    };
                    let _ = done_tx.send(ThumbDone { key, image, info });
                    ctx.request_repaint();
                }
            });
        OutputsState {
            records: Vec::new(),
            selected: None,
            search: String::new(),
            folder: None,
            scanning: false,
            loaded_once: false,
            confirm_delete: None,
            thumbs: HashMap::new(),
            info: HashMap::new(),
            scan_rx: None,
            thumb_tx,
            thumb_rx,
        }
    }

    /// Re-read history and rescan `folder` in the background.
    pub fn refresh(&mut self, history_file: &Path, folder: Option<PathBuf>, ctx: &egui::Context) {
        let (tx, rx) = unbounded();
        let hf = history_file.to_path_buf();
        let f2 = folder.clone();
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new()
            .name("outputs-scan".into())
            .spawn(move || {
                let h = History::load(&hf);
                let scanned = f2.as_deref().map(history::scan_folder).unwrap_or_default();
                let recs = history::merge(&h.entries, &scanned, |p| p.exists());
                let _ = tx.send(recs);
                ctx.request_repaint();
            });
        self.folder = folder;
        self.scanning = true;
        self.scan_rx = Some(rx);
    }

    /// Collect finished background work. Returns true when the list changed.
    pub fn pump(&mut self, ctx: &egui::Context) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.scan_rx
            && let Ok(recs) = rx.try_recv()
        {
            self.records = recs;
            self.scanning = false;
            self.loaded_once = true;
            self.scan_rx = None;
            changed = true;
        }
        while let Ok(d) = self.thumb_rx.try_recv() {
            if let Some(info) = d.info {
                self.info.insert(d.key.clone(), info);
            }
            let t = match d.image {
                Some(img) => Thumb::Ready(ctx.load_texture(
                    format!("out-thumb-{}", d.key),
                    crate::preview::to_color_image(&img),
                    egui::TextureOptions::LINEAR,
                )),
                None => Thumb::Failed,
            };
            self.thumbs.insert(d.key, t);
        }
        changed
    }

    /// Thumbnail for a record (queues generation on first request).
    pub fn thumb(&mut self, r: &OutputRecord) -> Option<&TextureHandle> {
        let key = history::path_key(&r.path);
        if !self.thumbs.contains_key(&key) {
            if !r.exists {
                self.thumbs.insert(key.clone(), Thumb::Failed);
            } else {
                self.thumbs.insert(key.clone(), Thumb::Pending);
                let _ = self.thumb_tx.send((key.clone(), r.path.clone()));
            }
        }
        match self.thumbs.get(&key) {
            Some(Thumb::Ready(t)) => Some(t),
            _ => None,
        }
    }

    pub fn forget(&mut self, path: &Path) {
        let key = history::path_key(path);
        self.thumbs.remove(&key);
        self.info.remove(&key);
        self.records.retain(|r| history::path_key(&r.path) != key);
        if self
            .selected
            .as_ref()
            .is_some_and(|s| history::path_key(s) == key)
        {
            self.selected = None;
        }
    }
}

/// Move a file to the Recycle Bin and drop it from history.
pub fn delete_output(path: &Path, history_file: &Path) -> Result<(), String> {
    if path.exists() {
        trash::delete(path).map_err(|e| e.to_string())?;
    }
    let mut h = History::load(history_file);
    if h.remove(path) {
        h.save(history_file).map_err(|e| e.to_string())?;
    }
    Ok(())
}
