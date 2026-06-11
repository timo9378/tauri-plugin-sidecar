//! Event surface of the supervisor, decoupled from Tauri so the core can be
//! tested (and reused) without an app handle.

use serde::{Deserialize, Serialize};

/// Lifecycle state of a sidecar, as reported to the app and frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SidecarState {
    /// Registered but not started.
    Idle,
    /// Waiting for `depends_on` sidecars to become healthy.
    WaitingForDeps,
    /// Process spawned, health check in progress.
    Starting,
    /// Health check passed; dependents may start.
    Healthy,
    /// Crashed; waiting out the backoff before respawn.
    Backoff { attempt: u32, delay_secs: u64 },
    /// Stop requested; graceful step / kill-tree in progress.
    Stopping,
    /// Exited on request (no restart will occur).
    Stopped,
    /// Crashed and the restart policy is exhausted (or `Never`).
    Failed { reason: String },
}

/// A captured output line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogLine {
    pub sidecar: String,
    /// "stdout" | "stderr"
    pub stream: &'static str,
    pub line: String,
}

/// Sink for supervisor events. The Tauri layer forwards these to the
/// webview; tests collect them in memory.
pub trait EventSink: Send + Sync + 'static {
    fn state_changed(&self, sidecar: &str, state: &SidecarState);
    fn log_line(&self, line: &LogLine);
}

/// An [`EventSink`] that drops everything (useful as a default in tests).
pub struct NullSink;

impl EventSink for NullSink {
    fn state_changed(&self, _sidecar: &str, _state: &SidecarState) {}
    fn log_line(&self, _line: &LogLine) {}
}
