//! Top-level eframe application state and event pumping.

use crossbeam_channel::{Receiver, Sender, unbounded};
use egui::TextureHandle;
use image::RgbImage;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::config::{Config, JobSettings, OverwritePolicy, Studio};
use crate::devices::{self, GpuInfo};
use crate::logging::UiLog;
use crate::media::encoders::{self, EncoderAvailability};
use crate::media::probe::{self, MediaInfo};
use crate::media::{MediaKind, detect_kind};
use crate::monitor::Monitor;
use crate::paths::{AppPaths, ModelSet, SetupReport, ToolSet};
use crate::pipeline::image_job::resolve_format;
use crate::pipeline::job::{Env, JobOutcome, JobSpec};
use crate::pipeline::runner::{Runner, RunnerEvent};
use crate::preview::canvas::PreviewCanvas;
use crate::preview::player::SamplePlayer;
use crate::preview::worker::{PreviewKind, PreviewRequest, PreviewResult, PreviewWorker};
use crate::t;

/// Main window page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// All-in-one (the 2.0 queue page).
    Queue,
    Video,
    Image,
    Editor,
    Models,
    Outputs,
    Settings,
}

impl Page {
    pub fn studio(self) -> Option<Studio> {
        match self {
            Page::Queue => Some(Studio::All),
            Page::Video => Some(Studio::Video),
            Page::Image => Some(Studio::Image),
            _ => None,
        }
    }
    pub fn of(s: Studio) -> Page {
        match s {
            Studio::All => Page::Queue,
            Studio::Video => Page::Video,
            Studio::Image => Page::Image,
        }
    }
}

/// Inputs + preview state of one studio page (All-in-one / Video / Image).
pub struct Workspace {
    pub items: Vec<QueueItem>,
    pub selected: Option<u64>,
    pub canvas: PreviewCanvas,
    pub player: Option<SamplePlayer>,
    pub preview_note: String,
    pub preview_error: Option<String>,
    pub preview_dirty: Option<Instant>,
    pub last_preview: Option<(u64, JobSettings, u64)>,
    pub scrub_time: f64,
    pub preview_secs: f64,
    /// Dropped files of the wrong kind for this page (offered to the other page).
    pub rejected: Vec<PathBuf>,
}

impl Workspace {
    pub fn new() -> Workspace {
        Workspace {
            items: Vec::new(),
            selected: None,
            canvas: PreviewCanvas::new(),
            player: None,
            preview_note: String::new(),
            preview_error: None,
            preview_dirty: None,
            last_preview: None,
            scrub_time: 0.0,
            preview_secs: 0.0,
            rejected: Vec::new(),
        }
    }

    pub fn selected_item(&self) -> Option<&QueueItem> {
        self.selected.and_then(|id| self.items.iter().find(|i| i.id == id))
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Workspace::new()
    }
}

/// How the app was started.
#[derive(Default)]
pub struct StartOptions {
    pub files: Vec<PathBuf>,
    /// `--install-tools`: show only the tools downloader (run by the installer).
    pub install_tools: bool,
    /// `--font-test`: open the Thai rendering test window.
    pub font_test: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemStatus {
    Probing,
    Pending,
    Running,
    Done,
    Failed,
    Cancelled,
    Skipped,
    Invalid,
}

impl ItemStatus {
    pub fn label(self) -> String {
        match self {
            ItemStatus::Probing => t!("status.probing"),
            ItemStatus::Pending => t!("status.pending"),
            ItemStatus::Running => t!("status.running"),
            ItemStatus::Done => t!("status.done"),
            ItemStatus::Failed => t!("status.failed"),
            ItemStatus::Cancelled => t!("status.cancelled"),
            ItemStatus::Skipped => t!("status.skipped"),
            ItemStatus::Invalid => t!("status.unreadable"),
        }
    }
}

pub struct QueueItem {
    pub id: u64,
    pub path: PathBuf,
    pub info: Option<MediaInfo>,
    pub thumb: Option<TextureHandle>,
    pub status: ItemStatus,
    /// `None` = follow the current settings; `Some` = pinned snapshot ("Apply settings").
    pub settings: Option<JobSettings>,
    /// Page the item was added on (its settings come from that page).
    pub origin: Studio,
    pub checked: bool,
    pub output: Option<PathBuf>,
    pub message: String,
}

impl QueueItem {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    }
    pub fn kind(&self) -> Option<MediaKind> {
        self.info.as_ref().map(|i| i.kind)
    }
}

pub(crate) struct ProbeDone {
    id: u64,
    result: Result<MediaInfo, String>,
    thumb: Option<RgbImage>,
}

/// Pending "output already exists" decision.
pub struct OverwritePrompt {
    pub jobs: Vec<(u64, PathBuf)>,
    pub conflicts: Vec<PathBuf>,
}

pub struct App {
    pub paths: AppPaths,
    pub config: Config,
    pub env: Arc<Env>,
    pub setup: SetupReport,
    pub show_setup: bool,
    pub vulkan_error: Option<String>,
    pub monitor: Monitor,
    pub runner: Runner,
    pub preview: PreviewWorker,
    pub ui_log: UiLog,

    /// Active studio page's workspace; the other two are parked in `parked`.
    pub ws: Workspace,
    parked: [Option<Workspace>; 3],
    /// Studio the active workspace belongs to (the last studio page shown).
    pub studio: Studio,
    next_id: u64,
    probe_tx: Sender<(u64, PathBuf)>,
    probe_rx: Receiver<ProbeDone>,

    pub toasts: Vec<(Instant, String)>,
    pub overwrite_prompt: Option<OverwritePrompt>,
    pub queue_total: usize,
    pub queue_done: usize,
    saved_config: Config,
    last_save: Instant,

