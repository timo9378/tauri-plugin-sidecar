//! Windows integration tests against real OS processes.
//!
//! The Unix suite (`integration.rs`) covers supervision semantics in depth;
//! this file pins the Windows-specific spawn path with a real process:
//! a bare program name resolved via `PATH` (which also exercises the
//! empty-parent cwd default), stdout-marker health, and stop through the
//! Job Object kill path. Regression test for the first Windows field bug,
//! where `powershell.exe` was wrongly anchored to the resource directory.

#![cfg(windows)]
// Tests use unwrap/expect freely (project standard: acceptable in test code).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use sidecar_core::{
    GracefulShutdown, HealthCheck, NullSink, RestartPolicy, ShutdownPolicy, SidecarConfig,
    SidecarManager, SidecarState,
};

async fn wait_for_state(
    manager: &SidecarManager,
    name: &str,
    want: fn(&SidecarState) -> bool,
    timeout: Duration,
) -> SidecarState {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let state = manager.state(name).unwrap();
        if want(&state) {
            return state;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting; last state: {state:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn bare_program_name_spawns_via_path_and_stops() {
    let dir = tempfile::tempdir().unwrap();
    let manager = SidecarManager::launch(
        vec![SidecarConfig::new("ps", "powershell.exe")
            .args([
                "-NoProfile",
                "-Command",
                "Write-Output WIN_READY; Start-Sleep 600",
            ])
            .health(HealthCheck::StdoutMarker {
                pattern: "WIN_READY".into(),
                timeout_secs: 30,
            })
            .restart(RestartPolicy::Never)
            .shutdown(ShutdownPolicy {
                graceful: GracefulShutdown::None,
                grace_secs: 1,
            })],
        Arc::new(NullSink),
        dir.path(),
        <std::path::PathBuf as Clone>::clone,
    )
    .unwrap();

    manager.start("ps").await.unwrap();
    wait_for_state(
        &manager,
        "ps",
        |s| matches!(s, SidecarState::Healthy),
        Duration::from_secs(30),
    )
    .await;

    manager.stop("ps").await.unwrap();
    wait_for_state(
        &manager,
        "ps",
        |s| matches!(s, SidecarState::Stopped),
        Duration::from_secs(10),
    )
    .await;
}
