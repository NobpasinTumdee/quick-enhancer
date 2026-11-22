# Changelog

All notable changes to Quick Enhancer. Versions follow [Semantic Versioning](https://semver.org);
the version in `Cargo.toml` is the single source of truth (exe resources, installer, About page,
update check).

## [2.1.0] — 2026-11-22

### Added
- **Pages:** All-in-one (unchanged), **Video**, **Image**, **Editor**, Outputs, Settings. Video
  and Image pages show only what applies to them, in processing order (Video: Denoise → Frame
  generation → Upscale → Sharpen → Output), each section with an on/off switch, a Simple /
  Advanced toggle and an always-visible summary line (e.g. `1920×1080 · 30 fps → 3840×2160 ·
  60 fps · NVENC HEVC · est. temp 18 GB · est. 12 min`). Each page has its own inputs and
  settings; all jobs run on the same queue, which shows the page a job came from. Dropping the
  wrong kind of file offers "Open in Image/Video page".
- Image page: **Restore at original size** mode next to Upscale 2/3/4x/target; Photo / Anime /
  Pixel art / General; a Clarity section (CAS, unsharp in Advanced, new **local contrast**
  slider, light denoise).
- **Named settings profiles** per page: built-ins (Anime 1080p → 4K 60 fps, Live action 2x +
  60 fps, Smooth 60 fps only, Light cleanup, Photo restore, Photo 4x, Anime 4x, Pixel art 4x),
  Save / Save as / Rename / Duplicate / Delete, default profile per page (loaded at start),
  recently used list, `*` modified marker with Revert, Export / Import `.toml`. Performance
  values are stored only when "Include performance settings" is ticked; importing such a profile
  checks them against this PC and offers "Adjust to this PC".
- **Resource-aware settings:** a live estimate for the selected file — temp disk per chunk and
  peak (calibrated from real PNG sizes), RAM, VRAM, CPU threads and time — compared with what this
  PC has, with OK / warning / blocking levels, inline messages and **Fix automatically**. The
  same check runs right before a queue starts. A video job now **pauses** (instead of failing)
  when the temp drive runs low and continues once space is free.
- **Benchmark this PC** (~20–60 s, cancellable, synthetic frames; offered once after the first
  tools install) and **Auto-tune for this PC** (tile, ncnn `-j`, threads, RAM budget, chunk
  size; shows what it changed). Results are stored per GPU.
- Settings page with Performance (Simple: preset + auto-tune + benchmark; Advanced: GPU, tile,
  `-j`, threads, priority, chunk size, RAM budget, temp folder, encoder preset and bitrate mode,
  each with a tooltip), Application and About.
- Encoder **preset** (Fast / Balanced / Quality) and **bitrate** rate control.
- **Editor page:** video trim on a thumbnail timeline (playhead, frame step, In/Out buttons and
  I/O keys, `hh:mm:ss.mmm` fields, play the selection with sound), Fast (keyframe, no
  re-encode) or Accurate trim, crop with handles, aspect presets and numeric x/y/w/h (always even);
  **join** clips with drag-to-reorder and per-clip trim — lossless when the clips match,
  otherwise re-encoded to the first clip's size/fps with letterboxing, resampled audio and
  silence for clips without sound; image crop / rotate / flip in the original or another format,
  keeping the colour profile and EXIF when possible; undo/redo (Ctrl+Z / Ctrl+Y); "Send to
  Video/Image page"; edits appear in Outputs marked "✂ Edited".

### Changed
- Outputs tab: wrapping grid with fixed cards and vertical scrolling only, names elided with the
  full name on hover, paths elided in the middle in the list view; only visible rows are laid
  out and only their thumbnails loaded. The queue and progress line truncate long names the
  same way. No horizontal overflow at any window size.
- Application settings and About moved from the side panel to the Settings page.
- Performance settings (device, preset, overrides, chunk size, RAM budget, temp folder) are now
  global for this PC; existing 2.0 settings are migrated automatically.

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
