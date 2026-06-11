//! The per-sidecar supervisor: one task owning the full lifecycle
//! state machine.
//!
//! ```text
//! Idle ──start──► Starting ──health──► Healthy ──crash──► Backoff ──► Starting…
//!                    │                    │                  │
//!                    │ (health timeout)   │ stop             │ (schedule exhausted)
//!                    ▼                    ▼                  ▼
//!                 Backoff             Stopping ──► Stopped  Failed
//! ```
//!
//! A requested stop never restarts; a crash restarts per policy; the backoff
//! schedule resets after a sustained healthy period.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{sleep, timeout};

use crate::domain::config::{
    AuthStrategy, GracefulShutdown, HealthCheck, PortStrategy, RestartPolicy, SidecarConfig,
};
use crate::domain::error::SidecarError;
use crate::domain::state::{EventSink, LogLine, SidecarState};
use crate::infra::health;
use crate::infra::pid_store::PidStore;
use crate::platform::{ports, process};

const LOG_RING_CAPACITY: usize = 500;

/// Control messages accepted by a running supervisor task.
pub enum Command {
    Start(oneshot::Sender<Result<(), String>>),
    Stop(oneshot::Sender<()>),
    Restart(oneshot::Sender<Result<(), String>>),
    /// Stop and end the supervisor task (app shutdown).
    Shutdown(oneshot::Sender<()>),
}

/// Handle owned by the manager.
pub struct SupervisorHandle {
    pub config: SidecarConfig,
    pub cmd_tx: mpsc::Sender<Command>,
    pub state_rx: watch::Receiver<SidecarState>,
    pub logs: Arc<Mutex<VecDeque<LogLine>>>,
    pub port: Arc<Mutex<Option<u16>>>,
}

pub struct Supervisor {
    config: SidecarConfig,
    resolved_binary: PathBuf,
    sink: Arc<dyn EventSink>,
    pid_store: Arc<PidStore>,
    state_tx: watch::Sender<SidecarState>,
    logs: Arc<Mutex<VecDeque<LogLine>>>,
    port: Arc<Mutex<Option<u16>>>,
    token: Option<String>,
    /// Set when stdout matches the health marker (StdoutMarker checks).
    marker_rx: watch::Receiver<bool>,
    marker_tx: watch::Sender<bool>,
}

impl Supervisor {
    /// Creates the supervisor and spawns its task. Returns the manager handle.
    pub fn launch(
        config: SidecarConfig,
        resolved_binary: PathBuf,
        sink: Arc<dyn EventSink>,
        pid_store: Arc<PidStore>,
    ) -> SupervisorHandle {
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (state_tx, state_rx) = watch::channel(SidecarState::Idle);
        let (marker_tx, marker_rx) = watch::channel(false);
        let logs = Arc::new(Mutex::new(VecDeque::with_capacity(LOG_RING_CAPACITY)));
        let port = Arc::new(Mutex::new(None));

        let token = match &config.auth {
            AuthStrategy::None => None,
            AuthStrategy::Token { .. } => Some(generate_token()),
        };

        let supervisor = Supervisor {
            config: config.clone(),
            resolved_binary,
            sink,
            pid_store,
            state_tx,
            logs: logs.clone(),
            port: port.clone(),
            token,
            marker_rx,
            marker_tx,
        };

        tokio::spawn(supervisor.run(cmd_rx));

        SupervisorHandle {
            config,
            cmd_tx,
            state_rx,
            logs,
            port,
        }
    }

    fn set_state(&self, state: SidecarState) {
        tracing::info!(sidecar = %self.config.name, ?state, "state change");
        self.sink.state_changed(&self.config.name, &state);
        let _ = self.state_tx.send(state);
    }

