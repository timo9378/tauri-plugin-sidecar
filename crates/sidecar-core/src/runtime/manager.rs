//! The manager: registry of supervisors, dependency-ordered startup, and
//! reverse-ordered shutdown.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::oneshot;

use super::supervisor::{Command, Supervisor, SupervisorHandle};
use crate::domain::config::SidecarConfig;
use crate::domain::error::SidecarError;
use crate::domain::ordering::topo_sort;
use crate::domain::state::{EventSink, SidecarState};
use crate::infra::pid_store::PidStore;

pub struct SidecarManager {
    handles: HashMap<String, SupervisorHandle>,
    /// Names in dependency (topological) order.
    start_order: Vec<String>,
}

impl SidecarManager {
    /// Validates configs, cleans up orphans from previous runs, and launches
    /// one supervisor task per sidecar (all start Idle).
    ///
    /// `resolve_binary` maps configured (possibly relative) paths to real
    /// ones — the Tauri layer resolves against the resource directory.
    pub fn launch(
        configs: Vec<SidecarConfig>,
        sink: Arc<dyn EventSink>,
        state_dir: &Path,
        resolve_binary: impl Fn(&PathBuf) -> PathBuf,
    ) -> Result<Self, SidecarError> {
        // Fail with a clear error instead of tokio's spawn panic when called
        // outside a runtime (e.g. directly from Tauri's setup thread).
        if tokio::runtime::Handle::try_current().is_err() {
            return Err(SidecarError::NoAsyncRuntime);
        }
        let start_order = topo_sort(&configs)?;

        let pid_store = Arc::new(PidStore::open(state_dir));
        pid_store.cleanup_stale();

        let mut handles = HashMap::new();
        for config in configs {
            let resolved = resolve_binary(&config.binary);
            let handle = Supervisor::launch(config, resolved, sink.clone(), pid_store.clone());
            handles.insert(handle.config.name.clone(), handle);
        }

        Ok(SidecarManager {
            handles,
            start_order,
        })
    }

    pub fn names(&self) -> Vec<String> {
        self.start_order.clone()
    }

    pub fn state(&self, name: &str) -> Result<SidecarState, SidecarError> {
        self.handles
            .get(name)
            .map(|h| h.state_rx.borrow().clone())
            .ok_or_else(|| SidecarError::UnknownSidecar(name.into()))
    }

    pub fn port(&self, name: &str) -> Result<Option<u16>, SidecarError> {
        self.handles
            .get(name)
            .map(|h| *h.port.lock())
            .ok_or_else(|| SidecarError::UnknownSidecar(name.into()))
    }

    pub fn logs(&self, name: &str, lines: usize) -> Result<Vec<String>, SidecarError> {
        self.handles
            .get(name)
            .map(|h| {
                let buf = h.logs.lock();
                buf.iter()
                    .rev()
                    .take(lines)
                    .map(|l| format!("[{}] {}", l.stream, l.line))
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect()
            })
            .ok_or_else(|| SidecarError::UnknownSidecar(name.into()))
    }

    /// Starts one sidecar, first starting its (transitive) dependencies in
    /// topological order and waiting for each to turn healthy — the
    /// docker-compose semantic. Starting an already-running sidecar is a
    /// no-op.
    pub async fn start(&self, name: &str) -> Result<(), SidecarError> {
        if !self.handles.contains_key(name) {
            return Err(SidecarError::UnknownSidecar(name.into()));
        }

        // Dependency closure of `name`, in start order.
        let mut needed = std::collections::HashSet::new();
        collect_deps(name, &self.handles, &mut needed);
        let chain: Vec<&String> = self
            .start_order
            .iter()
            .filter(|n| needed.contains(n.as_str()) || n.as_str() == name)
            .collect();

        for member in chain {
            let handle = &self.handles[member.as_str()];
            let (tx, rx) = oneshot::channel();
            let _ = handle.cmd_tx.send(Command::Start(tx)).await;
            let _ = rx.await;
            if member != name {
                self.await_healthy(member, name).await?;
            }
        }
        Ok(())
    }

