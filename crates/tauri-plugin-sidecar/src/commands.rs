//! Tauri commands exposed to the webview.

use serde::Serialize;
use tauri::{Runtime, State};

use crate::PluginState;
use sidecar_core::SidecarState;

#[derive(Serialize)]
pub(crate) struct SidecarStatus {
    pub name: String,
    pub state: SidecarState,
    pub port: Option<u16>,
}

/// Returns the state of one sidecar, or all of them when `name` is omitted.
#[tauri::command]
pub(crate) async fn status<R: Runtime>(
    _app: tauri::AppHandle<R>,
    state: State<'_, PluginState>,
    name: Option<String>,
) -> Result<Vec<SidecarStatus>, String> {
    let manager = &state.manager;
    let names = match name {
        Some(n) => vec![n],
        None => manager.names(),
    };
    names
        .into_iter()
        .map(|n| {
            Ok(SidecarStatus {
                state: manager.state(&n).map_err(|e| e.to_string())?,
                port: manager.port(&n).map_err(|e| e.to_string())?,
                name: n,
            })
        })
        .collect()
}

#[tauri::command]
pub(crate) async fn start<R: Runtime>(
    _app: tauri::AppHandle<R>,
    state: State<'_, PluginState>,
    name: String,
) -> Result<(), String> {
    state.manager.start(&name).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn stop<R: Runtime>(
    _app: tauri::AppHandle<R>,
    state: State<'_, PluginState>,
    name: String,
) -> Result<(), String> {
    state.manager.stop(&name).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn restart<R: Runtime>(
    _app: tauri::AppHandle<R>,
    state: State<'_, PluginState>,
    name: String,
) -> Result<(), String> {
    state
        .manager
        .restart(&name)
        .await
        .map_err(|e| e.to_string())
}

/// Tails the in-memory log ring buffer of a sidecar.
#[tauri::command]
pub(crate) async fn logs<R: Runtime>(
    _app: tauri::AppHandle<R>,
    state: State<'_, PluginState>,
    name: String,
    lines: Option<usize>,
) -> Result<Vec<String>, String> {
    state
        .manager
        .logs(&name, lines.unwrap_or(100))
        .map_err(|e| e.to_string())
}