    async fn run(self, mut cmd_rx: mpsc::Receiver<Command>) {
        loop {
            // Idle: wait for a start request.
            match cmd_rx.recv().await {
                Some(Command::Start(ack) | Command::Restart(ack)) => {
                    let _ = ack.send(Ok(()));
                }
                Some(Command::Stop(ack)) => {
                    let _ = ack.send(());
                    continue;
                }
                Some(Command::Shutdown(ack)) => {
                    let _ = ack.send(());
                    return;
                }
                None => return,
            }

            // Active: spawn/health/restart loop until stopped or failed.
            if self.active_loop(&mut cmd_rx).await {
                return; // shutdown requested
            }
        }
    }

    /// Runs the spawn → health → monitor → restart cycle.
    /// Returns true when the whole supervisor should end (app shutdown).
    async fn active_loop(&self, cmd_rx: &mut mpsc::Receiver<Command>) -> bool {
        let mut attempt: u32 = 0;

        loop {
            self.set_state(SidecarState::Starting);

            let mut spawned = match self.spawn_once() {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(sidecar = %self.config.name, error = %e, "spawn failed");
                    match self
                        .backoff_or_fail(&mut attempt, &e.to_string(), cmd_rx)
                        .await
                    {
                        BackoffOutcome::Retry => continue,
                        BackoffOutcome::Stopped => return false,
                        BackoffOutcome::Shutdown => return true,
                        BackoffOutcome::Failed => return false,
                    }
                }
            };
            self.pid_store
                .record(&self.config.name, spawned.pid, &self.resolved_binary);

            // Health gate.
            match self.wait_healthy(&mut spawned).await {
                Ok(()) => {}
                Err(detail) => {
                    tracing::warn!(sidecar = %self.config.name, %detail, "health check failed; killing tree");
                    spawned.kill_tree().await;
                    self.pid_store.clear(&self.config.name);
                    match self.backoff_or_fail(&mut attempt, &detail, cmd_rx).await {
                        BackoffOutcome::Retry => continue,
                        BackoffOutcome::Stopped => return false,
                        BackoffOutcome::Shutdown => return true,
                        BackoffOutcome::Failed => return false,
                    }
                }
            }

            self.set_state(SidecarState::Healthy);
            let healthy_since = Instant::now();

            // Monitor: crash vs. command.
            tokio::select! {
                exit = spawned.child.wait() => {
                    self.pid_store.clear(&self.config.name);
                    let code = exit.ok().and_then(|s| s.code());
                    tracing::warn!(sidecar = %self.config.name, ?code, "exited unexpectedly");

                    // A long healthy run earns a fresh backoff schedule.
                    if let RestartPolicy::OnCrash { reset_after_secs, .. } = &self.config.restart {
                        if healthy_since.elapsed() >= Duration::from_secs(*reset_after_secs) {
                            attempt = 0;
                        }
                    }
                    let detail = format!("exited with code {code:?}");
                    match self.backoff_or_fail(&mut attempt, &detail, cmd_rx).await {
                        BackoffOutcome::Retry => continue,
                        BackoffOutcome::Stopped => return false,
                        BackoffOutcome::Shutdown => return true,
                        BackoffOutcome::Failed => return false,
                    }
                }
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(Command::Stop(ack)) => {
                            self.stop_process(&mut spawned).await;
                            let _ = ack.send(());
                            self.set_state(SidecarState::Stopped);
                            return false;
                        }
                        Some(Command::Restart(ack)) => {
                            self.stop_process(&mut spawned).await;
                            let _ = ack.send(Ok(()));
                            attempt = 0;
                            continue;
                        }
                        Some(Command::Shutdown(ack)) => {
                            self.stop_process(&mut spawned).await;
                            let _ = ack.send(());
                            self.set_state(SidecarState::Stopped);
                            return true;
                        }
                        Some(Command::Start(ack)) => {
                            let _ = ack.send(Ok(())); // already running
                            continue;
                        }
                        None => {
                            // Plugin dropped: take the tree down with us.
                            self.stop_process(&mut spawned).await;
                            return true;
                        }
                    }
                }
            }
        }
    }

    fn spawn_once(&self) -> Result<process::SpawnedProcess, SidecarError> {
        // Port resolution per spawn — a crashed sidecar may leave its old
        // port in TIME_WAIT; a fresh dynamic port sidesteps that entirely.
        let mut env: Vec<(String, String)> = self
            .config
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let port = match &self.config.port {
            PortStrategy::None => None,
            PortStrategy::Fixed { port, inject_env } => {
                ports::ensure_free(*port)?;
                env.push((inject_env.clone(), port.to_string()));
                Some(*port)
            }
            PortStrategy::Dynamic { inject_env } => {
                let port = ports::allocate_dynamic()?;
                env.push((inject_env.clone(), port.to_string()));
                Some(port)
            }
        };
        *self.port.lock() = port;

        if let (AuthStrategy::Token { inject_env }, Some(token)) = (&self.config.auth, &self.token)
        {
            env.push((inject_env.clone(), token.clone()));
        }

        let cwd = self
            .config
            .cwd
            .clone()
            .or_else(|| {
                self.resolved_binary
                    .parent()
                    .map(std::path::Path::to_path_buf)
            })
            .unwrap_or_else(|| PathBuf::from("."));

        let mut spawned = process::spawn(&self.resolved_binary, &self.config.args, &env, &cwd)?;
        self.attach_readers(&mut spawned);
        Ok(spawned)
    }

    /// Wires stdout/stderr line readers: ring buffer + tracing + sink +
    /// stdout-marker detection.
    fn attach_readers(&self, spawned: &mut process::SpawnedProcess) {
        let _ = self.marker_tx.send(false);

        let marker = match &self.config.health {
            HealthCheck::StdoutMarker { pattern, .. } => regex::Regex::new(pattern).ok(),
            _ => None,
        };

        if let Some(stdout) = spawned.child.stdout.take() {
            let ctx = ReaderCtx {
                name: self.config.name.clone(),
                stream: "stdout",
                logs: self.logs.clone(),
                sink: self.sink.clone(),
                emit: self.config.emit_logs,
                marker,
                marker_tx: Some(self.marker_tx.clone()),
            };
            tokio::spawn(read_lines(stdout, ctx));
        }
        if let Some(stderr) = spawned.child.stderr.take() {
            let ctx = ReaderCtx {
                name: self.config.name.clone(),
                stream: "stderr",
                logs: self.logs.clone(),
                sink: self.sink.clone(),
                emit: self.config.emit_logs,
                marker: None,
                marker_tx: None,
            };
            tokio::spawn(read_lines(stderr, ctx));
        }
    }

    async fn wait_healthy(&self, spawned: &mut process::SpawnedProcess) -> Result<(), String> {
        let deadline = self.config.health_timeout();
        let port = *self.port.lock();

        // The process dying during the health wait is also a failure — race
        // the probe against the exit.
        let probe = async {
            match &self.config.health {
                HealthCheck::Immediate => Ok(()),
                HealthCheck::Tcp { .. } => match port {
                    Some(p) => health::wait_tcp(p, deadline).await,
                    None => Err("tcp health check requires a port strategy".into()),
                },
                HealthCheck::Http { path, .. } => match port {
                    Some(p) => health::wait_http(p, path, deadline).await,
                    None => Err("http health check requires a port strategy".into()),
                },
                HealthCheck::StdoutMarker { .. } => {
                    let mut rx = self.marker_rx.clone();
                    let waited = timeout(deadline, async {
                        loop {
                            if *rx.borrow() {
                                return;
                            }
                            if rx.changed().await.is_err() {
                                return;
                            }
                        }
                    })
                    .await;
                    match waited {
                        Ok(()) if *self.marker_rx.borrow() => Ok(()),
                        _ => Err("stdout marker never appeared".into()),
                    }
                }
            }
        };

        tokio::select! {
            result = probe => result,
            exit = spawned.child.wait() => {
                let code = exit.ok().and_then(|s| s.code());
                Err(format!("process exited during health check (code {code:?})"))
            }
        }
    }

    async fn stop_process(&self, spawned: &mut process::SpawnedProcess) {
        self.set_state(SidecarState::Stopping);

        match &self.config.shutdown.graceful {
            GracefulShutdown::None => {}
            GracefulShutdown::Signal => spawned.signal_graceful(),
            GracefulShutdown::HttpPost { path } => {
                let port = *self.port.lock(); // drop guard before await
                if let Some(port) = port {
                    health::post_shutdown_hook(port, path).await;
                }
            }
        }

        let grace = Duration::from_secs(self.config.shutdown.grace_secs);
        if !matches!(self.config.shutdown.graceful, GracefulShutdown::None) {
            let _ = timeout(grace, spawned.child.wait()).await;
        }

        spawned.kill_tree().await;
        self.pid_store.clear(&self.config.name);
    }

    async fn backoff_or_fail(
        &self,
        attempt: &mut u32,
        detail: &str,
        cmd_rx: &mut mpsc::Receiver<Command>,
    ) -> BackoffOutcome {
        let delay = match &self.config.restart {
            RestartPolicy::Never => None,
            RestartPolicy::OnCrash { backoff_secs, .. } => {
                backoff_secs.get(*attempt as usize).copied()
            }
        };

        let Some(delay_secs) = delay else {
            self.set_state(SidecarState::Failed {
                reason: detail.to_string(),
            });
            return BackoffOutcome::Failed;
        };

        self.set_state(SidecarState::Backoff {
            attempt: *attempt + 1,
            delay_secs,
        });
        *attempt += 1;

        // Stay responsive to commands while waiting out the backoff.
        tokio::select! {
            () = sleep(Duration::from_secs(delay_secs)) => BackoffOutcome::Retry,
            cmd = cmd_rx.recv() => match cmd {
                Some(Command::Stop(ack)) => {
                    let _ = ack.send(());
                    self.set_state(SidecarState::Stopped);
                    BackoffOutcome::Stopped
                }
                Some(Command::Restart(ack) | Command::Start(ack)) => {
                    let _ = ack.send(Ok(()));
                    *attempt = 0;
                    BackoffOutcome::Retry
                }
                Some(Command::Shutdown(ack)) => {
                    let _ = ack.send(());
                    BackoffOutcome::Shutdown
                }
                None => BackoffOutcome::Shutdown,
            },
        }
    }
}

