//! Install state of catalog models and runtimes, install plans (model + helpers + runtime) and
//! uninstall. Files are checked on disk (the source of truth); `tools-installed.json` records
//! which package versions the downloader wrote, to spot outdated files after a catalog update.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::catalog::{Catalog, ModelEntry, RUNTIME_CUDA, RUNTIME_DML, RuntimeNeed};
use crate::net::tools::{Component, InstalledTools, ToolsManifest, remove_package};
use crate::paths::AppPaths;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelState {
    Installed,
    NotInstalled,
    /// Some files are missing (interrupted copy, antivirus, manual deletion) → Repair.
    Partial,
    /// Installed, but a package version differs from the catalog → Update.
    Outdated,
}

impl ModelState {
    pub fn is_usable(self) -> bool {
        matches!(self, ModelState::Installed | ModelState::Outdated)
    }
    pub fn label_key(self) -> &'static str {
        match self {
            ModelState::Installed => "models.state_installed",
            ModelState::NotInstalled => "models.state_not_installed",
            ModelState::Partial => "models.state_partial",
            ModelState::Outdated => "models.state_outdated",
        }
    }
}

/// ONNX Runtime flavour. One runtime is loaded per process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum RuntimeKind {
    /// DirectML: any DirectX 12 GPU (NVIDIA, AMD, Intel). Small download.
    #[default]
    DirectMl,
    /// CUDA + cuDNN: NVIDIA only, ~1 GB download, faster for some models; needed by SeedVR2.
    Cuda,
}

impl RuntimeKind {
    pub fn packages(self) -> &'static [&'static str] {
        match self {
            RuntimeKind::DirectMl => RUNTIME_DML,
            RuntimeKind::Cuda => RUNTIME_CUDA,
        }
    }
    pub fn dir_name(self) -> &'static str {
        match self {
            RuntimeKind::DirectMl => "dml",
            RuntimeKind::Cuda => "cuda",
        }
    }
    /// Folder holding `onnxruntime.dll` and its providers.
    pub fn dir(self, base: &Path) -> PathBuf {
        base.join("runtime").join(self.dir_name())
    }
}

/// Snapshot of what is on disk, computed once and reused by the UI and the pipeline.
#[derive(Clone, Debug, Default)]
pub struct InstallSnapshot {
    pub base: PathBuf,
    /// Present files (relative paths as written in the catalog).
    present: BTreeSet<String>,
    pub installed_versions: InstalledTools,
    pub runtime_dml: bool,
    pub runtime_cuda: bool,
}

impl InstallSnapshot {
    pub fn scan(paths: &AppPaths, catalog: &Catalog, tools: &ToolsManifest) -> InstallSnapshot {
        let base = paths.base.clone();
        let mut present = BTreeSet::new();
        for m in &catalog.models {
            for f in &m.files {
                if base.join(f).is_file() {
                    present.insert(f.clone());
                }
            }
        }
        let runtime_ok = |k: RuntimeKind| {
            k.packages().iter().all(|id| {
                catalog
                    .resolve_package(id, tools)
                    .is_some_and(|p| p.is_present(&base))
            })
        };
        InstallSnapshot {
            runtime_dml: runtime_ok(RuntimeKind::DirectMl),
            runtime_cuda: runtime_ok(RuntimeKind::Cuda),
            installed_versions: InstalledTools::load(&paths.tools_state_file),
            present,
            base,
        }
    }

    pub fn has_runtime(&self, k: RuntimeKind) -> bool {
        match k {
            RuntimeKind::DirectMl => self.runtime_dml,
            RuntimeKind::Cuda => self.runtime_cuda,
        }
    }

    pub fn any_runtime(&self) -> bool {
        self.runtime_dml || self.runtime_cuda
    }

    /// State of the model's own files (helpers and runtime are reported separately).
    pub fn state(&self, catalog: &Catalog, tools: &ToolsManifest, m: &ModelEntry) -> ModelState {
        let have = m.files.iter().filter(|f| self.present.contains(*f)).count();
        if have == 0 {
            return ModelState::NotInstalled;
        }
        if have < m.files.len() {
            return ModelState::Partial;
        }
        let outdated = m.packages.iter().any(|id| {
            match (
                self.installed_versions.components.get(id),
                catalog.resolve_package(id, tools),
            ) {
                (Some(v), Some(p)) => *v != p.version,
                _ => false,
            }
        });
        if outdated {
            ModelState::Outdated
        } else {
            ModelState::Installed
        }
    }

    /// Everything needed to run the model: files, helper models and a matching runtime.
    pub fn ready(&self, catalog: &Catalog, tools: &ToolsManifest, m: &ModelEntry) -> bool {
        if !self.state(catalog, tools, m).is_usable() {
            return false;
        }
        let helpers_ok = m.required_models().all(|id| {
            catalog
                .model(id)
                .is_some_and(|h| self.state(catalog, tools, h).is_usable())
        });
        let runtime_ok = match m.needs_runtime() {
            None => true,
            Some(RuntimeNeed::Any) => self.any_runtime(),
            Some(RuntimeNeed::Cuda) => self.runtime_cuda,
        };
        helpers_ok && runtime_ok
    }

