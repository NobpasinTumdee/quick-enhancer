# Builds a Quick Enhancer release: exe -> (sign) -> installer -> (sign) -> SHA-256 -> latest.json.
#
#   powershell -ExecutionPolicy Bypass -File scripts\release.ps1 `
#       -NotesEn "Bug fixes." -NotesTh "<Thai notes>" [-ToolsChanges rife] [-SkipChecks]
#
# Output in dist\:  QuickEnhancer-Setup-<ver>.exe, QuickEnhancer-Setup-<ver>.exe.sha256, latest.json
# Upload all three to a GitHub release named v<ver> (see docs\RELEASING.md). The version comes
# from Cargo.toml - bump it there (and in CHANGELOG.md) before running this script.
param(
    [string]$NotesEn = "",
    [string]$NotesTh = "",
    # Files with longer release notes (UTF-8); override -NotesEn / -NotesTh when given.
    [string]$NotesEnFile = "",
    [string]$NotesThFile = "",
    # Oldest installed version allowed to update in place (older ones are sent to the Releases page).
    [string]$MinSupported = "2.0.0",
    # Ids of tools.json components whose version changed in this release (informational).
    [string[]]$ToolsChanges = @(),
    # GitHub "owner/repo". Default: read from src\net\mod.rs (GITHUB_REPO).
    [string]$Repo = "",
    [string]$Iscc = "",
    [switch]$SkipChecks,
    [switch]$SkipBuild
)
$ErrorActionPreference = "Stop"
$root = Resolve-Path "$PSScriptRoot\.."
Set-Location $root

# ---------------------------------------------------------------------------------------------
# CODE SIGNING - PLACEHOLDER. Unsigned builds work but trigger SmartScreen warnings.
# Fill in this function to sign a file in place, e.g. with signtool from the Windows SDK:
#   & signtool.exe sign /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 `
#       /f C:\path\to\cert.pfx /p $env:QE_CERT_PASSWORD $Path
# or with a cloud HSM / Azure Trusted Signing client. Leave it empty to skip signing.
function Invoke-CodeSign([string]$Path) {
    # TODO: sign $Path
    Write-Host "  (signing skipped - placeholder in scripts\release.ps1)" -ForegroundColor Yellow
}
# ---------------------------------------------------------------------------------------------

function Step([string]$m) { Write-Host "==> $m" -ForegroundColor Cyan }

# Version from Cargo.toml ([package] section, first `version = "..."`).
$cargo = Get-Content "$root\Cargo.toml" -Raw
if ($cargo -notmatch '(?ms)^\[package\].*?^version\s*=\s*"([^"]+)"') { throw "version not found in Cargo.toml" }
$version = $Matches[1]
Step "Quick Enhancer $version"

if (-not $Repo) {
    $net = Get-Content "$root\src\net\mod.rs" -Raw
    if ($net -match 'GITHUB_REPO:\s*&str\s*=\s*"([^"]+)"') { $Repo = $Matches[1] }
}
if (-not $Repo -or $Repo -eq "OWNER/REPO") {
    Write-Warning "GITHUB_REPO is still the placeholder 'OWNER/REPO' (src\net\mod.rs). The update check will not work until it is set."
    if (-not $Repo) { $Repo = "OWNER/REPO" }
}
$iss = Get-Content "$root\installer\QuickEnhancer.iss" -Raw
if ($iss -match 'Quick Enhancer Publisher|OWNER/REPO') {
    Write-Warning "installer\QuickEnhancer.iss still contains placeholders (AppPublisher / AppRepoUrl)."
}
if (-not (Select-String -Path "$root\CHANGELOG.md" -Pattern "\[$([regex]::Escape($version))\]" -Quiet)) {
    Write-Warning "CHANGELOG.md has no entry for [$version]."
}