    pub egui_ctx: egui::Context,
    pub page: Page,
    pub outputs: crate::outputs::OutputsState,
    pub tools_ui: crate::ui::tools::ToolsUi,
    pub models_ui: crate::ui::models::ModelsUi,
    pub wizard: crate::ui::wizard::Wizard,
    pub update_ui: crate::ui::update::UpdateUi,
    pub show_font_test: bool,
    /// Started with `--install-tools`: only the tools page is shown.
    pub tools_only: bool,
    /// First run: ask once whether to check for updates automatically.
    pub ask_update_optin: bool,
    job_started: HashMap<u64, Instant>,
    history_tx: Sender<()>,
    history_rx: Receiver<()>,
    pub outputs_dirty: bool,
    pub profiles: crate::profiles::ProfileStore,
    pub profile_dialog: Option<crate::ui::profiles::ProfileDialog>,
    pub bench: crate::bench::BenchStore,
    pub bench_run: Option<crate::bench::BenchRun>,
    pub bench_offer: bool,
    pub tune_report: Option<Vec<crate::resources::Change>>,
    pub preflight: Option<crate::ui::perf::Preflight>,
    sys_info: crate::resources::SystemInfo,
    sys_at: Instant,
    pub editor: crate::ui::editor::EditorState,
}

/// Noto Sans Thai (SIL OFL 1.1, see licenses/NotoSansThai-OFL.txt): egui's bundled fonts
/// have no Thai glyphs.
const THAI_FONT: &[u8] = include_bytes!("../assets/fonts/NotoSansThai-Regular.ttf");