    /// Mark files as present / absent after an install or uninstall without rescanning.
    pub fn refresh_files(&mut self, catalog: &Catalog) {
        self.present = catalog
            .models
            .iter()
            .flat_map(|m| m.files.iter())
            .filter(|f| self.base.join(f).is_file())
            .cloned()
            .collect();
    }
}

/// Packages to download so that `model` can run: helpers, the runtime (`runtime` when none is
/// installed; CUDA when the model needs it) and the model's own packages. Already-present
/// packages are skipped unless `repair` is set (then the model's own packages are fetched again).
pub fn install_plan(
    catalog: &Catalog,
    tools: &ToolsManifest,
    snap: &InstallSnapshot,
    model: &ModelEntry,
    runtime: RuntimeKind,
    repair: bool,
) -> Vec<Component> {
    let mut ids: Vec<String> = Vec::new();
    let mut push = |id: &str| {
        if !ids.iter().any(|x| x == id) {
            ids.push(id.to_string());
        }
    };
    match model.needs_runtime() {
        Some(RuntimeNeed::Cuda) if !snap.runtime_cuda => RUNTIME_CUDA.iter().for_each(|p| push(p)),
        Some(RuntimeNeed::Any) if !snap.any_runtime() => runtime.packages().iter().for_each(|p| push(p)),
        _ => {}
    }
    for h in model.required_models() {
        if let Some(h) = catalog.model(h)
            && !snap.state(catalog, tools, h).is_usable()
        {
            h.packages.iter().for_each(|p| push(p));
        }
    }
    model.packages.iter().for_each(|p| push(p));
    ids.into_iter()
        .filter_map(|id| catalog.resolve_package(&id, tools))
        .filter(|p| !p.is_present(&snap.base) || (repair && model.packages.contains(&p.id)))
        .collect()
}

/// Total download size of a plan.
pub fn plan_size(plan: &[Component]) -> u64 {
    plan.iter().map(|p| p.size).sum()
}

/// Uninstall a model: remove its own packages unless another installed model still uses them.
/// Packages from `tools.json` (Real-ESRGAN, RIFE) and runtimes are never removed here, and
/// files that another present package also writes (e.g. `bin/vcomp140.dll`) are kept.
/// Returns the removed package ids.
pub fn uninstall(
    paths: &AppPaths,
    catalog: &Catalog,
    tools: &ToolsManifest,
    snap: &InstallSnapshot,
    model: &ModelEntry,
) -> std::io::Result<Vec<String>> {
    let shared: BTreeSet<&str> = catalog
        .models
        .iter()
        .filter(|o| o.id != model.id && snap.state(catalog, tools, o).is_usable())
        .flat_map(|o| o.packages.iter().map(String::as_str))
        .collect();
    let runtime: BTreeSet<&str> = RUNTIME_DML.iter().chain(RUNTIME_CUDA).copied().collect();
    let mut removed = Vec::new();
    for id in &model.packages {
        let Some(pkg) = catalog.package(id) else { continue }; // tools.json package
        if shared.contains(id.as_str()) || runtime.contains(id.as_str()) {
            continue;
        }
        let keep = kept_destinations(catalog, tools, &snap.base, &pkg.id);
        remove_package(paths, pkg, &keep)?;
        removed.push(id.clone());
    }
    Ok(removed)
}

/// Remove an ONNX runtime (frees ~40 MB for DirectML, ~1 GB for CUDA).
pub fn uninstall_runtime(
    paths: &AppPaths,
    catalog: &Catalog,
    tools: &ToolsManifest,
    kind: RuntimeKind,
) -> std::io::Result<()> {
    for id in kind.packages() {
        if let Some(pkg) = catalog.package(id) {
            let keep = kept_destinations(catalog, tools, &paths.base, id);
            remove_package(paths, pkg, &keep)?;
        }
    }
    Ok(())
}

