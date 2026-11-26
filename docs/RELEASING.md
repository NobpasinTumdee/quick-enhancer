# Releasing Quick Enhancer

Releases are published on **GitHub Releases**. The app's update check downloads
`https://github.com/<GITHUB_REPO>/releases/latest/download/latest.json`, so every release must
attach a `latest.json` next to the installer. `scripts\release.ps1` produces both.

## One-time setup

| What | Where | Current value |
|---|---|---|
| GitHub repository (update check, Releases links in the app) | `src/net/mod.rs` → `GITHUB_REPO` | `NobpasinTumdee/quick-enhancer` |
| Publisher name and project URL (installer, Apps & features) | `installer/QuickEnhancer.iss` → `AppPublisher`, `AppRepoUrl` | `NobpasinTumdee`, `https://github.com/NobpasinTumdee/quick-enhancer` |
| Company name in the exe's version info | `build.rs` → `CompanyName`, `LegalCopyright` | `NobpasinTumdee`, `Quick Enhancer contributors` |
| Crate metadata | `Cargo.toml` → `authors`, `license` (+ add `repository`) | `Quick Enhancer contributors`, `MIT` |
| App icon | `assets/icon.ico` (16–256 px) and `assets/icon-256.png` (window icon), from `assets/logo.png` | `cargo run --release --example make_icon -- assets/logo.png` |
| Code signing | `scripts/release.ps1` → `Invoke-CodeSign`; optionally `SignTool=` in the `.iss` | does nothing |

**Never change `AppId`** in the `.iss` file: upgrades and uninstall are keyed on it.

## Release checklist

1. [ ] All changes merged; working tree clean.
2. [ ] Bump `version` in `Cargo.toml` (semver `MAJOR.MINOR.PATCH`). This is the only place.
3. [ ] Add a `## [x.y.z] — YYYY-MM-DD` section to `CHANGELOG.md`.
4. [ ] If a tool changed: update `tools.json` (see below) and run the archive test.
   If the model catalog changed: publish the models release / `models.json` first (see *Models
   release*) and run the real model tests (`cargo test --test models_real -- --ignored`).
5. [ ] If UI text changed: both `locales/en.toml` and `locales/th.toml` are updated (the tests
   check this).
6. [ ] Run the release script (clippy + tests + build + MinGW DLL check + installer +
   `portable.txt` check + checksum + manifest):

   ```powershell
   powershell -ExecutionPolicy Bypass -File scripts\release.ps1 `
       -NotesEnFile notes-en.txt -NotesThFile notes-th.txt [-ToolsChanges rife]
   ```

   Short notes can be given inline with `-NotesEn "..." -NotesTh "..."`. `-MinSupported 2.0.0`
   sets the oldest version allowed to update in place (older ones are told to download
   manually). Output in `dist\`: the installer, `<installer>.sha256` and `latest.json`.
7. [ ] Smoke test on a clean Windows user account or VM:
   * Run `dist\QuickEnhancer-Setup-x.y.z.exe`, download the tools, process one image and one
     video, check the Outputs tab.
   * **Update test**: install the *previous* version, then serve the new files locally. Copy
     `dist\` to a test folder, edit `installer_url` in its `latest.json` to
     `http://127.0.0.1:8765/QuickEnhancer-Setup-x.y.z.exe`, and:

     ```powershell
     python -m http.server 8765 --bind 127.0.0.1 --directory <test folder>   # other terminal
     $env:QUICK_ENHANCER_UPDATE_URL = "http://127.0.0.1:8765/latest.json"
     & "$env:LOCALAPPDATA\Programs\QuickEnhancer\QuickEnhancer.exe"
     ```

     Settings → Check for updates → Download and install → the app restarts as the new version
     with settings, history and tools intact.
   * Uninstall from Windows Settings → Apps; the "also delete your data?" question defaults to
     *No*.
8. [ ] Tag and publish:

   ```powershell
   git tag vX.Y.Z; git push origin vX.Y.Z
   gh release create vX.Y.Z dist\QuickEnhancer-Setup-X.Y.Z.exe dist\QuickEnhancer-Setup-X.Y.Z.exe.sha256 dist\latest.json `
       --title "Quick Enhancer X.Y.Z" --notes-file notes-en.txt
   ```

   The release must be **published** (not a draft or pre-release) for `releases/latest/download/`
   to point at it, and the tag must be `v` + the version (the URL in `latest.json` uses it). Keep
   the asset names exactly as produced.
9. [ ] Check that `https://github.com/NobpasinTumdee/quick-enhancer/releases/latest/download/latest.json`
   returns the new version and that an older install finds the update.