/// egui's bundled fonts lack arrows / warning signs / media symbols; add a system symbol
/// font as a low-priority fallback when one is available.
fn install_fallback_fonts(ctx: &egui::Context) {
    use egui::epaint::text::{FontInsert, InsertFontFamily};
    ctx.add_font(FontInsert::new(
        "noto-sans-thai",
        egui::FontData::from_static(THAI_FONT),
        vec![
            InsertFontFamily {
                family: egui::FontFamily::Proportional,
                priority: egui::epaint::text::FontPriority::Lowest,
            },
            InsertFontFamily {
                family: egui::FontFamily::Monospace,
                priority: egui::epaint::text::FontPriority::Lowest,
            },
        ],
    ));
    let candidates = [
        r"C:\Windows\Fonts\seguisym.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    ];
    for c in candidates {
        if let Ok(bytes) = std::fs::read(c) {
            ctx.add_font(FontInsert::new(
                "symbols-fallback",
                egui::FontData::from_owned(bytes),
                vec![
                    InsertFontFamily {
                        family: egui::FontFamily::Proportional,
                        priority: egui::epaint::text::FontPriority::Lowest,
                    },
                    InsertFontFamily {
                        family: egui::FontFamily::Monospace,
                        priority: egui::epaint::text::FontPriority::Lowest,
                    },
                ],
            ));
            break;
        }
    }
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        paths: AppPaths,
        config: Config,
        ui_log: UiLog,
        start: StartOptions,
    ) -> App {
        let ctx = cc.egui_ctx.clone();
        install_fallback_fonts(&ctx);
        let tools = ToolSet::discover(&paths);
        let models = ModelSet::discover(&paths.models);
        let setup = SetupReport::check(&paths, &tools, &models);
        let (gpus, vulkan_error) = match devices::enumerate() {
            Ok(g) => {
                for gpu in &g {
                    tracing::info!(
                        "Vulkan device {}: {} ({:?}, {:.1} GB, fp16={})",
                        gpu.index,
                        gpu.name,
                        gpu.kind,
                        gpu.vram_bytes as f64 / (1u64 << 30) as f64,
                        gpu.fp16
                    );
                }
                (g, None)
            }
            Err(e) => {
                tracing::warn!("{e} — GPU features disabled");
                (Vec::new(), Some(e))
            }
        };
        let encoders = detect_encoders_bg(tools.ffmpeg.clone(), &ctx);
        let monitor = Monitor::start(Some(ctx.clone()));
        let env = Arc::new(Env {
            tools,
            models,
            gpus,
            encoders,
            monitor: Some(monitor.clone()),
            cores: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4),
        });
        let runner = Runner::start(env.clone(), Some(ctx.clone()));
        let preview = PreviewWorker::start(env.clone(), Some(ctx.clone()));
        let (probe_tx, probe_rx) = spawn_probe_worker(env.clone(), ctx.clone());
        let outputs = crate::outputs::OutputsState::new(&paths, env.tools.clone(), ctx.clone());
        let (history_tx, history_rx) = unbounded();

        let profiles_dir = paths.data.join("profiles");
        let bench_file = crate::bench::store_file(&paths.data);
        let tools_only = start.install_tools;
        // Missing tools: open the downloader (the setup check stays available).
        // An update may pin newer tool versions: offer the re-download (only components
        // the downloader installed and whose version changed).
        let outdated = crate::net::tools::outdated(
            &crate::net::tools::ToolsManifest::embedded().for_platform(),
            &crate::net::tools::InstalledTools::load(&paths.tools_state_file),
        );
        if !outdated.is_empty() {
            tracing::info!("tools with a new pinned version: {}", outdated.join(", "));
        }
        let missing_tools = (!setup.all_present() || !outdated.is_empty()) && !tools_only;
        let show_setup = false;
        let ask_update_optin = !config.update_check_asked && !tools_only;
        let auto_check = config.auto_update_check && !tools_only;
        let models_ui = crate::ui::models::ModelsUi::new(&paths);
        let mut app = App {
            paths,
            saved_config: config.clone(),
            config,
            env,
            setup,
            show_setup,
            vulkan_error,
            monitor,
            runner,
            preview,
            ui_log,
            ws: Workspace::new(),
            parked: [None, None, None],
            studio: Studio::All,
            next_id: 1,
            probe_tx,
            probe_rx,
            toasts: Vec::new(),
            overwrite_prompt: None,
            queue_total: 0,
            queue_done: 0,
            last_save: Instant::now(),
            egui_ctx: ctx.clone(),
            page: Page::Queue,
            outputs,
            tools_ui: crate::ui::tools::ToolsUi::default(),
            models_ui,
            wizard: Default::default(),
            update_ui: crate::ui::update::UpdateUi::default(),
            show_font_test: start.font_test,
            tools_only,
            ask_update_optin,
            job_started: HashMap::new(),
            history_tx,
            history_rx,
            outputs_dirty: true,
            profiles: crate::profiles::ProfileStore::load(&profiles_dir),
            profile_dialog: None,
            bench: crate::bench::BenchStore::load(&bench_file),
            bench_run: None,
            bench_offer: false,
            tune_report: None,
            preflight: None,
            sys_info: Default::default(),
            sys_at: Instant::now(),
            editor: Default::default(),
        };
        app.refresh_sys();
        crate::ui::profiles::apply_defaults(&mut app);
        app.sync_monitor_gpu();
        // First run (and the installer's `--install-tools`): the setup wizard. Later starts
        // with missing or outdated tools: the classic tools list.
        if tools_only || (missing_tools && !app.config.wizard_done) {
            app.wizard.open();
        } else if missing_tools {
            app.tools_ui.open(&app.paths);
        }
        if auto_check {
            app.update_ui.start_check(&ctx, true);
        }
        if !start.files.is_empty() {
            app.add_paths(start.files);
        }
        app
    }

    pub fn active_gpu(&self) -> Option<&GpuInfo> {
        devices::resolve(self.config.machine.compute, &self.env.gpus)
    }

    pub fn sync_monitor_gpu(&self) {
        self.monitor.set_active_gpu(self.active_gpu().cloned());
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        let m = msg.into();
        tracing::info!("{m}");
        self.toasts.push((Instant::now(), m));
    }

    pub fn selected_item(&self) -> Option<&QueueItem> {
        self.ws.selected_item()
    }

    /// Settings of the active studio page.
    pub fn cur_settings(&self) -> &JobSettings {
        self.config.settings_for(self.studio)
    }

    /// Show a studio page, swapping its workspace in.
    pub fn switch_studio(&mut self, to: Studio) {
        self.page = Page::of(to);
        if to == self.studio {
            return;
        }
        self.preview.cancel();
        let mut old = std::mem::replace(
            &mut self.ws,
            self.parked[to.index()].take().unwrap_or_default(),
        );
        // A parked sample player would keep playing audio.
        old.player = None;
        old.preview_dirty = None;
        old.last_preview = None;
        self.parked[self.studio.index()] = Some(old);
        self.studio = to;
        if self.config.auto_preview && self.ws.selected.is_some() {
            self.mark_preview_dirty();
        }
    }

    /// Queue item by id, in whichever page holds it.
    pub fn item_mut(&mut self, id: u64) -> Option<&mut QueueItem> {
        if let Some(i) = self.ws.items.iter_mut().find(|i| i.id == id) {
            return Some(i);
        }
        self.parked
            .iter_mut()
            .flatten()
            .flat_map(|w| w.items.iter_mut())
            .find(|i| i.id == id)
    }

    pub fn item(&self, id: u64) -> Option<&QueueItem> {
        self.ws
            .items
            .iter()
            .chain(self.parked.iter().flatten().flat_map(|w| w.items.iter()))
            .find(|i| i.id == id)
    }

    /// (studio, pending + running) counts of items in other pages.
    pub fn other_pages_busy(&self) -> Vec<(Studio, usize)> {
        Studio::ALL
            .iter()
            .filter(|s| **s != self.studio)
            .filter_map(|s| {
                let w = self.parked[s.index()].as_ref()?;
                let n = w
                    .items
                    .iter()
                    .filter(|i| i.status == ItemStatus::Running || (i.status == ItemStatus::Pending && i.output.is_some()))
                    .count();
                (n > 0).then_some((*s, n))
            })
            .collect()
    }

    // ---- input ------------------------------------------------------------------------

    pub fn add_paths(&mut self, paths: Vec<PathBuf>) {
        let mut files = Vec::new();
        for p in paths {
            if p.is_dir() {
                collect_dir(&p, self.config.recursive, &mut files);
            } else {
                files.push(p);
            }
        }
        let existing: HashSet<PathBuf> = self.ws.items.iter().map(|i| i.path.clone()).collect();
        let mut skipped = Vec::new();
        let mut rejected = Vec::new();
        let mut added = 0;
        for f in files {
            if existing.contains(&f) {
                continue;
            }
            if let Some(k) = detect_kind(&f)
                && !self.studio.accepts(k)
            {
                rejected.push(f);
                continue;
            }
            if detect_kind(&f).is_none() {
                skipped.push(
                    f.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default(),
                );
                continue;
            }
            let id = self.alloc_id();
            self.ws.items.push(QueueItem {
                id,
                path: f.clone(),
                info: None,
                thumb: None,
                status: ItemStatus::Probing,
                settings: None,
                origin: self.studio,
                checked: false,
                output: None,
                message: String::new(),
            });
            let _ = self.probe_tx.send((id, f));
            added += 1;
            if self.ws.selected.is_none() {
                self.ws.selected = Some(id);
            }
        }
        if added > 0 {
            self.toast(t!("toast.added", n = added));
        }
        if !rejected.is_empty() {
            self.ws.rejected = rejected;
        }
        if !skipped.is_empty() {
            let shown: Vec<_> = skipped.iter().take(5).cloned().collect();
            self.toast(t!(
                "toast.skipped_unsupported",
                n = skipped.len(),
                names = format!("{}{}", shown.join(", "), if skipped.len() > 5 { ", …" } else { "" })
            ));
        }
    }

    pub fn remove_item(&mut self, id: u64) {
        self.ws.items
            .retain(|i| i.id != id || i.status == ItemStatus::Running);
        if self.ws.selected == Some(id) {
            self.ws.selected = self.ws.items.first().map(|i| i.id);
            self.ws.canvas.clear();
            self.ws.player = None;
            self.ws.last_preview = None;
        }
    }

    pub fn move_item(&mut self, id: u64, delta: isize) {
        if let Some(pos) = self.ws.items.iter().position(|i| i.id == id) {
            let np = (pos as isize + delta).clamp(0, self.ws.items.len() as isize - 1) as usize;
            self.ws.items.swap(pos, np);
        }
    }

    pub fn apply_settings(&mut self, only_checked: bool) {
        let s = self.cur_settings().clone();
        let mut n = 0;
        for i in &mut self.ws.items {
            if i.status != ItemStatus::Running && (!only_checked || i.checked) {
                i.settings = Some(s.clone());
                n += 1;
            }
        }
        self.toast(t!("toast.pinned", n = n));
    }

    // ---- queue ------------------------------------------------------------------------

    pub fn output_path_for(&self, item: &QueueItem) -> Option<PathBuf> {
        let info = item.info.as_ref()?;
        let s = &item
            .settings
            .as_ref()
            .unwrap_or(self.config.settings_for(item.origin))
            .output;
        let dir = s
            .folder
            .clone()
            .or_else(|| item.path.parent().map(Path::to_path_buf))?;
        let stem = item.path.file_stem()?.to_string_lossy().to_string();
        let ext = match info.kind {
            MediaKind::Image => resolve_format(s.image_format, &item.path).ext(),
            MediaKind::Video => s.container.ext(),
        };
        Some(dir.join(format!("{stem}{}.{ext}", s.safe_suffix())))
    }

    /// New job id (shared by queue items and editor jobs).
    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// History entry for an Editor export (shown as "Edited" on the Outputs page).
    pub fn record_edit_history(&mut self, output: &Path, kind: MediaKind, note: &str) {
        let ffprobe = self.env.tools.ffprobe.clone();
        let file = self.paths.history_file.clone();
        let output = output.to_path_buf();
        let tx = self.history_tx.clone();
        let note = note.to_string();
        let _ = std::thread::Builder::new().name("history".into()).spawn(move || {
            let info = probe::probe(ffprobe.as_deref(), &output).ok();
            let entry = crate::history::HistoryEntry {
                output: output.clone(),
                source: None,
                kind: match kind {
                    MediaKind::Video => crate::history::EntryKind::Video,
                    MediaKind::Image => crate::history::EntryKind::Image,
                },
                created: crate::history::now_unix(),
                profile: note,
                scale: "1x".into(),
                fps: info.as_ref().and_then(|i| i.fps).map(|f| format!("{f} fps")),
                encoder: None,
                width: info.as_ref().map(|i| i.width).unwrap_or(0),
                height: info.as_ref().map(|i| i.height).unwrap_or(0),
                duration_secs: info.as_ref().and_then(|i| i.duration),
                size_bytes: std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0),
                processing_secs: 0.0,
                edited: true,
            };
            if let Err(e) = crate::history::History::append_to_file(&file, entry) {
                tracing::warn!("writing history failed: {e}");
            }
            let _ = tx.send(());
        });
    }

    /// Cached system info for estimates (refreshed every few seconds in `pump`).
    pub fn sys(&self) -> &crate::resources::SystemInfo {
        &self.sys_info
    }

    pub fn refresh_sys(&mut self) {
        self.sys_info = crate::resources::gather(&self.env, &self.config.machine);
        self.sys_at = Instant::now();
    }

    /// Snapshot for `settings::after_edit`.
    pub fn cur_and_machine(&self) -> (JobSettings, crate::config::MachineSettings) {
        (self.cur_settings().clone(), self.config.machine.clone())
    }

    pub fn start_benchmark(&mut self) {
        if self.bench_run.is_some() || self.runner.is_busy() {
            return;
        }
        self.preview.cancel();
        self.bench_run = Some(crate::bench::BenchRun::start(
            self.env.clone(),
            self.config.machine.clone(),
            self.egui_ctx.clone(),
        ));
    }

    fn pump_bench(&mut self) {
        let Some(run) = &self.bench_run else {
            return;
        };
        let Ok(res) = run.done.try_recv() else {
            return;
        };
        self.bench_run = None;
        match res {
            Ok(r) => {
                self.bench.put(r);
                if let Err(e) = self.bench.save(&crate::bench::store_file(&self.paths.data)) {
                    tracing::warn!("saving benchmark failed: {e}");
                }
                self.toast(t!("perf.bench_done"));
                crate::ui::perf::auto_tune(self);
            }
            Err(e) => self.toast(t!("perf.bench_failed", error = e)),
        }
    }

    /// Pre-start check: every pending item of the active page is validated first.
    pub fn start_queue(&mut self) {
        self.refresh_sys();
        let infos: Vec<crate::media::probe::MediaInfo> = self
            .ws
            .items
            .iter()
            .filter(|i| matches!(i.status, ItemStatus::Pending | ItemStatus::Failed | ItemStatus::Cancelled))
            .filter_map(|i| i.info.clone())
            .collect();
        let mut issues: Vec<crate::resources::Issue> = Vec::new();
        for info in &infos {
            if let Some((_, _, v)) = crate::ui::perf::issues_for(self, info) {
                for i in v {
                    if !issues.iter().any(|x| x.field == i.field && x.message == i.message) {
                        issues.push(i);
                    }
                }
            }
        }
        // Informational warnings that don't need a decision.
        issues.retain(|i| i.field != crate::resources::Field::Fps);
        if issues.is_empty() {
            self.start_queue_checked();
        } else {
            issues.sort_by_key(|i| std::cmp::Reverse(i.level));
            self.preflight = Some(crate::ui::perf::Preflight { issues });
        }
    }

    /// "Fix automatically" from the pre-start dialog, then check again.
    pub fn fix_preflight(&mut self, issues: &[crate::resources::Issue]) {
        let info = self
            .ws
            .items
            .iter()
            .filter(|i| matches!(i.status, ItemStatus::Pending | ItemStatus::Failed | ItemStatus::Cancelled))
            .filter_map(|i| i.info.clone())
            .max_by_key(|i| i.width as u64 * i.height as u64 * i.frames.unwrap_or(1));
        let input = info.and_then(|info| crate::ui::perf::estimate_for(self, &info).map(|x| x.0));
        if let Some(i) = input {
            crate::ui::perf::apply_fix(self, &i, issues);
        }
        self.start_queue();
    }

    /// Start without the resource check (after it passed or "Start anyway").
    pub fn start_queue_checked(&mut self) {
        let mut jobs = Vec::new();
        let mut conflicts = Vec::new();
        let mut used: HashSet<PathBuf> = HashSet::new();
        let ids: Vec<u64> = self
            .ws
            .items
            .iter()
            .filter(|i| {
                matches!(
                    i.status,
                    ItemStatus::Pending | ItemStatus::Failed | ItemStatus::Cancelled
                )
            })
            .map(|i| i.id)
            .collect();
        for id in ids {
            let Some(item) = self.ws.items.iter().find(|i| i.id == id) else {
                continue;
            };
            let Some(mut out) = self.output_path_for(item) else {
                continue;
            };
            // Two queue items must never write the same file.
            if used.contains(&out) {
                out = unique_name(&out, &used);
            }
            used.insert(out.clone());
            if out.exists() {
                match item
                    .settings
                    .as_ref()
                    .unwrap_or(self.config.settings_for(item.origin))
                    .output
                    .overwrite
                {
                    OverwritePolicy::Ask => conflicts.push(out.clone()),
                    OverwritePolicy::Skip => {
                        self.set_status(
                            id,
                            ItemStatus::Skipped,
                            t!("status.output_exists_skip"),
                        );
                        continue;
                    }
                    OverwritePolicy::AutoRename => out = unique_name(&out, &used),
                    OverwritePolicy::Overwrite => {}
                }
                used.insert(out.clone());
            }
            jobs.push((id, out));
        }
        if jobs.is_empty() {
            self.toast(t!("toast.nothing_to_process"));
            return;
        }
        if conflicts.is_empty() {
            self.submit(jobs);
        } else {
            self.overwrite_prompt = Some(OverwritePrompt { jobs, conflicts });
        }
    }

    /// Resolve the overwrite prompt. `policy` = Overwrite / Skip / AutoRename.
    pub fn resolve_overwrite(&mut self, policy: Option<OverwritePolicy>) {
        let Some(p) = self.overwrite_prompt.take() else {
            return;
        };
        let Some(policy) = policy else { return };
        let conflict: HashSet<PathBuf> = p.conflicts.into_iter().collect();
        let mut used: HashSet<PathBuf> = p.jobs.iter().map(|(_, o)| o.clone()).collect();
        let mut jobs = Vec::new();
        for (id, out) in p.jobs {
            if conflict.contains(&out) {
                match policy {
                    OverwritePolicy::Skip => {
                        self.set_status(id, ItemStatus::Skipped, t!("status.output_exists"));
                        continue;
                    }
                    OverwritePolicy::AutoRename => {
                        let n = unique_name(&out, &used);
                        used.insert(n.clone());
                        jobs.push((id, n));
                        continue;
                    }
                    _ => {}
                }
            }
            jobs.push((id, out));
        }
        if !jobs.is_empty() {
            self.submit(jobs);
        }
    }

    fn submit(&mut self, jobs: Vec<(u64, PathBuf)>) {
        let mut specs = Vec::new();
        let current = self.cur_settings().clone();
        let config = &self.config;
        for (id, out) in jobs {
            if let Some(item) = self.ws.items.iter_mut().find(|i| i.id == id) {
                let Some(info) = item.info.clone() else {
                    continue;
                };
                item.status = ItemStatus::Pending;
                item.output = Some(out.clone());
                item.message = t!("status.queued");
                specs.push(JobSpec {
                    id,
                    info,
                    settings: config.job(item.settings.as_ref().unwrap_or(&current)),
                    output: out,
                    edit: None,
                });
            }
        }
        if !self.runner.is_busy() {
            self.queue_total = 0;
            self.queue_done = 0;
        }
        self.queue_total += specs.len();
        self.runner.submit(specs);
    }

    fn set_status(&mut self, id: u64, st: ItemStatus, msg: String) {
        if let Some(i) = self.item_mut(id) {
            i.status = st;
            i.message = msg;
        }
    }

    // ---- preview ----------------------------------------------------------------------

    fn selected_info(&self) -> Option<(u64, MediaInfo)> {
        let item = self.selected_item()?;
        Some((item.id, item.info.clone()?))
    }

    pub fn request_still_preview(&mut self, force: bool) {
        let Some((id, info)) = self.selected_info() else {
            return;
        };
        let settings = self.config.job(self.cur_settings());
        let t_ms = (self.ws.scrub_time * 1000.0) as u64;
        let key = (id, still_key(&settings), t_ms);
        if !force && self.ws.last_preview.as_ref() == Some(&key) {
            return;
        }
        self.ws.last_preview = Some(key);
        self.ws.player = None;
        self.ws.preview_error = None;
        self.preview.request(PreviewRequest {
            item: id,
            info,
            settings,
            kind: PreviewKind::Still {
                time: self.ws.scrub_time,
            },
        });
    }

    pub fn request_sample(&mut self) {
        let Some((id, info)) = self.selected_info() else {
            return;
        };
        self.ws.preview_error = None;
        self.ws.last_preview = None;
        self.preview.request(PreviewRequest {
            item: id,
            info,
            settings: self.config.job(self.cur_settings()),
            kind: PreviewKind::Sample {
                start: self.ws.scrub_time,
                dur: self.config.sample_seconds as f64,
            },
        });
    }

    pub fn request_compare_output(&mut self) {
        let Some(item) = self.selected_item() else {
            return;
        };
        let id = item.id;
        let (Some(info), Some(out)) = (item.info.clone(), item.output.clone()) else {
            return;
        };
        self.ws.player = None;
        self.ws.preview_error = None;
        self.ws.last_preview = None;
        self.preview.request(PreviewRequest {
            item: id,
            info,
            settings: self.config.job(self.cur_settings()),
            kind: PreviewKind::CompareOutput {
                output: out,
                time: self.ws.scrub_time,
            },
        });
    }

    pub fn mark_preview_dirty(&mut self) {
        self.ws.preview_dirty = Some(Instant::now());
    }

    // ---- event pumping ----------------------------------------------------------------

    fn pump(&mut self, ctx: &egui::Context) {
        self.pump_bench();
        if self.sys_at.elapsed() > Duration::from_secs(3) {
            self.refresh_sys();
        }
        while let Ok(p) = self.probe_rx.try_recv() {
            if let Some(item) = self.ws.items.iter_mut().find(|i| i.id == p.id) {
                match p.result {
                    Ok(info) => {
                        item.status = ItemStatus::Pending;
                        item.message = String::new();
                        if let Some(len) = info.length_label() {
                            tracing::info!("added video {}: {len}", item.name());
                        }
                        item.info = Some(info);
                    }
                    Err(e) => {
                        item.status = ItemStatus::Invalid;
                        item.message = e;
                    }
                }
                if let Some(t) = p.thumb {
                    item.thumb = Some(ctx.load_texture(
                        format!("thumb-{}", item.id),
                        crate::preview::to_color_image(&t),
                        egui::TextureOptions::LINEAR,
                    ));
                }
                if Some(p.id) == self.ws.selected && self.config.auto_preview {
                    self.mark_preview_dirty();
                }
            }
        }
        while let Ok(ev) = self.runner.events.try_recv() {
            match ev {
                RunnerEvent::Started(id) => {
                    self.job_started.insert(id, Instant::now());
                    self.set_status(id, ItemStatus::Running, t!("status.running"))
                }
                RunnerEvent::Finished(id, outcome) if self.editor.owns(id) => {
                    crate::ui::editor::on_finished(self, id, outcome);
                }
                RunnerEvent::Finished(id, outcome) => {
                    self.queue_done += 1;
                    match outcome {
                        JobOutcome::Done { output, note } => {
                            let secs = self
                                .job_started
                                .remove(&id)
                                .map(|t| t.elapsed().as_secs_f64())
                                .unwrap_or(0.0);
                            self.record_history(id, &output, secs);
                            if let Some(i) = self.item_mut(id) {
                                i.status = ItemStatus::Done;
                                i.output = Some(output);
                                i.message = note;
                            }
                        }
                        JobOutcome::Failed(e) => self.set_status(id, ItemStatus::Failed, e),
                        JobOutcome::Cancelled => {
                            self.set_status(id, ItemStatus::Cancelled, t!("status.cancelled"))
                        }
                    }
                }
                RunnerEvent::QueueIdle => {
                    self.toast(t!("toast.queue_finished", n = self.queue_done));
                }
            }
        }
        while let Ok(r) = self.preview.results.try_recv() {
            match r {
                PreviewResult::Still {
                    item,
                    before,
                    after,
                    note,
                    secs,
                } => {
                    if Some(item) == self.ws.selected {
                        self.ws.canvas.set_images(ctx, &before, &after);
                        self.ws.preview_note = note;
                        self.ws.preview_secs = secs;
                        self.ws.player = None;
                    }
                }
                PreviewResult::Sample {
                    item,
                    original,
                    enhanced,
                    src_fps,
                    out_fps,
                    src_size,
                    out_size,
                    audio,
                    note,
                } => {
                    if Some(item) == self.ws.selected {
                        let c = &mut self.config;
                        let prefs = crate::preview::player::PlayerPrefs {
                            volume: &mut c.preview_volume,
                            muted: &mut c.preview_muted,
                            looping: &mut c.preview_loop,
                        };
                        self.ws.player = Some(SamplePlayer::new(
                            original, enhanced, src_fps, out_fps, src_size, out_size, audio,
                            note, &prefs,
                        ));
                    }
                }
                PreviewResult::Error { item, msg } => {
                    if Some(item) == self.ws.selected {
                        tracing::error!("preview failed: {msg}");
                        self.ws.preview_error = Some(msg);
                    }
                }
            }
        }
        while self.history_rx.try_recv().is_ok() {
            self.outputs_dirty = true;
        }
        if self.outputs.pump(ctx)
            && let Some(sel) = &self.outputs.selected
            && !self.outputs.records.iter().any(|r| &r.path == sel)
        {
            self.outputs.selected = None;
        }
        if self.outputs_dirty && self.page == Page::Outputs {
            self.outputs_dirty = false;
            self.refresh_outputs(ctx);
        }
        if self.tools_ui.take_finished(&self.paths) {
            self.models_ui.rescan(&self.paths);
            self.reload_tools(ctx);
        }
        if self.models_ui.take_finished(&self.paths) {
            self.reload_tools(ctx);
        }
        // Debounced re-render of the still preview.
        if let Some(t) = self.ws.preview_dirty {
            if t.elapsed() >= Duration::from_millis(400) {
                self.ws.preview_dirty = None;
                self.request_still_preview(false);
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        self.toasts
            .retain(|(t, _)| t.elapsed() < Duration::from_secs(6));
        // Persist settings (throttled).
        if self.config != self.saved_config && self.last_save.elapsed() > Duration::from_secs(2) {
            if let Err(e) = self.config.save(&self.paths.config_file) {
                tracing::warn!("saving config failed: {e}");
            }
            self.saved_config = self.config.clone();
            self.last_save = Instant::now();
        }
    }
}

impl App {
    /// Folders the Outputs page scans: the output folder of every page that has one
    /// (pages writing next to the source are covered by history only).
    pub fn outputs_folders(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = Vec::new();
        for s in Studio::ALL {
            if let Some(f) = self.config.settings_for(s).output.folder.clone()
                && !v.iter().any(|x| crate::history::path_key(x) == crate::history::path_key(&f))
            {
                v.push(f);
            }
        }
        v
    }

    pub fn refresh_outputs(&mut self, ctx: &egui::Context) {
        let folders = self.outputs_folders();
        self.outputs.refresh(&self.paths.history_file, folders, ctx);
    }

    /// Append a finished job to `history.json` (probing the output off the UI thread).
    fn record_history(&mut self, id: u64, output: &Path, processing_secs: f64) {
        let Some(item) = self.item(id) else {
            return;
        };
        let settings = self.config.job(
            item.settings
                .as_ref()
                .unwrap_or(self.config.settings_for(item.origin)),
        );
        let source = item.path.clone();
        let src_info = item.info.clone();
        let profile = if settings.upscale_on {
            settings.profile.label()
        } else {
            t!("history.no_upscale")
        };
        let scale = if settings.upscale_on {
            settings.scale.label()
        } else {
            "1x".into()
        };
        let encoder = self
            .env
            .encoders
            .read()
            .ok()
            .and_then(|a| a.resolve(settings.output.encoder))
            .map(|e| e.label().to_string());
        let ffprobe = self.env.tools.ffprobe.clone();
        let file = self.paths.history_file.clone();
        let output = output.to_path_buf();
        let tx = self.history_tx.clone();
        let _ = std::thread::Builder::new()
            .name("history".into())
            .spawn(move || {
                let kind = crate::history::EntryKind::from_path(&output)
                    .unwrap_or(crate::history::EntryKind::Image);
                let out_info = probe::probe(ffprobe.as_deref(), &output).ok();
                let is_video = kind == crate::history::EntryKind::Video;
                let fps = is_video.then(|| {
                    let f = |i: &Option<MediaInfo>| {
                        i.as_ref()
                            .and_then(|i| i.fps)
                            .map(|f| f.to_string())
                            .unwrap_or_else(|| "?".into())
                    };
                    format!("{} → {} fps", f(&src_info), f(&out_info))
                });
                let entry = crate::history::HistoryEntry {
                    output: output.clone(),
                    source: Some(source),
                    kind,
                    created: crate::history::now_unix(),
                    profile,
                    scale,
                    fps,
                    encoder: if is_video { encoder } else { None },
                    width: out_info.as_ref().map(|i| i.width).unwrap_or(0),
                    height: out_info.as_ref().map(|i| i.height).unwrap_or(0),
                    duration_secs: out_info.as_ref().and_then(|i| i.duration),
                    size_bytes: std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0),
                    processing_secs,
                    edited: false,
                };
                if let Err(e) = crate::history::History::append_to_file(&file, entry) {
                    tracing::warn!("writing history failed: {e}");
                }
                let _ = tx.send(());
            });
    }

    /// Re-discover tools/models after the downloader installed something. Workers are
    /// restarted only while idle; otherwise the user is told to restart.
    pub fn reload_tools(&mut self, ctx: &egui::Context) {
        let tools = ToolSet::discover(&self.paths);
        let models = ModelSet::discover(&self.paths.models);
        self.setup = SetupReport::check(&self.paths, &tools, &models);
        if self.runner.is_busy() || self.preview.is_busy() {
            self.toast(t!("tools.restart_to_use"));
            return;
        }
        let encoders = detect_encoders_bg(tools.ffmpeg.clone(), ctx);
        let env = Arc::new(Env {
            tools,
            models,
            gpus: self.env.gpus.clone(),
            encoders,
            monitor: Some(self.monitor.clone()),
            cores: self.env.cores,
        });
        self.runner = Runner::start(env.clone(), Some(ctx.clone()));
        self.preview = PreviewWorker::start(env.clone(), Some(ctx.clone()));
        let (tx, rx) = spawn_probe_worker(env.clone(), ctx.clone());
        self.probe_tx = tx;
        self.probe_rx = rx;
        self.outputs = crate::outputs::OutputsState::new(&self.paths, env.tools.clone(), ctx.clone());
        self.outputs_dirty = true;
        self.env = env;
        self.toast(t!("tools.reloaded"));
        if !self.config.bench_offered && self.env.tools.realesrgan.is_some() && !self.env.gpus.is_empty() {
            self.bench_offer = true;
        }
    }

    /// Outputs page → "Compare with source": switch to the queue and show the exported
    /// file against its source.
    pub fn compare_with_source(&mut self, source: PathBuf, output: PathBuf) {
        let studio = match detect_kind(&source) {
            Some(MediaKind::Video) => Studio::Video,
            Some(MediaKind::Image) => Studio::Image,
            None => Studio::All,
        };
        self.switch_studio(studio);
        if !self.ws.items.iter().any(|i| i.path == source) {
            self.add_paths(vec![source.clone()]);
        }
        let Some(item) = self.ws.items.iter_mut().find(|i| i.path == source) else {
            return;
        };
        item.output = Some(output.clone());
        let id = item.id;
        let known = item.info.clone();
        let info = known
            .clone()
            .or_else(|| probe::probe(self.env.tools.ffprobe.as_deref(), &source).ok());
        if let Some(i) = self.ws.items.iter_mut().find(|i| i.id == id) {
            if i.info.is_none() {
                i.info = info.clone();
            }
            if i.status != ItemStatus::Running {
                i.status = ItemStatus::Done;
            }
        }
        self.ws.selected = Some(id);
        self.ws.player = None;
        self.ws.canvas.clear();
        let Some(info) = info else {
            self.ws.preview_error = Some(t!("outputs.source_unreadable"));
            return;
        };
        self.ws.preview_error = None;
        self.ws.last_preview = None;
        self.preview.request(PreviewRequest {
            item: id,
            info,
            settings: self.config.job(self.cur_settings()),
            kind: PreviewKind::CompareOutput { output, time: 0.0 },
        });
    }
}

