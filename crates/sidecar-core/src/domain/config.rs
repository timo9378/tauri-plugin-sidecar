//! Declarative sidecar configuration — the docker-compose of Tauri sidecars.
//!
//! Everything an app developer states about a sidecar lives here; everything
//! about *how* it is kept alive lives in the supervisor. The split is the
//! product: lifecycle machinery is generic, your sidecar is just config.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Complete description of one sidecar process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarConfig {
    /// Unique name, used in commands, events, and logs.
    pub name: String,
    /// Path to the executable. Absolute paths run as-is; bare program names
    /// (`python3`, `powershell.exe`) are resolved by the OS through `PATH`;
    /// relative paths with directory components resolve against the Tauri
    /// resource directory at runtime (where bundled sidecars land).
    pub binary: PathBuf,
    /// Arguments passed to the binary.
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Working directory. Defaults to the binary's parent directory —
    /// installers launch apps with surprising CWDs; never inherit one.
    /// Bare PATH-resolved program names have no parent and keep the app's
    /// own working directory.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    /// How the sidecar gets its listening port.
    #[serde(default)]
    pub port: PortStrategy,
    /// Auth material injected at spawn so only this app can talk to it.
    #[serde(default)]
    pub auth: AuthStrategy,
    /// How readiness is determined before dependents start.
    #[serde(default)]
    pub health: HealthCheck,
    /// What to do when the process exits without being asked to.
    #[serde(default)]
    pub restart: RestartPolicy,
    /// How shutdown is performed.
    #[serde(default)]
    pub shutdown: ShutdownPolicy,
    /// Names of sidecars that must be healthy before this one starts.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Forward each stdout/stderr line to the frontend as an event
    /// (`sidecar://log`). Lines always go to `tracing` regardless.
    #[serde(default)]
    pub emit_logs: bool,
}

/// How a sidecar learns which port to bind.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PortStrategy {
    /// The sidecar does not listen on a port (or manages its own).
    #[default]
    None,
    /// A fixed port. The supervisor verifies it is free before spawning and
    /// fails fast with a clear error instead of letting the sidecar crash.
    Fixed { port: u16, inject_env: String },
    /// The supervisor picks a free port at spawn time and injects it via the
    /// given environment variable. No more hardcoded-port collisions.
    Dynamic { inject_env: String },
}

/// Auth material generated per app session and injected at spawn.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthStrategy {
    /// No auth material injected.
    #[default]
    None,
    /// A session token injected via the given environment variable. The
    /// sidecar should reject requests without it — a sidecar not launched
    /// by this app simply never learns the token.
    ///
    /// With `value: None` a fresh random 32-byte hex token is generated.
    /// Provide a `value` when several consumers (other sidecars, the
    /// frontend) must share one secret; either way the effective token is
    /// readable back through `SidecarManager::auth_token`.
    Token {
        inject_env: String,
        #[serde(default)]
        value: Option<String>,
    },
}

/// Readiness probe. A sidecar is only "healthy" — and its dependents only
/// start — once this passes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[derive(Default)]
pub enum HealthCheck {
    /// Considered healthy as soon as the process is running.
    #[default]
    Immediate,
    /// A TCP connect to the sidecar's port succeeds.
    Tcp {
        #[serde(default = "default_health_timeout_secs")]
        timeout_secs: u64,
    },
    /// An HTTP GET to `http://127.0.0.1:{port}{path}` returns 2xx.
    Http {
        path: String,
        #[serde(default = "default_health_timeout_secs")]
        timeout_secs: u64,
    },
    /// A line of stdout matches the given regex (e.g. `"^READY$"`).
    StdoutMarker {
        pattern: String,
        #[serde(default = "default_health_timeout_secs")]
        timeout_secs: u64,
    },
}

fn default_health_timeout_secs() -> u64 {
    30
}

/// Crash handling. Restarts apply only to *unexpected* exits — a `stop()`
/// requested by the app never triggers a restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RestartPolicy {
    /// Stay down after any exit.
    Never,
    /// Restart with the given backoff schedule (seconds). After the schedule
    /// is exhausted the sidecar enters `Failed`. The schedule resets once the
    /// sidecar stays healthy for `reset_after_secs`.
    OnCrash {
        #[serde(default = "default_backoff")]
        backoff_secs: Vec<u64>,
        #[serde(default = "default_reset_after_secs")]
        reset_after_secs: u64,
    },
}

impl Default for RestartPolicy {
    fn default() -> Self {
        RestartPolicy::OnCrash {
            backoff_secs: default_backoff(),
            reset_after_secs: default_reset_after_secs(),
        }
    }
}

fn default_backoff() -> Vec<u64> {
    vec![1, 2, 4, 8]
}