if (-not $SkipBuild) {
    if (-not $SkipChecks) {
        Step "cargo clippy"
        cargo clippy --all-targets -- -D warnings
        if ($LASTEXITCODE -ne 0) { throw "clippy failed" }
        Step "cargo test"
        cargo test
        if ($LASTEXITCODE -ne 0) { throw "tests failed" }
    }
    Step "cargo build --release"
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "build failed" }
}
$exe = "$root\target\release\QuickEnhancer.exe"
if (-not (Test-Path $exe)) { throw "$exe not found" }
$exeVer = (Get-Item $exe).VersionInfo.ProductVersion
if ($exeVer -and $exeVer -ne $version) { throw "exe version $exeVer does not match Cargo.toml $version (stale build?)" }

Step "Check that the exe imports no MinGW runtime DLLs"
$objdump = (Get-Command objdump -ErrorAction SilentlyContinue).Source
$wl = Join-Path $env:LOCALAPPDATA "mingw-winlibs\mingw64\bin\objdump.exe"
if (-not $objdump -and (Test-Path $wl)) { $objdump = $wl }
if ($objdump) {
    $imports = & $objdump -p $exe 2>$null | Select-String "DLL Name:\s*(\S+)" | ForEach-Object { $_.Matches[0].Groups[1].Value }
    if (-not $imports) { throw "objdump returned no imports for $exe" }
    $bad = $imports | Where-Object { $_ -match '^(libgcc_s_.*|libwinpthread-.*|libstdc\+\+-.*)\.dll$' }
    if ($bad) { throw "QuickEnhancer.exe depends on MinGW runtime DLLs: $($bad -join ', ') (static linking in .cargo/config.toml not applied?)" }
    Write-Host "  imports: $($imports -join ', ')"
} elseif ((rustc -vV 2>$null) -match 'host: .*windows-gnu') {
    throw "objdump not found: cannot verify the GNU build (put WinLibs mingw64\bin on PATH)"
} else {
    Write-Host "  (objdump not found - skipped)" -ForegroundColor Yellow
}

Step "Sign exe"
Invoke-CodeSign $exe

Step "Build installer"
if (-not $Iscc) {
    $Iscc = @(
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
    ) | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $Iscc) { throw "ISCC.exe (Inno Setup 6) not found; pass -Iscc <path>" }
& $Iscc /Q "/DAppVersion=$version" "$root\installer\QuickEnhancer.iss"
if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }
$setupName = "QuickEnhancer-Setup-$version.exe"
$setup = "$root\dist\$setupName"
if (-not (Test-Path $setup)) { throw "$setup not produced" }

Step "Sign installer"
Invoke-CodeSign $setup

Step "SHA-256"
$item = Get-Item $setup
$sha = (Get-FileHash $setup -Algorithm SHA256).Hash.ToLowerInvariant()
[System.IO.File]::WriteAllText("$setup.sha256", "$sha  $setupName`n")
Write-Host "  $sha"

Step "latest.json"
if ($NotesEnFile) { $NotesEn = [System.IO.File]::ReadAllText((Resolve-Path $NotesEnFile)) }
if ($NotesThFile) { $NotesTh = [System.IO.File]::ReadAllText((Resolve-Path $NotesThFile)) }
$manifest = [ordered]@{
    version               = $version
    published             = (Get-Date -Format "yyyy-MM-dd")
    notes_en              = $NotesEn
    notes_th              = $NotesTh
    installer_url         = "https://github.com/$Repo/releases/download/v$version/$setupName"
    sha256                = $sha
    installer_size        = $item.Length
    min_supported_version = $MinSupported
    tools_changes         = @($ToolsChanges)
}
$json = $manifest | ConvertTo-Json -Depth 4
# UTF-8 without BOM.
[System.IO.File]::WriteAllText("$root\dist\latest.json", $json, (New-Object System.Text.UTF8Encoding $false))

Step "Done"
Write-Host "  dist\$setupName  ($([math]::Round($item.Length / 1MB, 1)) MB)"
Write-Host "  dist\$setupName.sha256"
Write-Host "  dist\latest.json"
Write-Host ""
Write-Host "Next: create the GitHub release tag v$version and upload these three files."
Write-Host "  gh release create v$version dist\$setupName dist\$setupName.sha256 dist\latest.json --title `"Quick Enhancer $version`" --notes-file <notes.md>"
