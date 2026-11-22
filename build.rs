//! Embeds the Windows icon and version information into QuickEnhancer.exe.
//! The version comes from Cargo.toml (`CARGO_PKG_VERSION`) — the single source of truth.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico")
        .set("ProductName", "Quick Enhancer")
        .set("FileDescription", "Quick Enhancer — local image & video enhancement")
        // Placeholder: replace with your name / organisation (see docs/RELEASING.md).
        .set("CompanyName", "Quick Enhancer Publisher")
        .set("LegalCopyright", "Copyright (c) Quick Enhancer contributors")
        .set("OriginalFilename", "QuickEnhancer.exe")
        .set("InternalName", "QuickEnhancer")
        .set("ProductVersion", &version)
        .set("FileVersion", &version);
    if let Err(e) = res.compile() {
        // Don't fail the build over resources (e.g. missing windres); the exe just has no icon.
        println!("cargo:warning=could not embed Windows resources: {e}");
    }
}