fn detect_encoders_bg(
    ffmpeg: Option<PathBuf>,
    ctx: &egui::Context,
) -> Arc<RwLock<EncoderAvailability>> {
    let encoders = Arc::new(RwLock::new(EncoderAvailability::default()));
    let enc = encoders.clone();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let r = encoders::detect(ffmpeg.as_deref());
        if let Ok(mut g) = enc.write() {
            *g = r;
        }
        ctx.request_repaint();
    });
    encoders
}

/// The subset of settings that affects a still preview (output / video-pipeline /
/// frame-generation options don't change a single processed frame).
pub fn still_key(s: &JobSettings) -> JobSettings {
    let d = JobSettings::default();
    JobSettings {
        output: d.output,
        video: d.video,
        framegen: d.framegen,
        max_fps: d.max_fps,
        rife_model: d.rife_model,
        scene_detect: d.scene_detect,
        scene_threshold: d.scene_threshold,
        ..s.clone()
    }
}

fn unique_name(out: &Path, used: &HashSet<PathBuf>) -> PathBuf {
    let stem = out
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = out
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let dir = out.parent().map(Path::to_path_buf).unwrap_or_default();
    (2..10_000)
        .map(|n| dir.join(format!("{stem} ({n}).{ext}")))
        .find(|p| !p.exists() && !used.contains(p))
        .unwrap_or_else(|| out.to_path_buf())
}

