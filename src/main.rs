#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use quick_enhancer::app::{App, StartOptions};
use quick_enhancer::config::Config;
use quick_enhancer::i18n;
use quick_enhancer::paths::{AppPaths, PathEnv};
use quick_enhancer::pipeline::temp::{cleanup_stale, legacy_temp_root, temp_root};

/// Window icon (the same artwork as the exe icon).
fn window_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../assets/icon-256.png");
    let img = image::load_from_memory(bytes).ok()?.into_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    })
}

fn main() -> anyhow::Result<()> {
    let env = PathEnv::current();
    let paths = AppPaths::resolve_with(&env);
    paths.ensure_dirs();
    let (_log_guard, ui_log) = quick_enhancer::logging::init(&paths.logs);
    tracing::info!(
        "Quick Enhancer {} starting · {:?} mode · data {} · tools {}",
        env!("CARGO_PKG_VERSION"),
        paths.mode,
        paths.data.display(),
        paths.base.display()
    );
    if let Some(from) = paths.migrate_legacy_config(&env) {
        tracing::info!("migrated settings from {}", from.display());
    }

    let mut config = Config::load(&paths.config_file);
    // Language: saved choice, else Thai when Windows' UI language is Thai.
    let lang = config.language.unwrap_or_else(i18n::system_default);
    i18n::set_lang(lang);
    config.language = Some(lang);

    let mut start = StartOptions::default();
    for a in std::env::args_os().skip(1) {
        match a.to_str() {
            Some("--install-tools") => start.install_tools = true,
            Some("--font-test") => start.font_test = true,
            // Files / folders passed on the command line (or dropped onto the exe).
            _ => start.files.push(a.into()),
        }
    }
    // Remove temp folders left by crashed / killed runs (current and pre-2.0 names).
    let custom = config.settings.video.temp_dir.clone();
    for c in [None, custom.as_deref()] {
        cleanup_stale(&temp_root(c));
        cleanup_stale(&legacy_temp_root(c));
        if custom.is_none() {
            break;
        }
    }

    // Installers downloaded by a previous update (the installer may still be finishing,
    // in which case its file is locked and stays until the next start).
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join("QuickEnhancer-update"));

    let (size, min) = if start.install_tools {
        ([900.0, 640.0], [700.0, 480.0])
    } else {
        ([1560.0, 960.0], [1100.0, 650.0])
    };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Quick Enhancer")
        .with_inner_size(size)
        .with_min_inner_size(min)
        .with_drag_and_drop(true);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Quick Enhancer",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, paths, config, ui_log, start)))),
    )
    .map_err(|e| anyhow::anyhow!("failed to start the UI: {e}"))?;
    Ok(())
}