fn default_reset_after_secs() -> u64 {
    60
}

/// Shutdown behaviour. Whatever happens, the *entire process tree* dies —
/// a sidecar that spawns children must not leave orphans holding ports and
/// file handles after the app quits.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ShutdownPolicy {
    /// Optional graceful step before force-kill.
    #[serde(default)]
    pub graceful: GracefulShutdown,
    /// How long to wait for a graceful exit before the kill-tree.
    #[serde(default = "default_grace_secs")]
    pub grace_secs: u64,
}

impl Default for ShutdownPolicy {
    fn default() -> Self {
        ShutdownPolicy {
            graceful: GracefulShutdown::default(),
            grace_secs: default_grace_secs(),
        }
    }
}

fn default_grace_secs() -> u64 {
    5
}

/// Graceful shutdown step attempted before the process tree is killed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GracefulShutdown {
    /// Unix: SIGTERM to the process group. Windows: skip straight to the
    /// grace wait (arbitrary Win32 processes have no SIGTERM equivalent).
    #[default]
    Signal,
    /// POST to `http://127.0.0.1:{port}{path}` — works on every platform and
    /// lets the sidecar flush state. The most reliable option in practice.
    HttpPost { path: String },
    /// No graceful step; kill the tree immediately.
    None,
}

impl SidecarConfig {
    /// Starts a builder for a sidecar with the given unique name.
    pub fn new(name: impl Into<String>, binary: impl Into<PathBuf>) -> Self {
        SidecarConfig {
            name: name.into(),
            binary: binary.into(),
            args: Vec::new(),
            env: HashMap::new(),
            cwd: None,
            port: PortStrategy::None,
            auth: AuthStrategy::None,
            health: HealthCheck::Immediate,
            restart: RestartPolicy::default(),
            shutdown: ShutdownPolicy::default(),
            depends_on: Vec::new(),
            emit_logs: false,
        }
    }

    /// Sets the process arguments.
    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    /// Adds an environment variable (in addition to the inherited environment).
    #[must_use]
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Sets the working directory (defaults to the binary's parent directory).
    #[must_use]
    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    /// Sets the port strategy.
    #[must_use]
    pub fn port(mut self, strategy: PortStrategy) -> Self {
        self.port = strategy;
        self
    }

    /// Convenience: a free port chosen at spawn time, injected via `env_var`.
    #[must_use]
    pub fn dynamic_port(self, env_var: impl Into<String>) -> Self {
        self.port(PortStrategy::Dynamic {
            inject_env: env_var.into(),
        })
    }

    /// Sets the auth strategy.
    #[must_use]
    pub fn auth(mut self, strategy: AuthStrategy) -> Self {
        self.auth = strategy;
        self
    }

    /// Convenience: a fresh session token injected via `env_var`.
    #[must_use]
    pub fn auth_token(self, env_var: impl Into<String>) -> Self {
        self.auth(AuthStrategy::Token {
            inject_env: env_var.into(),
            value: None,
        })
    }

    /// Convenience: an app-provided token injected via `env_var` — use this
    /// when several sidecars (or the frontend) must share one secret.
    #[must_use]
    pub fn auth_token_value(self, env_var: impl Into<String>, value: impl Into<String>) -> Self {
        self.auth(AuthStrategy::Token {
            inject_env: env_var.into(),
            value: Some(value.into()),
        })
    }

    /// Sets the readiness probe.
    #[must_use]
    pub fn health(mut self, check: HealthCheck) -> Self {
        self.health = check;
        self
    }

    /// Sets the crash-restart policy.
    #[must_use]
    pub fn restart(mut self, policy: RestartPolicy) -> Self {
        self.restart = policy;
        self
    }

    /// Sets the shutdown policy.
    #[must_use]
    pub fn shutdown(mut self, policy: ShutdownPolicy) -> Self {
        self.shutdown = policy;
        self
    }

    /// Declares that this sidecar must not start until `name` is healthy.
    #[must_use]
    pub fn depends_on(mut self, name: impl Into<String>) -> Self {
        self.depends_on.push(name.into());
        self
    }

    /// Forwards each captured output line to the frontend as a `sidecar://log`
    /// event (lines always go to `tracing` regardless).
    #[must_use]
    pub fn emit_logs(mut self, emit: bool) -> Self {
        self.emit_logs = emit;
        self
    }

    pub(crate) fn health_timeout(&self) -> Duration {
        let secs = match &self.health {
            HealthCheck::Immediate => 0,
            HealthCheck::Tcp { timeout_secs } => *timeout_secs,
            HealthCheck::Http { timeout_secs, .. } => *timeout_secs,
            HealthCheck::StdoutMarker { timeout_secs, .. } => *timeout_secs,
        };
        Duration::from_secs(secs)
    }
}