The icon and version info are embedded by `build.rs` itself (it writes the `.rsrc` COFF object),
so no `rc.exe` / `windres` is needed with either the GNU or the MSVC toolchain.

**Testing an update without publishing:** build a higher version, write a `latest.json` whose
`installer_url` is a `file:///…` path to that installer, and start the installed app with
`QUICK_ENHANCER_UPDATE_URL=file:///…/latest.json`. The whole flow (check → download → SHA-256 →
silent install → restart) runs exactly as with GitHub.

If something is wrong after publishing, publish a fixed higher version. Don't replace the assets
of an existing release: installed copies may already have checked its SHA-256.

## `latest.json` format

```json
{
  "version": "2.1.0",
  "published": "2026-12-01",
  "notes_en": "What's new, in English (plain text, newlines allowed).",
  "notes_th": "มีอะไรใหม่ (ภาษาไทย)",
  "installer_url": "https://github.com/OWNER/REPO/releases/download/v2.1.0/QuickEnhancer-Setup-2.1.0.exe",
  "sha256": "<64 lowercase hex characters: SHA-256 of the installer>",
  "installer_size": 8886009,
  "min_supported_version": "2.0.0",
  "tools_changes": ["rife"]
}
```

* `version` may have a `v` prefix; `2.1` means `2.1.0`. The update is offered only when it is
  greater than the running version.
* The downloaded installer is verified against `sha256` before it is run; a mismatch aborts the
  update and nothing is installed.
* The installer runs with `/SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS` and relaunches
  the app when done. User data lives outside the program folder and is never touched.
* `tools_changes` is only displayed. What actually happens: after updating, the new app compares
  its embedded `tools.json` with `tools-installed.json` and offers to re-download exactly the
  components whose `version` changed (tools placed by hand are left alone).
* Without network access, or if GitHub is unreachable, the app keeps working; the update window
  shows the error and a link to the Releases page.

## Updating `tools.json`

`tools.json` (project root) pins every downloadable tool. It is compiled into the exe, so a change
ships with the next app release. One component (abridged; RIFE also copies its other model folders):

```json
{
  "id": "rife",
  "name": "RIFE ncnn-vulkan + models",
  "version": "20221029",
  "required": false,
  "platform": "windows-x64",
  "url": "https://github.com/nihui/rife-ncnn-vulkan/releases/download/20221029/rife-ncnn-vulkan-20221029-windows.zip",
  "sha256": "<archive SHA-256, lowercase hex>",
  "size": 431540241,
  "archive": "zip",
  "license": "MIT (rife-ncnn-vulkan, nihui; RIFE models, hzwer / Megvii)",
  "homepage": "https://github.com/nihui/rife-ncnn-vulkan",
  "files": [
    { "from": "*/rife-ncnn-vulkan.exe", "to": "bin/rife-ncnn-vulkan.exe" },
    { "from": "*/rife-v4.6/", "to": "models/rife/rife-v4.6/" },
    { "from": "*/LICENSE", "to": "licenses/rife/LICENSE" }
  ]
}
```

* `id` is stable: never rename it.
* `version` must change whenever `url` / `sha256` change. That is what triggers the re-download
  on users' machines.
* `files`: `from` is a path inside the archive where `*` matches exactly one folder level; a
  trailing `/` copies a whole folder. `to` is relative to the tools folder. **Every rule must
  match something**, otherwise the install fails before anything is moved into place, so a
  changed archive layout is caught instead of half-installing.
* Only `zip` archives are supported.

Steps:

1. Download the new archive from the project's **official** release page.
2. Size and checksum:

   ```powershell
   (Get-Item .\new.zip).Length
   (Get-FileHash .\new.zip -Algorithm SHA256).Hash.ToLower()
   ```

3. Look at the layout (`tar -tf new.zip | more`) and adjust `files`.
4. Update `url`, `sha256`, `size`, **`version`**, and `license` if needed; update
   `licenses/THIRD_PARTY.md` as well.
5. Test with the real archive, offline. Put it in a folder under the file name its URL ends with:

   ```powershell
   $env:QE_TOOLS_ARCHIVES = "D:\archives"
   cargo test --test tools_archives -- --ignored install_real_archives_offline
   ```

   Then `cargo test` (a unit test validates the embedded manifest) and run the app once with
   *Manage tools* to watch it install.
6. Pass `-ToolsChanges <id>` to `release.ps1`.

## Models release (`models-YYYY.MM`)

Converted model files (ONNX / ncnn), the slim RIFE repack and `models.json` are hosted on a
separate GitHub release, e.g. `models-2026.11`. It is **not** an app release: the app's update
check reads `releases/latest`, so this release must never become "Latest".

