//! Child worker processes for memory-heavy AI (the generative "Make sharp" models).
//!
//! The app starts `QuickEnhancer.exe --worker <task.json>`; the child runs one task and exits, so
//! everything it allocated (GPU memory, the ONNX Runtime arena, CUDA / DirectML state) is returned
//! to Windows. The user still sees one program: the child has no window.
//!
//! Protocol: the child writes one message per line on stdout:
//! `progress <0..1>`, `log <text>`, `done`, or `error <text>` / `oom <text>` (out of memory, so
//! the parent can retry with smaller tiles). A child that exits without `done` crashed; its exit
//! code and the end of its stderr make the error message. Cancelling kills the child.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use crate::pipeline::cancel::CancelToken;
use crate::util::{ChildGuard, StderrTail, new_command};

/// Command-line switch that turns the exe into a worker.
pub const ARG: &str = "--worker";

/// How a self-test task ends (for the protocol tests).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SelfTestEnd {
    Done,
    Error,
    Oom,
    /// Exit with code 3 without a result (like a crash).
    Crash,
    /// Panic (Rust's panic exit code 101).
    Panic,
}

/// One unit of work for a child process.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Task {
    /// Protocol test: `steps` progress messages `ms` apart, then `end`.
    SelfTest { steps: u32, ms: u64, end: SelfTestEnd },
    /// One phase of the SeedVR2 generative restoration.
    Seedvr2(crate::sharp::seedvr2::PhaseTask),
}

/// A parsed stdout line.
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    Progress(f32),
    Log(String),
    Done,
    Error(String),
    Oom(String),
}

pub fn parse_line(line: &str) -> Option<Msg> {
    let line = line.trim_end_matches(['\r', '\n']);
    let (head, rest) = line.split_once(' ').unwrap_or((line, ""));
    match head {
        "progress" => rest.trim().parse::<f32>().ok().map(|v| Msg::Progress(v.clamp(0.0, 1.0))),
        "log" => Some(Msg::Log(rest.to_string())),
        "done" => Some(Msg::Done),
        "error" => Some(Msg::Error(rest.to_string())),
        "oom" => Some(Msg::Oom(rest.to_string())),
        _ => None,
    }
}

/// Child-side reporter (stdout, flushed per line so the parent sees progress at once).
pub struct Reporter;

impl Reporter {
    fn line(&self, s: &str) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{}", s.replace(['\r', '\n'], " "));
        let _ = out.flush();
    }
    pub fn progress(&self, v: f32) {
        self.line(&format!("progress {v:.4}"));
    }
    pub fn log(&self, s: &str) {
        self.line(&format!("log {s}"));
    }
}

/// Error from a worker task, as seen by the parent.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkerError {
    Cancelled,
    /// Out of (GPU) memory: retry with smaller tiles.
    Oom(String),
    /// The task reported an error.
    Failed(String),
    /// The child exited without a result.
    Crashed { code: Option<i32>, tail: String },
    /// The child could not be started.
    Spawn(String),
}

impl std::fmt::Display for WorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerError::Cancelled => write!(f, "{}", crate::t!("worker.cancelled")),
            WorkerError::Oom(m) => write!(f, "{}: {m}", crate::t!("worker.oom")),
            WorkerError::Failed(m) => write!(f, "{m}"),
            WorkerError::Crashed { code, tail } => {
                let code = code.map(|c| c.to_string()).unwrap_or_else(|| "?".into());
                let tail = tail.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
                write!(f, "{}", crate::t!("worker.crashed", code = code, detail = tail))
            }
            WorkerError::Spawn(m) => write!(f, "{}: {m}", crate::t!("worker.spawn_failed")),
        }
    }
}

/// Overrides the worker exe (tests run inside a test harness, not `QuickEnhancer.exe`).
pub const EXE_ENV: &str = "QUICK_ENHANCER_WORKER_EXE";

/// The exe to start as a worker (this program).
pub fn worker_exe() -> Result<PathBuf, WorkerError> {
    if let Some(p) = std::env::var_os(EXE_ENV) {
        return Ok(PathBuf::from(p));
    }
    std::env::current_exe().map_err(|e| WorkerError::Spawn(e.to_string()))
}

