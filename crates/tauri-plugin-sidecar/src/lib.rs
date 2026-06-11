//! # tauri-plugin-sidecar
//!
//! Production-grade sidecar lifecycle management for Tauri v2 — the
//! docker-compose of sidecars. Declare each sidecar once; the plugin owns
//! spawning, supervision, crash restarts with backoff, dynamic port
//! injection, session-token auth, health gating, dependency ordering,
//! kill-tree shutdown, and orphan cleanup across app runs.
//!
//! ```rust,ignore
//! use tauri_plugin_sidecar::{Builder, SidecarConfig, HealthCheck};
//!
//! tauri::Builder::default()
//!     .plugin(
//!         Builder::new()
//!             .sidecar(
//!                 SidecarConfig::new("backend", "binaries/backend/server")
//!                     .dynamic_port("BACKEND_PORT")
//!                     .auth_token("BACKEND_TOKEN")
//!                     .health(HealthCheck::Http { path: "/healthz".into(), timeout_secs: 30 }),
//!             )
//!             .sidecar(
//!                 SidecarConfig::new("asr", "binaries/asr/asr-server")
//!                     .depends_on("backend")
//!                     .health(HealthCheck::StdoutMarker { pattern: "READY".into(), timeout_secs: 60 }),
//!             )
//!             .autostart(true)
//!             .build(),
//!     );
//! ```

mod commands;
pub use sidecar_core as core;

use std::sync::Arc;

use tauri::plugin::{Builder as PluginBuilder, TauriPlugin};
use tauri::{Emitter, Manager, RunEvent, Runtime};

pub use sidecar_core::{
    AuthStrategy, EventSink, GracefulShutdown, HealthCheck, LogLine, PortStrategy, RestartPolicy,
    ShutdownPolicy, SidecarConfig, SidecarError, SidecarManager, SidecarState,
};

/// Event name for state changes: payload `{ name, state }`.
pub const EVENT_STATE: &str = "sidecar://state";
/// Event name for log lines (only for sidecars with `emit_logs(true)`).
pub const EVENT_LOG: &str = "sidecar://log";

/// Plugin configuration builder.
#[derive(Default)]
pub struct Builder {
    configs: Vec<SidecarConfig>,
    autostart: bool,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a sidecar.
    #[must_use]
    pub fn sidecar(mut self, config: SidecarConfig) -> Self {
        self.configs.push(config);
        self
    }

    /// Starts every sidecar (in dependency order) as soon as the app runs.
    #[must_use]
    pub fn autostart(mut self, autostart: bool) -> Self {
        self.autostart = autostart;
        self
    }

    pub fn build<R: Runtime>(self) -> TauriPlugin<R> {
        let configs = self.configs;
        let autostart = self.autostart;

        PluginBuilder::new("sidecar")
            .invoke_handler(tauri::generate_handler![
                commands::status,
                commands::start,
                commands::stop,
                commands::restart,
                commands::logs,
            ])
            .setup(move |app, _api| {
                let sink = Arc::new(TauriSink { app: app.clone() });

                let state_dir = app
                    .path()
                    .app_data_dir()
                    .map_err(|e| format!("cannot resolve app data dir: {e}"))?;

                let resource_dir = app.path().resource_dir().ok();
                let resolve = move |p: &std::path::PathBuf| -> std::path::PathBuf {
                    if p.is_absolute() {
                        return p.clone();
                    }
                    match &resource_dir {
                        Some(dir) => dir.join(p),
                        None => p.clone(),
                    }
                };

                let manager = SidecarManager::launch(configs, sink, &state_dir, resolve)
                    .map_err(|e| e.to_string())?;
                let manager = Arc::new(manager);
                app.manage(PluginState {
                    manager: manager.clone(),
                });

                if autostart {
                    tauri::async_runtime::spawn(async move {
                        manager.start_all().await;
                    });
                }
                Ok(())
            })
            .on_event(|app, event| {
                if let RunEvent::Exit = event {
                    // Take the whole fleet down with the app — no orphans.
                    if let Some(state) = app.try_state::<PluginState>() {
                        let manager = state.manager.clone();
                        tauri::async_runtime::block_on(async move {
                            manager.shutdown_all().await;
                        });
                    }
                }
            })
            .build()
    }
}

pub(crate) struct PluginState {
    pub manager: Arc<SidecarManager>,
}

struct TauriSink<R: Runtime> {
    app: tauri::AppHandle<R>,
}

impl<R: Runtime> EventSink for TauriSink<R> {
    fn state_changed(&self, sidecar: &str, state: &SidecarState) {
        let _ = self.app.emit(
            EVENT_STATE,
            serde_json::json!({ "name": sidecar, "state": state }),
        );
    }

    fn log_line(&self, line: &LogLine) {
        let _ = self.app.emit(EVENT_LOG, line);
    }
}