Only upload files whose license allows redistribution (`license.redistribute` in the catalog;
the catalog test fails if a non-redistributable model points at our release). Models with a
non-commercial license (CodeFormer) may be hosted only with their license text and are labelled
"Non-commercial" in the app.

1. Convert / export the models with the scripts in `tools/convert/` (the
   [README](../tools/convert/README.md) has the recipe for every published file), check each with
   `compare.py` against the PyTorch weights, and try it in the app (drop it into
   `models/custom/`) on DirectML and on the CPU.
2. Update the table in `tools/catalog/gen_models_json.py` (file name, SHA-256, size, license),
   bump `catalog_version`, and regenerate the catalog:

   ```sh
   sha256sum <files>; stat -c %s <files>
   python tools/catalog/gen_models_json.py models.json
   cargo test models::
   ```

3. Put the files, `models.json`, a `MODELS-LICENSES.md` (file → original model, author, license,
   changes made) and each license text (CC-BY-4.0, Apache-2.0, MIT, S-Lab …) in one folder, then
   create the release **not as a draft and not as latest**:

   ```powershell
   gh release create models-2026.11 --latest=false --title "Models 2026.11" --notes-file notes.md <files…>
   ```

4. Check that `releases/latest` still points at the app release, and that a download matches its
   hash:

   ```powershell
   gh api repos/NobpasinTumdee/quick-enhancer/releases/latest --jq .tag_name   # → v2.x.y
   ```

5. To publish a newer catalog without an app update, upload the new `models.json` to the release
   that `update_url` points at (`gh release upload models-2026.11 models.json --clobber`). The app
   uses it only when its `schema` matches and its `catalog_version` is newer than the embedded one.
   Never replace a model file in place: a new file needs a new name (old catalogs pin the hash).

**New catalog schema → new models release.** When `models.json` gets fields or groups an older app
can't read, bump `schema` in the generator and point `update_url` at a **new** models release
(2.3: schema 2 on `models-2026.12`). The older release keeps its schema 1 `models.json`
unchanged, so installed older versions keep refreshing from it; files already on the older
release stay where they are and the new catalog keeps their URLs.

**Files over 2 GB** (GitHub's asset limit), e.g. SeedVR2's 3.56 GB `dit.onnx.data`, are
downloaded from their original host at a pinned revision (a Hugging Face `resolve/<commit>/` URL)
with the SHA-256 and size in the catalog; check that each URL still answers before a release:

```sh
curl -sSIL <url> | grep -i content-length
```

**Real-model tests for Make sharp:** besides `models_real`, run
`cargo test --release --test sharp_real -- --ignored --nocapture` with `QE_TEST_BASE` (a tools
root with the ONNX runtime and the Make sharp models) and `QE_TEST_PAIRS` (the evaluation's
blurry / sharp pairs, see `docs/DEBLUR_EVAL.md`); `QE_TEST_CREATIVE=0` skips the slow generative
runs.

## Code signing and SmartScreen

Unsigned installers and exes work, but Windows **SmartScreen** shows "Windows protected your PC"
(the user has to click *More info → Run anyway*), and some antivirus products treat unsigned
downloads with more suspicion. Options:

* An **OV or EV code-signing certificate** from a CA (Sectigo, DigiCert, SSL.com, …). The private
  key must live on a hardware token or a cloud HSM, so signing goes through the vendor's tool or
  `signtool` with the token. Signed files build SmartScreen reputation as people download them;
  the first releases can still show a warning.
* **Azure Trusted Signing** (Microsoft's managed signing service; monthly fee; identity
  validation) — `signtool` with its dlib plugin.
* Open-source projects can apply to the **SignPath Foundation** for free signing.

Wiring it up:

1. Implement `Invoke-CodeSign` in `scripts/release.ps1` (an example `signtool` command is in the
   comment). The script signs the exe **before** it is packed into the installer, then signs the
   installer.
2. To sign the uninstaller too, uncomment `SignTool=qesign` and `SignedUninstaller=yes` in
   `installer/QuickEnhancer.iss` and pass the sign tool to ISCC, e.g.
   `"/Sqesign=signtool.exe sign /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 /f cert.pfx `$f"`
   (add it to the ISCC call in `release.ps1`).
3. Always timestamp (`/tr`) so signatures stay valid after the certificate expires.
4. Verify: `signtool verify /pa /v dist\QuickEnhancer-Setup-x.y.z.exe`.

Never commit certificates or passwords; read them from environment variables or a secret store.
