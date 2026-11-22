//! Data / tool locations (installed vs portable), external tool and model discovery, and
//! the first-run setup check.
//!
//! * **Installed mode** (default): config, history, thumbnails and logs live in
//!   `%APPDATA%\QuickEnhancer`; downloaded tools and models in `%LOCALAPPDATA%\QuickEnhancer`
//!   (`bin/`, `models/`). An app in `Program Files` cannot write next to its exe.
//! * **Portable mode**: everything next to the exe. Enabled by a `portable.txt` file next to
//!   the exe, by `QUICK_ENHANCER_HOME` (or the v1 alias `ENHANCER_HOME`) pointing at a
//!   folder, or automatically under `cargo run` / `cargo test` (the project folder is used).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Folder name used under `%APPDATA%` / `%LOCALAPPDATA%`.
pub const APP_DIR_NAME: &str = "QuickEnhancer";
/// Marker file that switches the app to portable mode.
pub const PORTABLE_MARKER: &str = "portable.txt";
/// Environment overrides (the second one is the v1 name, kept as an alias).
pub const HOME_ENV: &str = "QUICK_ENHANCER_HOME";
pub const LEGACY_HOME_ENV: &str = "ENHANCER_HOME";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathMode {
    Installed,
    Portable,
}

/// Directories the app uses.
#[derive(Clone, Debug)]
pub struct AppPaths {
    pub mode: PathMode,
    /// Folder that contains the executable.
    pub exe_dir: PathBuf,
    /// Root for downloaded tools and models (parent of `bin/` and `models/`).
    pub base: PathBuf,
    pub bin: PathBuf,
    pub models: PathBuf,
    /// Root for config, history, thumbnails and logs.
    pub data: PathBuf,
    pub logs: PathBuf,
    pub thumbs: PathBuf,
    pub config_file: PathBuf,
    pub history_file: PathBuf,
    /// Records which tool versions the downloader installed.
    pub tools_state_file: PathBuf,
}

/// Everything path resolution depends on (gathered from the environment, or given by tests).
#[derive(Clone, Debug, Default)]
pub struct PathEnv {
    pub exe_dir: PathBuf,
    /// `QUICK_ENHANCER_HOME`, else `ENHANCER_HOME`.
    pub home_override: Option<PathBuf>,
    /// Set by `cargo run` / `cargo test`.
    pub cargo_manifest_dir: Option<PathBuf>,
    /// `%APPDATA%` (Linux: config dir).
    pub roaming: Option<PathBuf>,
    /// `%LOCALAPPDATA%` (Linux: local data dir).
    pub local: Option<PathBuf>,
}

impl PathEnv {
    pub fn current() -> PathEnv {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        PathEnv {
            exe_dir,
            home_override: var(HOME_ENV).or_else(|| var(LEGACY_HOME_ENV)),
            cargo_manifest_dir: var("CARGO_MANIFEST_DIR"),
            roaming: dirs::config_dir(),
            local: dirs::data_local_dir(),
        }
    }
}

impl AppPaths {
    pub fn resolve() -> AppPaths {
        Self::resolve_with(&PathEnv::current())
    }

    /// Pure resolution logic (unit-tested).
    pub fn resolve_with(env: &PathEnv) -> AppPaths {
        if let Some(home) = &env.home_override {
            return Self::portable(home.clone(), env.exe_dir.clone());
        }
        if env.exe_dir.join(PORTABLE_MARKER).is_file() {
            return Self::portable(env.exe_dir.clone(), env.exe_dir.clone());
        }
        if let Some(dev) = &env.cargo_manifest_dir {
            return Self::portable(dev.clone(), env.exe_dir.clone());
        }
        match (&env.roaming, &env.local) {
            (Some(roaming), Some(local)) => {
                let data = roaming.join(APP_DIR_NAME);
                let base = local.join(APP_DIR_NAME);
                Self::build(PathMode::Installed, env.exe_dir.clone(), base, data)
            }
            _ => Self::portable(env.exe_dir.clone(), env.exe_dir.clone()),
        }
    }

