//! tracing setup: daily-rolling file in `logs/`, stderr, and an in-memory ring for the UI log panel.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

const UI_LOG_CAP: usize = 500;

#[derive(Clone, Debug)]
pub struct LogLine {
    pub level: Level,
    pub time: String,
    pub text: String,
}

/// Shared ring buffer of recent log lines (INFO and above).
#[derive(Clone, Default)]
pub struct UiLog(pub Arc<Mutex<VecDeque<LogLine>>>);

impl UiLog {
    pub fn snapshot(&self) -> Vec<LogLine> {
        self.0
            .lock()
            .map(|g| g.iter().cloned().collect())
            .unwrap_or_default()
    }
    pub fn clear(&self) {
        if let Ok(mut g) = self.0.lock() {
            g.clear();
        }
    }
}

struct UiLayer(UiLog);

#[derive(Default)]
struct MsgVisitor(String);

impl Visit for MsgVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.0, "{value:?}");
        } else {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0.push_str(value);
        } else {
            let _ = write!(self.0, " {}={value}", field.name());
        }
    }
}

impl<S: Subscriber> Layer<S> for UiLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let level = *event.metadata().level();
        if level > Level::INFO {
            return;
        }
        // Only our own crate's messages go to the UI.
        if !event.metadata().target().starts_with("quick_enhancer") {
            return;
        }
        let mut v = MsgVisitor::default();
        event.record(&mut v);
        let time = now_hms();
        if let Ok(mut g) = (self.0).0.lock() {
            if g.len() >= UI_LOG_CAP {
                g.pop_front();
            }
            g.push_back(LogLine {
                level,
                time,
                text: v.0,
            });
        }
    }
}

fn now_hms() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // UTC wall clock; good enough for a log panel without a tz dependency.
    let s = secs % 86_400;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// Initialise logging. Keep the returned guard alive for the program duration.
pub fn init(logs_dir: &Path) -> (Option<tracing_appender::non_blocking::WorkerGuard>, UiLog) {
    let ui = UiLog::default();
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"));

    let _ = std::fs::create_dir_all(logs_dir);
    let file = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("quick-enhancer")
        .filename_suffix("log")
        .max_log_files(14)
        .build(logs_dir)
        .ok();

    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(UiLayer(ui.clone()))
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr));

    let guard = if let Some(file) = file {
        let (nb, guard) = tracing_appender::non_blocking(file);
        let _ = registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(nb),
            )
            .try_init();
        Some(guard)
    } else {
        let _ = registry.try_init();
        None
    };
    (guard, ui)
}
