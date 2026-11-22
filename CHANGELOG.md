# Changelog

All notable changes to Quick Enhancer. Versions follow [Semantic Versioning](https://semver.org);
the version in `Cargo.toml` is the single source of truth (exe resources, installer, About page,
update check).

## [2.0.0] — 2026-11-22

The project is now **Quick Enhancer** (previously "Enhancer").

### Added
- **Thai language.** The whole interface is available in English and Thai (ไทย). Switch live in
  Settings → Application → Language; the choice is saved. On first start Thai is chosen
  automatically when the Windows display language is Thai. Thai text uses the embedded Noto Sans
  Thai font (SIL OFL 1.1). A "Thai font test" window (Settings, or `--font-test`) shows the
  tricky combinations.
- **Sound in the sample preview.** The sample player plays the clip's audio and uses it as the
  master clock, so both sides stay in sync while looping. Play/pause, restart, loop, ¼ speed,
  mute and volume (mute/volume/loop are remembered). Clips without audio, or PCs without an audio
  device, play silently with a note instead of failing.
- Each preview side shows its FPS and resolution, plus the frame rate actually displayed. A notice
  explains when the enhanced FPS is higher than the monitor's refresh rate. The still compare
  also shows each side's resolution.
- **Outputs tab** ("ผลงาน"): every finished job is recorded in `history.json`; the tab merges the
  history with the media files found in the output folder (when an output folder is set),
  removes duplicates and marks missing files. Grid or list view, sort, filter (images/videos),
  search, cached thumbnails, a details panel, Open, Show in Explorer, Compare with source, and
  Delete (to the Recycle Bin, with confirmation).
- **Installer** (Inno Setup 6): per-user install by default (all-users optional), Start-menu and
  optional desktop shortcut, English/Thai, third-party notices page, uninstaller that asks
  whether to delete your data (default: keep).
- **Tool downloader** (installer, first start, Settings → Manage tools, after updates): choose
  components, see sizes and licenses, per-item progress with speed, cancel and retry. Every
  archive is checked against a pinned SHA-256 before anything is extracted, and files are moved
  into place only after a complete, verified extraction. Offline install from archives you
  downloaded yourself.
- **Updates** from GitHub Releases: "Check for updates" in Settings, optional check at startup
  (off by default; asked once). Downloads the new installer, verifies it, installs silently and
  restarts. Refuses to update while a job is running. Settings, history and tools are kept; tools
  are re-downloaded only when their pinned version changes. No telemetry.
- Settings → About / Paths shows the version, install mode and every folder the app uses, with
  "Open folder" buttons.
- Windows exe icon and version information.

### Changed
- Executable renamed to `QuickEnhancer.exe`.
- Data locations: an installed copy keeps settings, history, thumbnails and logs in
  `%APPDATA%\QuickEnhancer` and tools/models in `%LOCALAPPDATA%\QuickEnhancer`. A
  `portable.txt` next to the exe (or `cargo run`) keeps everything next to the exe as before.
- The environment variable is now `QUICK_ENHANCER_HOME` (`ENHANCER_HOME` still works).
- Error messages shown in the UI are translated and explain what to do.

### Migration
- An existing `config.toml` from Enhancer 1.x (next to the old exe, or `%APPDATA%\Enhancer`) is
  imported once on first start.
- Temp folders from 1.x (`enhancer-tmp`) are cleaned up like the new `quick-enhancer-tmp` ones.

## [1.0.0]
- First version ("Enhancer"): image and video upscaling (Real-ESRGAN ncnn), frame generation
  (RIFE ncnn), denoise/sharpen filters, chunked video pipeline, before/after preview, resource
  monitor.
