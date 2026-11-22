//! Cooperative cancellation + pause flag shared between UI and workers.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> CancelToken {
        CancelToken::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Clone, Debug, Default)]
pub struct PauseFlag(Arc<AtomicBool>);

impl PauseFlag {
    pub fn set(&self, paused: bool) {
        self.0.store(paused, Ordering::SeqCst);
    }
    pub fn is_paused(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    /// Block while paused; returns early when cancelled.
    pub fn wait_while_paused(&self, cancel: &CancelToken) {
        while self.is_paused() && !cancel.is_cancelled() {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}
