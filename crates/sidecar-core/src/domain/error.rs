//! Error types — every failure a sidecar can hit, with enough context to be
//! actionable from a log line alone.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    #[error("failed to spawn `{binary}`: {source}")]
    Spawn {
        binary: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("process exited before it could be tracked")]
    AlreadyExited,

    #[error("fixed port {port} is already in use — another instance running, or an orphan from a previous run?")]
    PortInUse { port: u16 },

    #[error("no free port could be allocated: {0}")]
    PortAllocation(std::io::Error),

    #[error("health check did not pass within {timeout_secs}s ({detail})")]
    HealthTimeout { timeout_secs: u64, detail: String },

    #[error("unknown sidecar `{0}`")]
    UnknownSidecar(String),

    #[error("dependency cycle involving `{0}`")]
    DependencyCycle(String),

    #[error("dependency `{dep}` of `{name}` is not registered")]
    UnknownDependency { name: String, dep: String },

    #[error("dependency `{dep}` failed; not starting `{name}`")]
    DependencyFailed { name: String, dep: String },

    #[error(
        "SidecarManager::launch requires a tokio runtime context — call it inside \
         tauri::async_runtime::block_on (or any tokio runtime)"
    )]
    NoAsyncRuntime,

    #[error("os error: {0}")]
    Os(String),
}
