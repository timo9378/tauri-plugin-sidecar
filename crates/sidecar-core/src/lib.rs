//! Sidecar process supervision — the Tauri-agnostic core of
//! `tauri-plugin-sidecar`.
//!
//! Layering (one-way dependencies, deepest first):
//!
//! ```text
//! runtime  →  domain + platform + infra      (async orchestration)
//! platform →  OS process / port primitives   (no business logic)
//! infra    →  network probes + persistence   (no business logic)
//! domain   →  pure types & algorithms        (no IO, fully unit-testable)
//! ```

pub mod domain;
pub mod infra;
pub mod platform;
pub mod runtime;

// Flat re-exports so consumers don't memorize the layer map.
pub use domain::config::{
    AuthStrategy, GracefulShutdown, HealthCheck, PortStrategy, RestartPolicy, ShutdownPolicy,
    SidecarConfig,
};
pub use domain::error::SidecarError;
pub use domain::state::{EventSink, LogLine, NullSink, SidecarState};
pub use runtime::manager::SidecarManager;