    /// Starts every sidecar in dependency order. Independent chains proceed
    /// in parallel; dependents launch the moment their deps turn healthy.
    pub async fn start_all(&self) {
        let mut waiters = Vec::new();
        for name in &self.start_order {
            let name = name.clone();
            let handle = &self.handles[&name];
            let deps = handle.config.depends_on.clone();
            let cmd_tx = handle.cmd_tx.clone();
            let dep_rxs: Vec<_> = deps
                .iter()
                .filter_map(|d| self.handles.get(d).map(|h| (d.clone(), h.state_rx.clone())))
                .collect();
            let sidecar = name.clone();

            // Each waiter is its own task so independent dependency chains
            // start in parallel instead of queueing behind one another.
            waiters.push(tokio::spawn(async move {
                for (dep, mut rx) in dep_rxs {
                    loop {
                        let state = rx.borrow().clone();
                        match state {
                            SidecarState::Healthy => break,
                            SidecarState::Failed { .. } => {
                                tracing::error!(%sidecar, %dep, "dependency failed; not starting");
                                return;
                            }
                            _ => {
                                if rx.changed().await.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                }
                let (tx, rx) = oneshot::channel();
                let _ = cmd_tx.send(Command::Start(tx)).await;
                let _ = rx.await;
            }));
        }
        for waiter in waiters {
            let _ = waiter.await;
        }
    }

    pub async fn stop(&self, name: &str) -> Result<(), SidecarError> {
        let handle = self
            .handles
            .get(name)
            .ok_or_else(|| SidecarError::UnknownSidecar(name.into()))?;
        let (tx, rx) = oneshot::channel();
        let _ = handle.cmd_tx.send(Command::Stop(tx)).await;
        let _ = rx.await;
        Ok(())
    }

    pub async fn restart(&self, name: &str) -> Result<(), SidecarError> {
        let handle = self
            .handles
            .get(name)
            .ok_or_else(|| SidecarError::UnknownSidecar(name.into()))?;
        let (tx, rx) = oneshot::channel();
        let _ = handle.cmd_tx.send(Command::Restart(tx)).await;
        let _ = rx.await;
        Ok(())
    }

    /// Stops everything in reverse dependency order and ends the supervisor
    /// tasks. Called on app exit.
    pub async fn shutdown_all(&self) {
        for name in self.start_order.iter().rev() {
            if let Some(handle) = self.handles.get(name) {
                let (tx, rx) = oneshot::channel();
                let _ = handle.cmd_tx.send(Command::Shutdown(tx)).await;
                let _ = rx.await;
            }
        }
    }

    async fn await_healthy(&self, dep: &str, dependent: &str) -> Result<(), SidecarError> {
        let handle = self
            .handles
            .get(dep)
            .ok_or_else(|| SidecarError::UnknownDependency {
                name: dependent.into(),
                dep: dep.into(),
            })?;
        let mut rx = handle.state_rx.clone();
        loop {
            let state = rx.borrow().clone();
            match state {
                SidecarState::Healthy => return Ok(()),
                SidecarState::Failed { .. } => {
                    return Err(SidecarError::DependencyFailed {
                        name: dependent.into(),
                        dep: dep.into(),
                    })
                }
                _ => {
                    if rx.changed().await.is_err() {
                        return Err(SidecarError::DependencyFailed {
                            name: dependent.into(),
                            dep: dep.into(),
                        });
                    }
                }
            }
        }
    }
}

/// Collects the transitive dependencies of `name` into `out`.
fn collect_deps(
    name: &str,
    handles: &HashMap<String, SupervisorHandle>,
    out: &mut std::collections::HashSet<String>,
) {
    if let Some(handle) = handles.get(name) {
        for dep in &handle.config.depends_on {
            if out.insert(dep.clone()) {
                collect_deps(dep, handles, out);
            }
        }
    }
}
