# Building Quick Enhancer

Quick Enhancer is a single Rust crate (`quick-enhancer`, binary `QuickEnhancer.exe`). The
version in `Cargo.toml` is the single source of truth: `build.rs` writes it into the exe's version
resource, the installer reads it from there, and the app shows it in Settings → About.

## Requirements

* **Rust** stable, edition 2024 (1.85 or newer) — <https://rustup.rs>
* **Windows 10/11 x64** (the app also builds on Linux for development; Windows-only parts are
  behind `cfg(windows)`)
* A GPU driver with Vulkan for running the AI tools (not needed to build)
* **Inno Setup 6** (only for the installer) — <https://jrsoftware.org/isinfo.php>. `ISCC.exe` is
  usually in `C:\Program Files (x86)\Inno Setup 6\` or, for a per-user install,
  `%LOCALAPPDATA%\Programs\Inno Setup 6\`.

You need **one** of the two Windows toolchains below.

### Option A — MSVC (the usual choice)

1. Install *Visual Studio Build Tools 2022* (or Visual Studio) with the workload
   **Desktop development with C++** (MSVC v143 + Windows 11 SDK).
2. `rustup default stable-x86_64-pc-windows-msvc`
3. Build from a normal terminal (PowerShell or cmd):

   ```powershell
   cargo build --release
   ```

The icon and version info are embedded by `build.rs` itself (it writes the `.rsrc` COFF
object), so no `rc.exe` / `windres` is needed with either toolchain.

### Option B — GNU (MinGW-w64)

Works without Visual Studio, but needs a **complete** MinGW-w64 toolchain on `PATH`: some
dependencies (`ort`, `libloading`) link through `raw-dylib`, which needs `dlltool` **and** `as`.
The toolchain bundled with rustup's GNU target is not enough. A
[winlibs](https://winlibs.com/) release (UCRT, x86_64, posix threads) works:

1. Unpack winlibs, e.g. to `%LOCALAPPDATA%\mingw-winlibs` (it contains `mingw64\bin`).
2. `rustup default stable-x86_64-pc-windows-gnu`
3. Put `mingw64\bin` **before** everything else on `PATH`, then build. In Git Bash:

   ```sh
   export PATH="$HOME/.cargo/bin:$LOCALAPPDATA/mingw-winlibs/mingw64/bin:$PATH"
   cargo build --release
   ```

   In PowerShell:

   ```powershell
   $env:PATH = "$env:LOCALAPPDATA\mingw-winlibs\mingw64\bin;$env:PATH"
   cargo build --release
   ```

`.cargo/config.toml` links libgcc and winpthread **statically**, so `QuickEnhancer.exe` needs no
MinGW DLLs (`libgcc_s_seh-1.dll`, `libwinpthread-1.dll`, `libstdc++-6.dll`). `scripts/release.ps1`
checks this with `objdump -p` and fails if one of them is imported. To check by hand:

```sh
objdump -p target/release/QuickEnhancer.exe | grep "DLL Name"
```

The ONNX Runtime (`onnxruntime.dll`, DirectML or CUDA) is **not** linked at build time: the `ort`
crate uses `load-dynamic`, and the app loads the DLL that the Model Manager downloaded into
`runtime/dml/` or `runtime/cuda/`. Building needs no ONNX Runtime files. At run time the DLL needs
the Microsoft Visual C++ 2015–2022 redistributable (x64), which most PCs already have; the load
error message links to it.

## Running during development

```sh
cargo run --release -- [files…]
```

`cargo run` is always **portable**: settings, history, logs, `bin/` and `models/` are taken from
the project folder. Put the tools there by hand or press *Manage tools* in the app to download them
into the project folder. `QUICK_ENHANCER_HOME=<dir>` overrides the folder.

Useful switches and variables:

| | |
|---|---|
| `--install-tools` | only the setup wizard (what the installer runs) |
| `--font-test` | opens the Thai font test window |
| `QUICK_ENHANCER_HOME` | put all data and tools into this folder (`ENHANCER_HOME` = old alias) |
| `QUICK_ENHANCER_UPDATE_URL` | read `latest.json` from this URL instead of GitHub (testing updates) |
| `RUST_LOG=debug` | more detailed logs (`logs/` in the data folder) |

## Tests and lints

```sh
cargo clippy --all-targets -- -D warnings
cargo test
```

* Unit tests run everywhere. Integration tests in `tests/pipeline_filters.rs` need
  `bin/ffmpeg.exe` (and, for the AI cases, the ncnn tools, models and a Vulkan GPU) in the
  project folder; they skip themselves otherwise.
* `tests/tools_archives.rs` holds two `#[ignore]`d tests that are run on demand:

  ```sh
  # Install the real archives named in tools.json from a local folder (offline, checks SHA-256)
  QE_TOOLS_ARCHIVES=D:/archives cargo test --test tools_archives -- --ignored install_real_archives_offline
  # Download the smallest component over HTTPS and verify it (needs internet)
  cargo test --test tools_archives -- --ignored download_realesrgan_over_https
  ```