/// Destinations written by other packages that are currently present.
fn kept_destinations(catalog: &Catalog, tools: &ToolsManifest, base: &Path, except: &str) -> BTreeSet<String> {
    catalog
        .packages
        .iter()
        .chain(tools.components.iter())
        .filter(|p| p.id != except && p.is_present(base))
        .flat_map(|p| p.destinations())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(name: &str) -> (AppPaths, Catalog, ToolsManifest) {
        let dir = std::env::temp_dir().join(format!("qe-inst-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (AppPaths::from_base(dir), Catalog::embedded(), ToolsManifest::embedded())
    }

    fn touch(base: &Path, rel: &str) {
        let p = base.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"x").unwrap();
    }

    fn install_files(base: &Path, pkg: &Component) {
        for f in &pkg.files {
            if f.to.ends_with('/') {
                touch(base, &format!("{}dummy.bin", f.to));
            } else {
                touch(base, &f.to);
            }
        }
    }

    #[test]
    fn state_detection() {
        let (paths, cat, tools) = setup("state");
        let m = cat.model("realcugan-se").unwrap().clone();
        let snap = InstallSnapshot::scan(&paths, &cat, &tools);
        assert_eq!(snap.state(&cat, &tools, &m), ModelState::NotInstalled);
        touch(&paths.base, &m.files[0]);
        let snap = InstallSnapshot::scan(&paths, &cat, &tools);
        assert_eq!(snap.state(&cat, &tools, &m), ModelState::Partial);
        for f in &m.files {
            touch(&paths.base, f);
        }
        let mut snap = InstallSnapshot::scan(&paths, &cat, &tools);
        assert_eq!(snap.state(&cat, &tools, &m), ModelState::Installed);
        assert!(snap.ready(&cat, &tools, &m), "ncnn model needs no runtime");
        // Recorded with an old version → outdated.
        snap.installed_versions.components.insert("realcugan".into(), "old".into());
        assert_eq!(snap.state(&cat, &tools, &m), ModelState::Outdated);
        assert!(snap.state(&cat, &tools, &m).is_usable());
        let _ = std::fs::remove_dir_all(&paths.base);
    }

    #[test]
    fn onnx_model_needs_runtime_and_helpers() {
        let (paths, cat, tools) = setup("ready");
        let gfp = cat.model("gfpgan-1.4").unwrap().clone();
        for f in &gfp.files {
            touch(&paths.base, f);
        }
        let snap = InstallSnapshot::scan(&paths, &cat, &tools);
        assert_eq!(snap.state(&cat, &tools, &gfp), ModelState::Installed);
        assert!(!snap.ready(&cat, &tools, &gfp));
        // Plan: DirectML runtime + YuNet helper, but not the already-installed model.
        let plan = install_plan(&cat, &tools, &snap, &gfp, RuntimeKind::DirectMl, false);
        let ids: Vec<&str> = plan.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["ort-dml", "onnx-yunet"]);
        // Repair re-fetches the model's own package.
        let plan = install_plan(&cat, &tools, &snap, &gfp, RuntimeKind::DirectMl, true);
        assert!(plan.iter().any(|p| p.id == "onnx-gfpgan"));
        install_files(&paths.base, cat.package("ort-dml").unwrap());
        for f in &cat.model("yunet").unwrap().files {
            touch(&paths.base, f);
        }
        let snap = InstallSnapshot::scan(&paths, &cat, &tools);
        assert!(snap.runtime_dml && !snap.runtime_cuda);
        assert!(snap.ready(&cat, &tools, &gfp));
        // SeedVR2 needs CUDA even when DirectML is installed.
        let seed = cat.model("seedvr2-3b").unwrap();
        let plan = install_plan(&cat, &tools, &snap, seed, RuntimeKind::DirectMl, false);
        assert!(plan.iter().any(|p| p.id == "ort-cuda"));
        assert!(plan.iter().any(|p| p.id == "cuda-cudnn"));
        assert!(plan_size(&plan) > 4_000_000_000);
        let _ = std::fs::remove_dir_all(&paths.base);
    }

    #[test]
    fn uninstall_keeps_shared_packages_and_files() {
        let (paths, cat, tools) = setup("uninst");
        let se = cat.model("realcugan-se").unwrap().clone();
        let pro = cat.model("realcugan-pro").unwrap().clone();
        install_files(&paths.base, cat.package("realcugan").unwrap());
        for f in se.files.iter().chain(&pro.files) {
            touch(&paths.base, f);
        }
        // realesrgan (tools.json) also writes bin/vcomp140.dll.
        let resr = tools.components.iter().find(|c| c.id == "realesrgan").unwrap();
        install_files(&paths.base, resr);
        let snap = InstallSnapshot::scan(&paths, &cat, &tools);
        // Pro still uses the package → nothing removed.
        assert!(uninstall(&paths, &cat, &tools, &snap, &se).unwrap().is_empty());
        std::fs::remove_dir_all(paths.base.join("models/realcugan/models-pro")).unwrap();
        let snap = InstallSnapshot::scan(&paths, &cat, &tools);
        let removed = uninstall(&paths, &cat, &tools, &snap, &se).unwrap();
        assert_eq!(removed, ["realcugan"]);
        assert!(!paths.base.join("bin/realcugan-ncnn-vulkan.exe").exists());
        assert!(paths.base.join("bin/vcomp140.dll").exists(), "shared DLL kept");
        // tools.json packages are never removed by the model manager.
        let x4 = cat.model("realesrgan-x4plus").unwrap();
        let snap = InstallSnapshot::scan(&paths, &cat, &tools);
        assert!(uninstall(&paths, &cat, &tools, &snap, x4).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&paths.base);
    }
}
