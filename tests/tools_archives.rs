//! Offline install of the real tool archives listed in `tools.json` (verifies the pinned
//! checksums and that every extraction rule matches the real archive layout).
//!
//! Ignored by default (needs ~600 MB of archives). Run with:
//! `QE_TOOLS_ARCHIVES=<folder with the .zip files> cargo test --test tools_archives -- --ignored`

use quick_enhancer::net::tools::{InstalledTools, Source, ToolsManifest, find_archives_in, install_one};
use quick_enhancer::paths::{AppPaths, ModelSet, ToolSet};
use quick_enhancer::pipeline::cancel::CancelToken;

#[test]
#[ignore]
fn install_real_archives_offline() {
    let dir = std::env::var_os("QE_TOOLS_ARCHIVES").expect("set QE_TOOLS_ARCHIVES");
    let comps = ToolsManifest::embedded().for_platform();
    let found = find_archives_in(std::path::Path::new(&dir), &comps);
    assert_eq!(found.len(), comps.len(), "archives found: {found:?}");

    let root = std::env::temp_dir().join(format!("qe-real-tools-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let paths = AppPaths::from_base(root.clone());
    for (id, archive) in found {
        let comp = comps.iter().find(|c| c.id == id).unwrap();
        let t0 = std::time::Instant::now();
        install_one(&paths, comp, &Source::LocalFile(archive), &CancelToken::new(), &mut |_| {})
            .unwrap_or_else(|e| panic!("{id}: {e}"));
        eprintln!("{id} installed in {:.1}s", t0.elapsed().as_secs_f64());
        // Offline sources are never deleted.
    }
    let tools = ToolSet::discover(&paths);
    let models = ModelSet::discover(&paths.models);
    assert!(paths.bin.join("ffmpeg.exe").is_file());
    assert!(paths.bin.join("ffprobe.exe").is_file());
    assert!(tools.realesrgan.is_some() && tools.rife.is_some());
    assert!(models.has("realesrgan-x4plus") && models.has("realesr-animevideov3-x2"));
    assert!(models.rife_model("rife-v4.6").is_some(), "{:?}", models.rife);
    assert!(paths.base.join("licenses").join("ffmpeg").join("LICENSE.txt").is_file());
    let state = InstalledTools::load(&paths.tools_state_file);
    assert_eq!(state.components.len(), comps.len());
    // ffmpeg actually runs.
    let out = std::process::Command::new(paths.bin.join("ffmpeg.exe"))
        .arg("-version")
        .output()
        .unwrap();
    assert!(out.status.success());
    let _ = std::fs::remove_dir_all(&root);
}

/// Real HTTPS download + verify + install of the smallest component (Real-ESRGAN, 45 MB).
/// `cargo test --test tools_archives download_realesrgan -- --ignored`
#[test]
#[ignore]
fn download_realesrgan_over_https() {
    let comps = ToolsManifest::embedded().for_platform();
    let comp = comps.iter().find(|c| c.id == "realesrgan").unwrap();
    let root = std::env::temp_dir().join(format!("qe-dl-tools-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let paths = AppPaths::from_base(root.clone());
    let mut last = 0u64;
    let mut updates = 0;
    install_one(&paths, comp, &Source::Download, &CancelToken::new(), &mut |s| {
        if let quick_enhancer::net::tools::CompStatus::Downloading(p) = s {
            updates += 1;
            last = p.done;
        }
    })
    .unwrap();
    assert_eq!(last, comp.size);
    assert!(updates >= 1);
    assert!(ToolSet::discover(&paths).realesrgan.is_some());
    // The verified archive is removed after a successful install.
    assert!(!paths.base.join("downloads").join(comp.file_name()).exists());
    let _ = std::fs::remove_dir_all(&root);
}