    /// Portable layout rooted at `base` (also used by tests and headless tools).
    pub fn portable(base: PathBuf, exe_dir: PathBuf) -> AppPaths {
        Self::build(PathMode::Portable, exe_dir, base.clone(), base)
    }

    pub fn from_base(base: PathBuf) -> AppPaths {
        Self::portable(base.clone(), base)
    }

    fn build(mode: PathMode, exe_dir: PathBuf, base: PathBuf, data: PathBuf) -> AppPaths {
        AppPaths {
            mode,
            exe_dir,
            bin: base.join("bin"),
            models: base.join("models"),
            tools_state_file: base.join("tools-installed.json"),
            logs: data.join("logs"),
            thumbs: data.join("thumbs"),
            config_file: data.join("config.toml"),
            history_file: data.join("history.json"),
            data,
            base,
        }
    }

    /// Create the writable folders (best effort).
    pub fn ensure_dirs(&self) {
        for d in [&self.data, &self.logs, &self.thumbs, &self.bin, &self.models] {
            let _ = std::fs::create_dir_all(d);
        }
    }

    /// v1 ("Enhancer") config files that may be imported once on first start.
    pub fn legacy_config_candidates(&self, env: &PathEnv) -> Vec<PathBuf> {
        let mut v = vec![env.exe_dir.join("config.toml")];
        if let Some(r) = &env.roaming {
            v.push(r.join("Enhancer").join("config.toml"));
        }
        v.retain(|p| *p != self.config_file);
        v
    }

    /// Import a v1 config once: only when this install has no config yet.
    /// Returns the file that was imported.
    pub fn migrate_legacy_config(&self, env: &PathEnv) -> Option<PathBuf> {
        if self.config_file.exists() {
            return None;
        }
        let src = self
            .legacy_config_candidates(env)
            .into_iter()
            .find(|p| p.is_file())?;
        std::fs::create_dir_all(&self.data).ok()?;
        std::fs::copy(&src, &self.config_file).ok()?;
        Some(src)
    }
}

fn exe_name(stem: &str) -> String {
    format!("{stem}{}", std::env::consts::EXE_SUFFIX)
}

