//! Child-process helpers: hidden console, priority, stderr tail capture, cancellable runs.

use std::collections::VecDeque;
use std::io::Read;
use std::path::Path;
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::config::Priority;
use crate::pipeline::cancel::CancelToken;

#[derive(thiserror::Error, Debug)]
pub enum ProcError {
    #[error("failed to start {program}: {source}")]
    Spawn {
        program: String,
        source: std::io::Error,
    },
    #[error("{program} failed ({code})\n{tail}")]
    Failed {
        program: String,
        code: String,
        tail: String,
    },
    #[error(
        "{program}: the GPU driver reset (Vulkan device lost). This usually means one GPU          step took too long — a smaller tile size (Eco preset or Overrides → Tile size)          avoids it. Updating the GPU driver can also help.
{tail}"
    )]
    GpuLost { program: String, tail: String },
    #[error("cancelled")]
    Cancelled,
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

impl ProcError {
    pub fn tail(&self) -> &str {
        match self {
            ProcError::Failed { tail, .. } | ProcError::GpuLost { tail, .. } => tail,
            _ => "",
        }
    }
}

/// A `Command` that never pops up a console window on Windows.
pub fn new_command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.stdin(Stdio::null());
    cmd
}

pub fn program_name(cmd: &Command) -> String {
    Path::new(cmd.get_program())
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "process".into())
}

/// Apply a scheduling priority to a running child process (Windows `SetPriorityClass`).
pub fn apply_priority(child: &Child, priority: Priority) {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Threading::{
            ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
            SetPriorityClass,
        };
        let class = match priority {
            Priority::BelowNormal => BELOW_NORMAL_PRIORITY_CLASS,
            Priority::Normal => NORMAL_PRIORITY_CLASS,
            Priority::AboveNormal => ABOVE_NORMAL_PRIORITY_CLASS,
        };
        let h = HANDLE(child.as_raw_handle());
        // SAFETY: the handle is owned by `child` and valid for its lifetime.
        if let Err(e) = unsafe { SetPriorityClass(h, class) } {
            tracing::debug!("SetPriorityClass failed: {e}");
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (child, priority);
    }
}

/// Spawn a command with the given priority, mapping spawn errors.
pub fn spawn(cmd: &mut Command, priority: Priority) -> Result<Child, ProcError> {
    let program = program_name(cmd);
    tracing::debug!(
        "spawn {:?} {:?}",
        cmd.get_program(),
        cmd.get_args().collect::<Vec<_>>()
    );
    let child = cmd
        .spawn()
        .map_err(|source| ProcError::Spawn { program, source })?;
    apply_priority(&child, priority);
    Ok(child)
}

/// Collects the last N lines of a child's stderr on a background thread
/// (also prevents the pipe from filling up and blocking the child).
pub struct StderrTail {
    lines: Arc<Mutex<VecDeque<String>>>,
    handle: Option<JoinHandle<()>>,
    gpu_lost: Arc<AtomicBool>,
}

/// stderr lines from ncnn/Vulkan tools meaning the GPU device was lost (driver reset):
/// every later GPU call fails too, so the process should be stopped right away.
pub fn is_gpu_lost_line(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    l.contains("vkqueuesubmit failed")
        || l.contains("vkwaitforfences failed")
        || l.contains("device_lost")
        || l.contains("device lost")
}

impl StderrTail {
    pub fn spawn(mut stderr: ChildStderr, cap: usize) -> StderrTail {
        let lines = Arc::new(Mutex::new(VecDeque::with_capacity(cap)));
        let l2 = lines.clone();
        let gpu_lost = Arc::new(AtomicBool::new(false));
        let lost2 = gpu_lost.clone();
        let handle = std::thread::Builder::new()
            .name("stderr-tail".into())
            .spawn(move || {
                let mut buf = [0u8; 4096];
                let mut cur = Vec::<u8>::new();
                let push = |cur: &mut Vec<u8>| {
                    if cur.is_empty() {
                        return;
                    }
                    let s = String::from_utf8_lossy(cur).trim().to_string();
                    cur.clear();
                    if s.is_empty() {
                        return;
                    }
                    if is_gpu_lost_line(&s) {
                        lost2.store(true, Ordering::Relaxed);
                    }
                    if let Ok(mut g) = l2.lock() {
                        if g.len() >= cap {
                            g.pop_front();
                        }
                        g.push_back(s);
                    }
                };
                loop {
                    match stderr.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            for &b in &buf[..n] {
                                if b == b'\n' || b == b'\r' {
                                    push(&mut cur);
                                } else {
                                    cur.push(b);
                                }
                            }
                        }
                    }
                }
                push(&mut cur);
            })
            .ok();
        StderrTail {
            lines,
            handle,
            gpu_lost,
        }
    }

    /// The child reported a lost Vulkan device.
    pub fn gpu_lost(&self) -> bool {
        self.gpu_lost.load(Ordering::Relaxed)
    }

    pub fn text(&self) -> String {
        self.lines
            .lock()
            .map(|g| g.iter().cloned().collect::<Vec<_>>().join("\n"))
            .unwrap_or_default()
    }

    /// Wait for the reader to hit EOF, then return the tail.
    pub fn finish(mut self) -> String {
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        self.text()
    }
}

