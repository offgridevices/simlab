//! How a long solve reports progress and learns it should stop.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub trait Progress: Send + Sync {
    /// `fraction` in 0..=1 for the whole run; `message` in plain words
    /// ("Solving: 412 of 2000 steps").
    fn report(&self, fraction: f32, message: &str);
    /// A named number worth plotting live (e.g. "drag_n", "residual").
    fn metric(&self, _name: &str, _step: u64, _value: f64) {}
    fn cancelled(&self) -> bool;
}

/// Progress that goes nowhere; for tests.
pub struct Silent;
impl Progress for Silent {
    fn report(&self, _f: f32, _m: &str) {}
    fn cancelled(&self) -> bool {
        false
    }
}

/// Prints to stderr; for command-line runs and validation.
pub struct Stderr {
    pub cancel: Arc<AtomicBool>,
}
impl Default for Stderr {
    fn default() -> Self {
        Self { cancel: Arc::new(AtomicBool::new(false)) }
    }
}
impl Progress for Stderr {
    fn report(&self, f: f32, m: &str) {
        eprintln!("[{:5.1}%] {m}", f * 100.0);
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Returned (inside anyhow) when a run was stopped by the user.
#[derive(Debug)]
pub struct Cancelled;
impl std::error::Error for Cancelled {}
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("stopped")
    }
}