fn collect_dir(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            if recursive {
                collect_dir(&p, recursive, out);
            }
        } else {
            out.push(p);
        }
    }
}

fn spawn_probe_worker(
    env: Arc<Env>,
    ctx: egui::Context,
) -> (Sender<(u64, PathBuf)>, Receiver<ProbeDone>) {
    let (tx, rx) = unbounded::<(u64, PathBuf)>();
    let (done_tx, done_rx) = unbounded();
    let _ = std::thread::Builder::new()
        .name("probe".into())
        .spawn(move || {
            for (id, path) in rx {
                let result =
                    probe::probe(env.tools.ffprobe.as_deref(), &path).map_err(|e| e.to_string());
                let thumb = result.as_ref().ok().and_then(|info| thumbnail(&env, info));
                let _ = done_tx.send(ProbeDone { id, result, thumb });
                ctx.request_repaint();
            }
        });
    (tx, done_rx)
}

fn thumbnail(env: &Env, info: &MediaInfo) -> Option<RgbImage> {
    const T: u32 = 96;
    let (w, h) = crate::engines::resize::fit_within(info.width, info.height, T, T);
    match info.kind {
        MediaKind::Image => {
            let img = image::open(&info.path).ok()?;
            Some(img.thumbnail(T, T).into_rgb8())
        }
        MediaKind::Video => {
            let t = info.duration.map(|d| d * 0.1).unwrap_or(0.0);
            crate::media::ffmpeg::extract_frame(
                env.tools.ffmpeg.as_ref()?,
                &info.path,
                t,
                w.max(2),
                h.max(2),
                true,
                None,
            )
            .ok()
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pump(&ctx);

        // Drag & drop.
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            match self.page {

                Page::Queue | Page::Video | Page::Image => self.add_paths(dropped),
                Page::Editor => crate::ui::editor::add_paths(self, dropped),
                // Other pages: add to the page matching the first file.
                _ => {
                    let to = match dropped.iter().find_map(|p| detect_kind(p)) {
                        Some(MediaKind::Video) => Studio::Video,
                        Some(MediaKind::Image) => Studio::Image,
                        None => self.studio,
                    };
                    self.switch_studio(to);
                    self.add_paths(dropped);
                }
            }
        }
        // Space toggles before/after.
        if !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Space)) {
            self.ws.canvas.toggle();
        }

        if self.tools_only {
            crate::ui::wizard::wizard_page(self, ui);
            crate::ui::toasts(self, &ctx);
            return;
        }

        crate::ui::top_bar(self, ui);
        crate::ui::bottom_panel(self, ui);
        match self.page {
            Page::Queue => {
                crate::ui::queue::queue_panel(self, ui);
                crate::ui::settings::settings_panel(self, ui);
                crate::ui::preview_panel(self, ui);
            }
            Page::Video => {
                crate::ui::queue::queue_panel(self, ui);
                crate::ui::studio::video_panel(self, ui);
                crate::ui::preview_panel(self, ui);
            }
            Page::Image => {
                crate::ui::queue::queue_panel(self, ui);
                crate::ui::studio::image_panel(self, ui);
                crate::ui::preview_panel(self, ui);
            }
            Page::Editor => crate::ui::editor::editor_page(self, ui),
            Page::Models => crate::ui::models::models_page(self, ui),
            Page::Outputs => crate::ui::outputs::outputs_page(self, ui),
            Page::Settings => crate::ui::settings_page::settings_page(self, ui),
        }
        crate::ui::setup::setup_window(self, &ctx);
        crate::ui::tools::tools_window(self, &ctx);
        crate::ui::wizard::wizard_window(self, &ctx);
        crate::ui::models::models_dialogs(self, &ctx);
        crate::ui::update::update_window(self, &ctx);
        crate::ui::update::optin_modal(self, &ctx);
        crate::ui::fonttest::font_test_window(self, &ctx);
        crate::ui::perf::preflight_window(self, &ctx);
        crate::ui::perf::bench_offer_window(self, &ctx);
        crate::ui::overwrite_window(self, &ctx);
        crate::ui::toasts(self, &ctx);
        crate::ui::drop_hint(&ctx);

        if self.runner.is_busy() || self.preview.is_busy() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn on_exit(&mut self) {
        self.runner.cancel_all();
        self.preview.cancel();
        if let Err(e) = self.config.save(&self.paths.config_file) {
            tracing::warn!("saving config failed: {e}");
        }
    }
}