/// Kills the child when dropped unless `disarm`ed — keeps error paths from leaking processes.
pub struct ChildGuard(pub Option<Child>);

impl ChildGuard {
    pub fn child(&mut self) -> &mut Child {
        self.0.as_mut().expect("child taken")
    }
    pub fn disarm(mut self) -> Child {
        self.0.take().expect("child taken")
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

pub struct ProcOutput {
    pub stdout: Vec<u8>,
    pub stderr_tail: String,
}

/// Run a command to completion, capturing stdout and the stderr tail.
/// Polls `cancel` and kills the child when it fires.
pub fn run_capture(
    cmd: Command,
    cancel: Option<&CancelToken>,
    priority: Priority,
) -> Result<ProcOutput, ProcError> {
    run_capture_tick(cmd, cancel, priority, &mut || {})
}

/// Like [`run_capture`], calling `tick` roughly every 250 ms while the child runs
/// (used to poll output-file counts for progress).
pub fn run_capture_tick(
    mut cmd: Command,
    cancel: Option<&CancelToken>,
    priority: Priority,
    tick: &mut dyn FnMut(),
) -> Result<ProcOutput, ProcError> {
    let mut last_tick = std::time::Instant::now();
    let program = program_name(&cmd);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut guard = ChildGuard(Some(spawn(&mut cmd, priority)?));
    let child = guard.child();
    let tail = StderrTail::spawn(child.stderr.take().expect("stderr piped"), 40);
    let mut stdout = child.stdout.take().expect("stdout piped");
    let out_handle = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = stdout.read_to_end(&mut v);
        v
    });
    let status = loop {
        if let Some(c) = cancel
            && c.is_cancelled()
        {
            drop(guard);
            let _ = out_handle.join();
            return Err(ProcError::Cancelled);
        }
        if tail.gpu_lost() {
            // Give the tool a moment to print its error, then stop it: it would otherwise
            // keep going and write black / garbage frames.
            std::thread::sleep(Duration::from_millis(300));
            drop(guard);
            let _ = out_handle.join();
            return Err(ProcError::GpuLost {
                program,
                tail: tail.finish(),
            });
        }
        match guard.child().try_wait()? {
            Some(st) => break st,
            None => std::thread::sleep(Duration::from_millis(25)),
        }
        if last_tick.elapsed() >= Duration::from_millis(250) {
            last_tick = std::time::Instant::now();
            tick();
        }
    };
    let _ = guard.disarm().wait();
    let stdout = out_handle.join().unwrap_or_default();
    let stderr_tail = tail.finish();
    if !status.success() {
        return Err(ProcError::Failed {
            program,
            code: status
                .code()
                .map(describe_exit_code)
                .unwrap_or_else(|| "terminated".into()),
            tail: stderr_tail,
        });
    }
    Ok(ProcOutput {
        stdout,
        stderr_tail,
    })
}

pub fn human_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// 1234567 → "1,234,567".
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn human_duration(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "--".into();
    }
    let s = secs.round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formatting() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_duration(65.0), "1:05");
        assert_eq!(human_duration(3725.0), "1:02:05");
    }
}

/// Human-readable exit code; Windows NTSTATUS crash codes are named.
pub fn describe_exit_code(c: i32) -> String {
    let name = match c as u32 {
        0xC000_0005 => Some("crashed: access violation"),
        0xC000_0409 => Some("crashed: stack buffer overrun / fail-fast"),
        0xC000_001D => Some("crashed: illegal instruction"),
        0xC000_00FD => Some("crashed: stack overflow"),
        0xC000_0135 => Some("could not start: a required DLL is missing"),
        0xC000_0142 => Some("could not start: DLL initialization failed"),
        0xC000_0017 | 0xC000_009A => Some("crashed: out of memory"),
        _ => None,
    };
    match name {
        Some(n) => format!("{n}, exit code 0x{:08X}", c as u32),
        None => format!("exit code {c}"),
    }
}

#[cfg(test)]
mod exit_code_tests {
    use super::describe_exit_code;
    #[test]
    fn names_ntstatus_crashes() {
        assert!(describe_exit_code(-1073741819).contains("access violation"));
        assert_eq!(describe_exit_code(1), "exit code 1");
    }
}

#[cfg(test)]
mod gpu_lost_tests {
    use super::{is_gpu_lost_line, thousands};
    #[test]
    fn detects_device_lost() {
        assert!(is_gpu_lost_line("vkQueueSubmit failed -4"));
        assert!(is_gpu_lost_line("vkWaitForFences failed -4"));
        assert!(!is_gpu_lost_line("75.00%"));
        assert!(!is_gpu_lost_line(
            "[0 NVIDIA GeForce RTX 5060 Ti]  queueC=2[8]"
        ));
    }
    #[test]
    fn thousands_sep() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(1234567), "1,234,567");
    }
}
