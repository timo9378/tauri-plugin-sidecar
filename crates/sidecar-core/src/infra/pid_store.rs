//! Orphan cleanup across app runs.
//!
//! If the app (or the OS) dies without a clean shutdown, sidecars from the
//! previous run survive — holding ports, file locks, and GPU memory. On the
//! next launch the supervisor consults a small state file of previously
//! spawned pids and kills anything that is still alive *and still runs the
//! same executable* (pid recycling is checked by name).

use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::platform::process;

#[derive(Debug, Default, Serialize, Deserialize)]
struct PidFile {
    /// sidecar name → (pid, executable file name)
    entries: HashMap<String, (u32, String)>,
}

/// Persistent record of live sidecar pids for this app.
pub struct PidStore {
    path: PathBuf,
    state: Mutex<PidFile>,
}

impl PidStore {
    /// Opens (or creates) the store at `dir/sidecar-pids.json`.
    pub fn open(dir: &Path) -> Self {
        let path = dir.join("sidecar-pids.json");
        let state = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        PidStore {
            path,
            state: Mutex::new(state),
        }
    }

    /// Kills every recorded process from a previous run that is still alive
    /// and still runs the recorded executable. Returns the names cleaned up.
    pub fn cleanup_stale(&self) -> Vec<String> {
        let mut cleaned = Vec::new();
        let mut state = self.state.lock();
        for (name, (pid, exe)) in state.entries.drain() {
            if process::kill_stale(pid, &exe) {
                tracing::warn!(sidecar = %name, pid, "killed orphan from previous run");
                cleaned.push(name);
            }
        }
        drop(state);
        self.flush();
        cleaned
    }

    pub fn record(&self, name: &str, pid: u32, binary: &Path) {
        let exe = binary
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.state
            .lock()
            .entries
            .insert(name.to_string(), (pid, exe));
        self.flush();
    }

    pub fn clear(&self, name: &str) {
        self.state.lock().entries.remove(name);
        self.flush();
    }

    fn flush(&self) {
        let state = self.state.lock();
        if let Ok(json) = serde_json::to_string_pretty(&*state) {
            if let Some(parent) = self.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&self.path, json);
        }
    }
}