/// Run `task` in a child process of `exe`, writing the task file into `dir`.
pub fn run(
    exe: &Path,
    task: &Task,
    dir: &Path,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(f32),
) -> Result<(), WorkerError> {
    std::fs::create_dir_all(dir).map_err(|e| WorkerError::Spawn(e.to_string()))?;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let file = dir.join(format!("task-{}-{n}.json", std::process::id()));
    std::fs::write(&file, serde_json::to_vec(task).expect("task serialises"))
        .map_err(|e| WorkerError::Spawn(e.to_string()))?;
    let mut cmd = new_command(exe);
    cmd.arg(ARG).arg(&file).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut guard = ChildGuard(Some(cmd.spawn().map_err(|e| WorkerError::Spawn(e.to_string()))?));
    let child = guard.child();
    let tail = StderrTail::spawn(child.stderr.take().expect("stderr piped"), 30);
    let stdout = child.stdout.take().expect("stdout piped");
    let (tx, rx) = crossbeam_channel::unbounded::<Msg>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(m) = parse_line(&line)
                && tx.send(m).is_err()
            {
                break;
            }
        }
    });
    let mut result: Option<Result<(), WorkerError>> = None;
    loop {
        if cancel.is_cancelled() {
            let _ = std::fs::remove_file(&file);
            return Err(WorkerError::Cancelled); // the guard kills the child
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Msg::Progress(v)) => on_progress(v),
            Ok(Msg::Log(s)) => tracing::info!("worker: {s}"),
            Ok(Msg::Done) => result = Some(Ok(())),
            Ok(Msg::Error(m)) => result = Some(Err(WorkerError::Failed(m))),
            Ok(Msg::Oom(m)) => result = Some(Err(WorkerError::Oom(m))),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                // stdout closed: the child is exiting.
                let status = guard.disarm().wait().ok();
                let _ = std::fs::remove_file(&file);
                return match result {
                    Some(r) => r,
                    None => Err(WorkerError::Crashed {
                        code: status.and_then(|s| s.code()),
                        tail: tail.text(),
                    }),
                };
            }
        }
    }
}

/// Entry point of `QuickEnhancer.exe --worker <task.json>`: returns the process exit code.
pub fn child_main(task_file: &Path) -> i32 {
    let rep = Reporter;
    let task: Task = match std::fs::read(task_file)
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
    {
        Ok(t) => t,
        Err(e) => {
            rep.line(&format!("error invalid task file: {e}"));
            return 2;
        }
    };
    let res: Result<(), TaskError> = match task {
        Task::SelfTest { steps, ms, end } => self_test(&rep, steps, ms, end),
        Task::Seedvr2(t) => crate::sharp::seedvr2::run_phase(&t, &rep),
    };
    match res {
        Ok(()) => {
            rep.line("done");
            0
        }
        Err(TaskError::Oom(m)) => {
            rep.line(&format!("oom {m}"));
            1
        }
        Err(TaskError::Failed(m)) => {
            rep.line(&format!("error {m}"));
            1
        }
    }
}

/// Error of a task inside the child.
#[derive(Debug)]
pub enum TaskError {
    Oom(String),
    Failed(String),
}

impl TaskError {
    /// Classify an engine error message.
    pub fn from_message(m: String) -> TaskError {
        if crate::engines::onnx::runtime::is_oom(&m) {
            TaskError::Oom(m)
        } else {
            TaskError::Failed(m)
        }
    }
}

fn self_test(rep: &Reporter, steps: u32, ms: u64, end: SelfTestEnd) -> Result<(), TaskError> {
    for i in 0..steps {
        std::thread::sleep(Duration::from_millis(ms));
        rep.progress((i + 1) as f32 / steps.max(1) as f32);
    }
    match end {
        SelfTestEnd::Done => Ok(()),
        SelfTestEnd::Error => Err(TaskError::Failed("self-test error".into())),
        SelfTestEnd::Oom => Err(TaskError::Oom("self-test out of memory".into())),
        SelfTestEnd::Crash => {
            eprintln!("self-test crash");
            std::process::exit(3)
        }
        SelfTestEnd::Panic => panic!("self-test panic"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_protocol_lines() {
        assert_eq!(parse_line("progress 0.5"), Some(Msg::Progress(0.5)));
        assert_eq!(parse_line("progress 7\r\n"), Some(Msg::Progress(1.0)));
        assert_eq!(parse_line("done"), Some(Msg::Done));
        assert_eq!(parse_line("error bad thing"), Some(Msg::Error("bad thing".into())));
        assert_eq!(parse_line("oom tile 512"), Some(Msg::Oom("tile 512".into())));
        assert_eq!(parse_line("log hi there"), Some(Msg::Log("hi there".into())));
        assert_eq!(parse_line("random output"), None);
        assert_eq!(parse_line("progress x"), None);
    }

    #[test]
    fn task_json_round_trip() {
        let t = Task::SelfTest { steps: 3, ms: 1, end: SelfTestEnd::Oom };
        let j = serde_json::to_string(&t).unwrap();
        assert!(j.contains("\"kind\":\"self_test\""), "{j}");
        let back: Task = serde_json::from_str(&j).unwrap();
        assert!(matches!(back, Task::SelfTest { steps: 3, end: SelfTestEnd::Oom, .. }));
    }
}
