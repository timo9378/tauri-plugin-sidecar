//! Async orchestration: the per-sidecar supervisor task and the manager
//! that owns ordering, commands, and shutdown.

pub mod manager;
pub mod supervisor;