enum BackoffOutcome {
    Retry,
    Stopped,
    Shutdown,
    Failed,
}

struct ReaderCtx {
    name: String,
    stream: &'static str,
    logs: Arc<Mutex<VecDeque<LogLine>>>,
    sink: Arc<dyn EventSink>,
    emit: bool,
    marker: Option<regex::Regex>,
    marker_tx: Option<watch::Sender<bool>>,
}

async fn read_lines(reader: impl tokio::io::AsyncRead + Unpin, ctx: ReaderCtx) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::debug!(sidecar = %ctx.name, stream = ctx.stream, "{line}");

        if let (Some(re), Some(tx)) = (&ctx.marker, &ctx.marker_tx) {
            if re.is_match(&line) {
                let _ = tx.send(true);
            }
        }

        let log = LogLine {
            sidecar: ctx.name.clone(),
            stream: ctx.stream,
            line,
        };
        if ctx.emit {
            ctx.sink.log_line(&log);
        }
        let mut buf = ctx.logs.lock();
        if buf.len() == LOG_RING_CAPACITY {
            buf.pop_front();
        }
        buf.push_back(log);
    }
}

fn generate_token() -> String {
    use std::fmt::Write as _;

    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let mut hex = String::with_capacity(64);
    for b in bytes {
        let _ = write!(hex, "{b:02x}");
    }
    hex
}
