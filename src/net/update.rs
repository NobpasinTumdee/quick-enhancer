//! App updates from GitHub Releases. Each release carries the installer `.exe` and a
//! `latest.json` manifest; the app downloads the installer to temp, verifies its SHA-256
//! and runs it silently (it upgrades in place, keeps settings / history / tools and
//! relaunches the app). Only runs when the user asks or opted in to the startup check.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use super::http::{self, NetError, Progress};
use crate::pipeline::cancel::CancelToken;

/// `latest.json`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ReleaseManifest {
    pub version: String,
    #[serde(default)]
    pub published: Option<String>,
    #[serde(default)]
    pub notes_en: String,
    #[serde(default)]
    pub notes_th: String,
    pub installer_url: String,
    pub sha256: String,
    #[serde(default)]
    pub installer_size: Option<u64>,
    /// Oldest version that can update in place; older ones must install manually.
    #[serde(default)]
    pub min_supported_version: Option<String>,
    /// Components whose `tools.json` entry changed in this release (informational; the new
    /// app re-downloads exactly the components whose version differs).
    #[serde(default)]
    pub tools_changes: Vec<String>,
}

impl ReleaseManifest {
    pub fn parse(text: &str) -> Result<ReleaseManifest, NetError> {
        let m: ReleaseManifest = serde_json::from_str(text)
            .map_err(|e| NetError::Invalid(format!("latest.json: {e}")))?;
        parse_version(&m.version)
            .ok_or_else(|| NetError::Invalid(format!("latest.json: bad version '{}'", m.version)))?;
        if m.sha256.len() != 64 || !m.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(NetError::Invalid("latest.json: bad sha256".into()));
        }
        if let Some(min) = &m.min_supported_version
            && parse_version(min).is_none()
        {
            return Err(NetError::Invalid(format!("latest.json: bad min_supported_version '{min}'")));
        }
        Ok(m)
    }

    pub fn notes(&self, lang: crate::i18n::Lang) -> &str {
        match lang {
            crate::i18n::Lang::Th if !self.notes_th.trim().is_empty() => &self.notes_th,
            _ => &self.notes_en,
        }
    }
}

/// Accepts `1.2.3`, `v1.2.3`, `1.2` (→ 1.2.0) and pre-releases like `2.0.0-beta.1`.
pub fn parse_version(s: &str) -> Option<semver::Version> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    semver::Version::parse(s).ok().or_else(|| {
        let parts: Vec<&str> = s.split('.').collect();
        match parts.as_slice() {
            [a, b] => semver::Version::parse(&format!("{a}.{b}.0")).ok(),
            [a] => semver::Version::parse(&format!("{a}.0.0")).ok(),
            _ => None,
        }
    })
}

pub fn compare_versions(a: &str, b: &str) -> Option<Ordering> {
    Some(parse_version(a)?.cmp(&parse_version(b)?))
}

#[derive(Clone, Debug, PartialEq)]
pub enum CheckResult {
    UpToDate { latest: String },
    Available(ReleaseManifest),
    /// Newer version exists but this one is older than `min_supported_version`.
    ManualRequired(ReleaseManifest),
}

pub fn evaluate(current: &str, m: &ReleaseManifest) -> Result<CheckResult, NetError> {
    let cur = parse_version(current)
        .ok_or_else(|| NetError::Invalid(format!("bad current version {current}")))?;
    let latest = parse_version(&m.version).expect("validated in parse");
    if latest <= cur {
        return Ok(CheckResult::UpToDate {
            latest: m.version.clone(),
        });
    }
    if let Some(min) = m.min_supported_version.as_deref().and_then(parse_version)
        && cur < min
    {
        return Ok(CheckResult::ManualRequired(m.clone()));
    }
    Ok(CheckResult::Available(m.clone()))
}

/// Fetch `latest.json` and compare with this build.
pub fn check() -> Result<CheckResult, NetError> {
    let url = super::update_manifest_url();
    tracing::info!("checking for updates: {url}");
    let text = http::get_text(&url)?;
    let m = ReleaseManifest::parse(&text)?;
    evaluate(super::app_version(), &m)
}

/// Download the installer into the temp folder and verify it.
pub fn download_installer(
    m: &ReleaseManifest,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<PathBuf, NetError> {
    let dir = std::env::temp_dir().join("QuickEnhancer-update");
    let dest = dir.join(format!("QuickEnhancer-Setup-{}.exe", m.version));
    http::download(&m.installer_url, &dest, Some(&m.sha256), cancel, on_progress)?;
    Ok(dest)
}

/// Inno Setup switches for an unattended in-place upgrade that shows progress.
pub const SILENT_ARGS: [&str; 4] = ["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS"];

/// Start the installer (detached). The caller exits the app right after.
pub fn launch_installer(installer: &Path) -> std::io::Result<()> {
    std::process::Command::new(installer)
        .args(SILENT_ARGS)
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(version: &str, min: Option<&str>) -> ReleaseManifest {
        ReleaseManifest {
            version: version.into(),
            published: None,
            notes_en: "Fixes".into(),
            notes_th: "แก้ไข".into(),
            installer_url: "https://example.invalid/setup.exe".into(),
            sha256: "a".repeat(64),
            installer_size: None,
            min_supported_version: min.map(Into::into),
            tools_changes: vec![],
        }
    }

    #[test]
    fn version_comparison() {
        assert_eq!(compare_versions("2.0.0", "2.0.1"), Some(Ordering::Less));
        assert_eq!(compare_versions("v2.1", "2.1.0"), Some(Ordering::Equal));
        assert_eq!(compare_versions("2.10.0", "2.9.9"), Some(Ordering::Greater));
        assert_eq!(compare_versions("2.0.0-beta.1", "2.0.0"), Some(Ordering::Less));
        assert_eq!(compare_versions("x", "1.0.0"), None);
    }

    #[test]
    fn evaluate_cases() {
        assert!(matches!(evaluate("2.0.0", &manifest("2.0.0", None)).unwrap(), CheckResult::UpToDate { .. }));
        assert!(matches!(evaluate("2.1.0", &manifest("2.0.5", None)).unwrap(), CheckResult::UpToDate { .. }));
        assert!(matches!(evaluate("2.0.0", &manifest("2.1.0", Some("2.0.0"))).unwrap(), CheckResult::Available(_)));
        assert!(matches!(evaluate("1.9.0", &manifest("2.1.0", Some("2.0.0"))).unwrap(), CheckResult::ManualRequired(_)));
    }

    #[test]
    fn latest_json_parsing() {
        let text = format!(
            r#"{{"version":"2.1.0","notes_en":"New","notes_th":"ใหม่","installer_url":"https://github.com/o/r/releases/download/v2.1.0/QuickEnhancer-Setup-2.1.0.exe","sha256":"{}","min_supported_version":"2.0.0","tools_changes":["rife"]}}"#,
            "b".repeat(64)
        );
        let m = ReleaseManifest::parse(&text).unwrap();
        assert_eq!(m.version, "2.1.0");
        assert_eq!(m.tools_changes, vec!["rife"]);
        assert_eq!(m.notes(crate::i18n::Lang::Th), "ใหม่");
        assert!(ReleaseManifest::parse(r#"{"version":"abc","installer_url":"x","sha256":"x"}"#).is_err());
        let bad_sha = text.replace(&"b".repeat(64), "zz");
        assert!(ReleaseManifest::parse(&bad_sha).is_err());
    }
}
