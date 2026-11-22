//! Minimal HTTPS client helpers (ureq + rustls): fetch small text documents and stream
//! large downloads to disk with progress, cancellation and SHA-256 verification.
//! `file://` URLs and plain local paths are also accepted (offline installs and tests).

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::pipeline::cancel::CancelToken;

#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("network error: {0}")]
    Http(String),
    #[error("server returned HTTP {0}")]
    Status(u16),
    #[error("file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("checksum mismatch: expected {expected}, got {actual}")]
    Checksum { expected: String, actual: String },
    #[error("cancelled")]
    Cancelled,
    #[error("invalid data: {0}")]
    Invalid(String),
}

impl From<ureq::Error> for NetError {
    fn from(e: ureq::Error) -> Self {
        match e {
            ureq::Error::StatusCode(c) => NetError::Status(c),
            other => NetError::Http(other.to_string()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Progress {
    pub done: u64,
    pub total: Option<u64>,
    /// Bytes per second (smoothed).
    pub speed: f64,
}

impl Progress {
    pub fn fraction(&self) -> Option<f32> {
        self.total
            .filter(|t| *t > 0)
            .map(|t| (self.done as f64 / t as f64).clamp(0.0, 1.0) as f32)
    }
}

fn agent() -> ureq::Agent {
    let cfg = ureq::Agent::config_builder()
        .user_agent(super::user_agent())
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .build();
    ureq::Agent::new_with_config(cfg)
}

/// A local path for `file://…` URLs or plain paths; `None` for http(s).
pub fn local_path(url: &str) -> Option<PathBuf> {
    if let Some(rest) = url.strip_prefix("file:///") {
        // file:///C:/x  →  C:/x ;  file:///home/x → /home/x
        let p = if cfg!(windows) { rest.to_string() } else { format!("/{rest}") };
        return Some(PathBuf::from(p.replace("%20", " ")));
    }
    if let Some(rest) = url.strip_prefix("file://") {
        return Some(PathBuf::from(rest.replace("%20", " ")));
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        return None;
    }
    Some(PathBuf::from(url))
}

/// GET a small text document (manifests). Limited to 4 MB.
pub fn get_text(url: &str) -> Result<String, NetError> {
    if let Some(p) = local_path(url) {
        return Ok(std::fs::read_to_string(p)?);
    }
    let mut resp = agent().get(url).call()?;
    let s = resp
        .body_mut()
        .with_config()
        .limit(4 << 20)
        .read_to_string()?;
    Ok(s)
}

/// Lowercase hex SHA-256 of a file.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compare a file's SHA-256 with the expected value (case-insensitive hex).
pub fn verify_sha256(path: &Path, expected: &str) -> Result<(), NetError> {
    let actual = sha256_file(path)?;
    if actual.eq_ignore_ascii_case(expected.trim()) {
        Ok(())
    } else {
        Err(NetError::Checksum {
            expected: expected.trim().to_lowercase(),
            actual,
        })
    }
}

/// Download `url` to `dest`. Data goes to `dest.part` first and is renamed into place only
/// after the transfer completed and (if given) the SHA-256 matched, so an interrupted or
/// corrupted download never leaves a file at `dest`.
pub fn download(
    url: &str,
    dest: &Path,
    expected_sha256: Option<&str>,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<(), NetError> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = dest.with_extension(format!(
        "{}.part",
        dest.extension().and_then(|e| e.to_str()).unwrap_or("bin")
    ));
    let result = (|| -> Result<String, NetError> {
        let (mut reader, total): (Box<dyn Read>, Option<u64>) = match local_path(url) {
            Some(p) => {
                let f = std::fs::File::open(&p)?;
                let len = f.metadata().ok().map(|m| m.len());
                (Box::new(f), len)
            }
            None => {
                let resp = agent().get(url).call()?;
                let total = resp.body().content_length();
                (Box::new(resp.into_body().into_reader()), total)
            }
        };
        let mut out = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(&part)?);
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 256 * 1024];
        let mut p = Progress {
            total,
            ..Default::default()
        };
        let mut window_start = Instant::now();
        let mut window_bytes = 0u64;
        let mut last_report = Instant::now();
        loop {
            if cancel.is_cancelled() {
                return Err(NetError::Cancelled);
            }
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
            hasher.update(&buf[..n]);
            p.done += n as u64;
            window_bytes += n as u64;
            let w = window_start.elapsed().as_secs_f64();
            if w >= 0.5 {
                let inst = window_bytes as f64 / w;
                p.speed = if p.speed == 0.0 { inst } else { p.speed * 0.6 + inst * 0.4 };
                window_start = Instant::now();
                window_bytes = 0;
            }
            if last_report.elapsed() >= Duration::from_millis(100) {
                on_progress(p);
                last_report = Instant::now();
            }
        }
        out.flush()?;
        drop(out);
        on_progress(p);
        if let Some(t) = total
            && p.done != t
        {
            return Err(NetError::Http(format!(
                "incomplete download: {} of {t} bytes",
                p.done
            )));
        }
        Ok(hex(&hasher.finalize()))
    })();
    match result {
        Ok(actual) => {
            if let Some(exp) = expected_sha256
                && !actual.eq_ignore_ascii_case(exp.trim())
            {
                let _ = std::fs::remove_file(&part);
                return Err(NetError::Checksum {
                    expected: exp.trim().to_lowercase(),
                    actual,
                });
            }
            let _ = std::fs::remove_file(dest);
            std::fs::rename(&part, dest)?;
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("qe-net-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    // SHA-256("hello world")
    const HELLO: &str = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";

    #[test]
    fn sha256_good_and_bad() {
        let d = tmpdir("sha");
        let f = d.join("x.txt");
        std::fs::write(&f, b"hello world").unwrap();
        assert_eq!(sha256_file(&f).unwrap(), HELLO);
        assert!(verify_sha256(&f, &HELLO.to_uppercase()).is_ok());
        let bad = "0".repeat(64);
        assert!(matches!(
            verify_sha256(&f, &bad),
            Err(NetError::Checksum { .. })
        ));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn download_verifies_and_never_leaves_partial_files() {
        let d = tmpdir("dl");
        let src = d.join("src.bin");
        std::fs::write(&src, b"hello world").unwrap();
        let url = src.to_string_lossy().to_string();
        let dest = d.join("out").join("file.zip");
        let mut calls = 0;
        download(&url, &dest, Some(HELLO), &CancelToken::new(), &mut |_| calls += 1).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"hello world");
        assert!(calls > 0);
        // Wrong hash: no file at dest, no .part left behind.
        let dest2 = d.join("out").join("bad.zip");
        let r = download(&url, &dest2, Some(&"1".repeat(64)), &CancelToken::new(), &mut |_| {});
        assert!(matches!(r, Err(NetError::Checksum { .. })));
        assert!(!dest2.exists());
        let leftovers: Vec<_> = std::fs::read_dir(d.join("out"))
            .unwrap()
            .flatten()
            .filter(|e| e.path().to_string_lossy().ends_with(".part"))
            .collect();
        assert!(leftovers.is_empty());
        // Cancelled before start.
        let c = CancelToken::new();
        c.cancel();
        assert!(matches!(
            download(&url, &d.join("c.zip"), None, &c, &mut |_| {}),
            Err(NetError::Cancelled)
        ));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn file_urls() {
        if cfg!(windows) {
            assert_eq!(
                local_path("file:///C:/a%20b/x.json"),
                Some(PathBuf::from("C:/a b/x.json"))
            );
        }
        assert_eq!(local_path("https://example.com/x"), None);
    }
}
