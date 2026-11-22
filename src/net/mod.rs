//! Everything that touches the network lives here (and only here): downloading the
//! external tools and checking for / downloading app updates. The processing pipeline never
//! uses this module. There is no telemetry.

pub mod http;
pub mod tools;
pub mod update;

/// GitHub repository that publishes releases. **Placeholder — set before the first release**
/// (see docs/RELEASING.md).
pub const GITHUB_REPO: &str = "OWNER/REPO";

/// Release manifest URL (`latest.json` attached to the newest GitHub release). The
/// `QUICK_ENHANCER_UPDATE_URL` env var overrides it (for testing with a local server).
pub fn update_manifest_url() -> String {
    std::env::var("QUICK_ENHANCER_UPDATE_URL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            format!("https://github.com/{GITHUB_REPO}/releases/latest/download/latest.json")
        })
}

pub fn releases_page() -> String {
    format!("https://github.com/{GITHUB_REPO}/releases")
}

pub fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn user_agent() -> String {
    format!("QuickEnhancer/{}", app_version())
}