/// Look for `bin/<name>`, then (only for ffmpeg/ffprobe) the system PATH.
fn find_tool(bin: &Path, stem: &str, allow_path: bool) -> Option<PathBuf> {
    let local = bin.join(exe_name(stem));
    if local.is_file() {
        return Some(local);
    }
    if allow_path {
        let path_var = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path_var) {
            let cand = dir.join(exe_name(stem));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

/// Paths to the external executables (any may be missing).
#[derive(Clone, Debug, Default)]
pub struct ToolSet {
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
    pub realesrgan: Option<PathBuf>,
    pub rife: Option<PathBuf>,
}

impl ToolSet {
    pub fn discover(paths: &AppPaths) -> ToolSet {
        ToolSet {
            ffmpeg: find_tool(&paths.bin, "ffmpeg", true),
            ffprobe: find_tool(&paths.bin, "ffprobe", true),
            realesrgan: find_tool(&paths.bin, "realesrgan-ncnn-vulkan", false),
            rife: find_tool(&paths.bin, "rife-ncnn-vulkan", false),
        }
    }

    /// Tools for tests / headless use from an explicit bin dir.
    pub fn from_bin_dir(bin: &Path) -> ToolSet {
        ToolSet {
            ffmpeg: find_tool(bin, "ffmpeg", false),
            ffprobe: find_tool(bin, "ffprobe", false),
            realesrgan: find_tool(bin, "realesrgan-ncnn-vulkan", false),
            rife: find_tool(bin, "rife-ncnn-vulkan", false),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RifeModel {
    pub name: String,
    pub path: PathBuf,
    /// RIFE v4.x models support arbitrary frame counts (3x, 4x); older ones only 2x.
    pub arbitrary_count: bool,
}

/// Installed model files.
#[derive(Clone, Debug, Default)]
pub struct ModelSet {
    pub realesrgan_dir: PathBuf,
    /// Real-ESRGAN model names (file stems) for which both `.param` and `.bin` exist.
    pub realesrgan: BTreeSet<String>,
    pub rife: Vec<RifeModel>,
}

impl ModelSet {
    pub fn discover(models_dir: &Path) -> ModelSet {
        let realesrgan_dir = models_dir.join("realesrgan");
        let mut realesrgan = BTreeSet::new();
        if let Ok(rd) = std::fs::read_dir(&realesrgan_dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "param")
                    && let Some(stem) = p.file_stem().and_then(|s| s.to_str())
                    && p.with_extension("bin").is_file()
                {
                    realesrgan.insert(stem.to_string());
                }
            }
        }
        let mut rife = Vec::new();
        if let Ok(rd) = std::fs::read_dir(models_dir.join("rife")) {
            for e in rd.flatten() {
                let p = e.path();
                if p.join("flownet.param").is_file() && p.join("flownet.bin").is_file() {
                    let name = e.file_name().to_string_lossy().to_string();
                    let arbitrary_count = name.contains("v4");
                    rife.push(RifeModel {
                        name,
                        path: p,
                        arbitrary_count,
                    });
                }
            }
        }
        rife.sort_by(|a, b| b.name.cmp(&a.name));
        ModelSet {
            realesrgan_dir,
            realesrgan,
            rife,
        }
    }

    pub fn has(&self, model: &str) -> bool {
        self.realesrgan.contains(model)
    }

    pub fn rife_model(&self, name: &str) -> Option<&RifeModel> {
        self.rife
            .iter()
            .find(|m| m.name == name)
            .or_else(|| self.rife.first())
    }

    /// Native scales present for `realesr-animevideov3` (files `-x2`, `-x3`, `-x4`).
    pub fn animevideo_scales(&self) -> Vec<u32> {
        (2..=4)
            .filter(|s| self.has(&format!("realesr-animevideov3-x{s}")))
            .collect()
    }
}

/// One line of the setup panel.
#[derive(Clone, Debug)]
pub struct SetupItem {
    pub what: String,
    pub expected: PathBuf,
    pub present: bool,
    pub needed_for: &'static str,
    pub required: bool,
}

#[derive(Clone, Debug, Default)]
pub struct SetupReport {
    pub items: Vec<SetupItem>,
}

impl SetupReport {
    pub fn check(paths: &AppPaths, tools: &ToolSet, models: &ModelSet) -> SetupReport {
        let mut items = Vec::new();
        let mut tool = |what: &str, p: &Option<PathBuf>, needed_for: &'static str, required| {
            items.push(SetupItem {
                what: what.to_string(),
                expected: p.clone().unwrap_or_else(|| paths.bin.join(exe_name(what))),
                present: p.is_some(),
                needed_for,
                required,
            })
        };
        tool(
            "ffmpeg",
            &tools.ffmpeg,
            "setup.need_ffmpeg",
            true,
        );
        tool("ffprobe", &tools.ffprobe, "setup.need_ffprobe", true);
        tool(
            "realesrgan-ncnn-vulkan",
            &tools.realesrgan,
            "setup.need_ai",
            false,
        );
        tool("rife-ncnn-vulkan", &tools.rife, "setup.need_framegen", false);
        for m in [
            "realesrgan-x4plus",
            "realesrgan-x4plus-anime",
            "realesr-animevideov3-x2",
            "realesr-animevideov3-x3",
            "realesr-animevideov3-x4",
        ] {
            items.push(SetupItem {
                what: format!("{m}.param + .bin"),
                expected: models.realesrgan_dir.join(format!("{m}.param")),
                present: models.has(m),
                needed_for: if m.contains("anime") {
                    "setup.need_anime"
                } else {
                    "setup.need_realistic"
                },
                required: false,
            });
        }
        items.push(SetupItem {
            what: crate::t!("setup.rife_model_folder"),
            expected: paths.models.join("rife").join("rife-v4.6"),
            present: !models.rife.is_empty(),
            needed_for: "setup.need_framegen",
            required: false,
        });
        SetupReport { items }
    }

    pub fn missing(&self) -> impl Iterator<Item = &SetupItem> {
        self.items.iter().filter(|i| !i.present)
    }

    pub fn all_present(&self) -> bool {
        self.items.iter().all(|i| i.present)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("qe-paths-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn installed_env(root: &Path) -> PathEnv {
        PathEnv {
            exe_dir: root.join("Program Files").join("QuickEnhancer"),
            home_override: None,
            cargo_manifest_dir: None,
            roaming: Some(root.join("Roaming")),
            local: Some(root.join("Local")),
        }
    }

    #[test]
    fn installed_mode_splits_roaming_and_local() {
        let root = tmp("inst");
        let env = installed_env(&root);
        std::fs::create_dir_all(&env.exe_dir).unwrap();
        let p = AppPaths::resolve_with(&env);
        assert_eq!(p.mode, PathMode::Installed);
        assert_eq!(p.data, root.join("Roaming").join(APP_DIR_NAME));
        assert_eq!(p.config_file, p.data.join("config.toml"));
        assert_eq!(p.history_file, p.data.join("history.json"));
        assert_eq!(p.thumbs, p.data.join("thumbs"));
        assert_eq!(p.logs, p.data.join("logs"));
        assert_eq!(p.bin, root.join("Local").join(APP_DIR_NAME).join("bin"));
        assert_eq!(p.models, root.join("Local").join(APP_DIR_NAME).join("models"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn portable_marker_cargo_and_override() {
        let root = tmp("port");
        let mut env = installed_env(&root);
        std::fs::create_dir_all(&env.exe_dir).unwrap();
        // portable.txt next to the exe → everything next to the exe.
        std::fs::write(env.exe_dir.join(PORTABLE_MARKER), "").unwrap();
        let p = AppPaths::resolve_with(&env);
        assert_eq!(p.mode, PathMode::Portable);
        assert_eq!(p.data, env.exe_dir);
        assert_eq!(p.bin, env.exe_dir.join("bin"));
        assert_eq!(p.config_file, env.exe_dir.join("config.toml"));
        std::fs::remove_file(env.exe_dir.join(PORTABLE_MARKER)).unwrap();
        // `cargo run` → portable at the project folder.
        env.cargo_manifest_dir = Some(root.join("project"));
        let p = AppPaths::resolve_with(&env);
        assert_eq!(p.mode, PathMode::Portable);
        assert_eq!(p.base, root.join("project"));
        // QUICK_ENHANCER_HOME wins over everything.
        env.home_override = Some(root.join("home"));
        let p = AppPaths::resolve_with(&env);
        assert_eq!(p.base, root.join("home"));
        assert_eq!(p.data, root.join("home"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn legacy_config_is_migrated_once() {
        let root = tmp("mig");
        let env = installed_env(&root);
        std::fs::create_dir_all(&env.exe_dir).unwrap();
        let old = env.roaming.as_ref().unwrap().join("Enhancer");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("config.toml"), "recursive = true\n").unwrap();
        let p = AppPaths::resolve_with(&env);
        assert_eq!(p.migrate_legacy_config(&env), Some(old.join("config.toml")));
        assert_eq!(std::fs::read_to_string(&p.config_file).unwrap(), "recursive = true\n");
        // Second start: the new config exists, nothing is copied again.
        std::fs::write(old.join("config.toml"), "recursive = false\n").unwrap();
        assert_eq!(p.migrate_legacy_config(&env), None);
        assert_eq!(std::fs::read_to_string(&p.config_file).unwrap(), "recursive = true\n");
        let _ = std::fs::remove_dir_all(&root);
    }
}