* The i18n tests fail if a key exists in only one of `locales/en.toml` / `locales/th.toml`, if a
  placeholder differs between them, or if the code uses a `t!("…")` key that doesn't exist.
* `tests/models_real.rs` (`#[ignore]`) downloads real models from the catalog into
  `target/it-scratch/` (reused between runs) and runs them. They need `bin/ffmpeg.exe` and a GPU;
  the face test also needs the reference images in the git-ignored `.feasibility/` folder and
  skips itself without them:

  ```sh
  cargo test --test models_real -- --ignored --nocapture                 # all (~1.5 GB download)
  cargo test --test models_real onnx_models -- --ignored --nocapture     # DirectML vs CPU, video in memory
  ```

## Model catalog and conversion

* `models.json` (embedded into the exe, and published on the models release so the app can
  refresh it) is generated by `python tools/catalog/gen_models_json.py models.json`. Edit the
  script, not the JSON. Bump `catalog_version` whenever the published file changes.
* `tools/convert/` holds the developer-only scripts that turn PyTorch weights into the ONNX /
  ncnn files on the models release, with the exact recipe for each file
  ([README](../tools/convert/README.md)). End users never need Python.
* Publishing model files: [RELEASING.md](RELEASING.md) → *Models release*.

## Building the installer

```powershell
cargo build --release
& "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe" installer\QuickEnhancer.iss
# → dist\QuickEnhancer-Setup-<version>.exe
```

The script reads the version from `target\release\QuickEnhancer.exe`. To override it (e.g. a test
build, or an exe without version info): `ISCC /DAppVersion=2.0.1 installer\QuickEnhancer.iss`.
`scripts\release.ps1` does build + installer + checksum + `latest.json` in one go (see
[RELEASING.md](RELEASING.md)).

The installer does not contain FFmpeg, Real-ESRGAN, RIFE, ONNX Runtime or any model; it runs
`QuickEnhancer.exe --install-tools` at the end, which opens the setup wizard and downloads the
pinned files listed in `tools.json` and `models.json` (both embedded into the exe at build time).

## Third-party license list for Rust crates

`licenses/THIRD_PARTY.md` lists the tools, runtimes, AI models and the font. For a full list of crate licenses:

```sh
cargo install cargo-about
cargo about init        # once, creates about.toml
cargo about generate about.hbs > licenses/rust-crates.html
```

(or `cargo install cargo-license && cargo license`). Files in `licenses/` are installed next to the
app automatically.

## Project layout (short)

```
src/main.rs          entry point: data paths, logging, language, command line
src/app.rs           application state and event pumping
src/ui/              egui panels (queue, settings, preview, outputs, AI models, setup wizard,
                     tools, update, font test)
src/pipeline/        image / video jobs, chunked video pipeline, runner
src/engines/         Real-ESRGAN / Real-CUGAN / RIFE CLI wrappers, Lanczos fallback
src/engines/onnx/    ONNX Runtime loading, tiling, ONNX upscaler, face restoration
src/models/          model catalog, install state, custom models, GPU fit
src/media/           ffprobe / ffmpeg helpers (incl. audio decode for the preview)
src/preview/         preview worker, canvas, sample player, audio output
src/net/             ALL network code: tools downloader, update check, HTTP helper
src/i18n.rs          t! macro, language switch; strings in locales/*.toml
src/paths.rs         installed / portable data locations, migration from 1.x
src/history.rs       history.json; src/outputs.rs  Outputs tab state + thumbnails
installer/           Inno Setup script
tools.json           pinned tool downloads (URL, SHA-256, size, license, files)
models.json          model catalog (generated by tools/catalog/gen_models_json.py)
tools/convert/       developer-only model conversion scripts
```

See `PLAN.md` for the design notes.
