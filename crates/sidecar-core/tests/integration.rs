//! Integration tests against real OS processes (Unix).
//!
//! These spawn actual `sh`/`python3` children to prove the claims that
//! matter: whole-tree kills, crash backoff, health gating, dependency
//! ordering, and orphan cleanup.

#![cfg(unix)]
// Tests use unwrap/expect freely (project standard: acceptable in test code).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use sidecar_core::{
    EventSink, GracefulShutdown, HealthCheck, LogLine, RestartPolicy, ShutdownPolicy,
    SidecarConfig, SidecarManager, SidecarState,
};

/// Collects every event for assertions.
#[derive(Default)]
struct RecordingSink {
    states: Mutex<Vec<(String, SidecarState)>>,
}

impl EventSink for RecordingSink {
    fn state_changed(&self, sidecar: &str, state: &SidecarState) {
        self.states
            .lock()
            .unwrap()
            .push((sidecar.to_string(), state.clone()));
    }
    fn log_line(&self, _line: &LogLine) {}
}

impl RecordingSink {
    fn states_of(&self, name: &str) -> Vec<SidecarState> {
        self.states
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, s)| s.clone())
            .collect()
    }
}

fn sh(name: &str, script: &str) -> SidecarConfig {
    SidecarConfig::new(name, "/bin/sh").args(["-c", script])
}

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

fn pid_alive(pid: i32) -> bool {
    // SAFETY: kill(pid, 0) sends no signal; it only probes existence and
    // touches no memory. Returns 0 if the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn launch(
    configs: Vec<SidecarConfig>,
    sink: Arc<RecordingSink>,
    dir: &std::path::Path,
) -> SidecarManager {
    SidecarManager::launch(configs, sink, dir, <std::path::PathBuf as Clone>::clone).unwrap()
}

#[tokio::test]
async fn kill_tree_reaches_grandchildren() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingSink::default());
    // The sidecar spawns a child and prints its pid, then waits forever.
    let manager = launch(
        vec![
            sh("tree", "sleep 300 & echo CHILD_PID=$!; echo READY; wait")
                .health(HealthCheck::StdoutMarker {
                    pattern: "READY".into(),
                    timeout_secs: 10,
                })
                .shutdown(ShutdownPolicy {
                    graceful: GracefulShutdown::None,
                    grace_secs: 1,
                }),
        ],
        sink.clone(),
        dir.path(),
    );

    manager.start("tree").await.unwrap();
    wait_for_state(
        &manager,
        "tree",
        |s| matches!(s, SidecarState::Healthy),
        Duration::from_secs(10),
    )
    .await;

    // Find the grandchild pid from the log buffer.
    let logs = manager.logs("tree", 50).unwrap();
    let child_pid: i32 = logs
        .iter()
        .find_map(|l| l.split("CHILD_PID=").nth(1))
        .expect("child pid logged")
        .trim()
        .parse()
        .unwrap();
    assert!(
        pid_alive(child_pid),
        "grandchild should be alive while running"
    );

    manager.stop("tree").await.unwrap();
    wait_for_state(
        &manager,
        "tree",
        |s| matches!(s, SidecarState::Stopped),
        Duration::from_secs(10),
    )
    .await;

    // The whole tree must be gone — this is the core promise.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !pid_alive(child_pid),
        "grandchild must die with the sidecar"
    );
}

#[tokio::test]
async fn crash_restarts_with_backoff_then_fails() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingSink::default());
    let manager = launch(
        vec![sh("crasher", "exit 7").restart(RestartPolicy::OnCrash {
            backoff_secs: vec![0, 0],
            reset_after_secs: 3600,
        })],
        sink.clone(),
        dir.path(),
    );

    manager.start("crasher").await.unwrap();
    wait_for_state(
        &manager,
        "crasher",
        |s| matches!(s, SidecarState::Failed { .. }),
        Duration::from_secs(15),
    )
    .await;

    let states = sink.states_of("crasher");
    let backoffs = states
        .iter()
        .filter(|s| matches!(s, SidecarState::Backoff { .. }))
        .count();
    let starts = states
        .iter()
        .filter(|s| matches!(s, SidecarState::Starting))
        .count();
    assert_eq!(
        backoffs, 2,
        "two backoff entries for a [0,0] schedule: {states:?}"
    );
    assert_eq!(starts, 3, "initial start + two retries: {states:?}");
}

#[tokio::test]
async fn requested_stop_does_not_restart() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingSink::default());
    let manager = launch(
        vec![sh("steady", "sleep 300").health(HealthCheck::Immediate)],
        sink.clone(),
        dir.path(),
    );

    manager.start("steady").await.unwrap();
    wait_for_state(
        &manager,
        "steady",
        |s| matches!(s, SidecarState::Healthy),
        Duration::from_secs(10),
    )
    .await;
    manager.stop("steady").await.unwrap();
    wait_for_state(
        &manager,
        "steady",
        |s| matches!(s, SidecarState::Stopped),
        Duration::from_secs(10),
    )
    .await;

    // Linger and verify no Starting follows the stop.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let states = sink.states_of("steady");
    let stop_idx = states
        .iter()
        .position(|s| matches!(s, SidecarState::Stopped))
        .unwrap();
    assert!(
        !states[stop_idx..]
            .iter()
            .any(|s| matches!(s, SidecarState::Starting)),
        "no restart after a requested stop: {states:?}"
    );
}

#[tokio::test]
async fn dynamic_port_injected_and_health_gates_on_it() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingSink::default());
    // Python http server binds the injected port; Tcp health must pass.
    let manager = launch(
        vec![SidecarConfig::new("web", "/usr/bin/python3")
            .args(["-c", "import http.server, os; http.server.HTTPServer(('127.0.0.1', int(os.environ['WEB_PORT'])), http.server.SimpleHTTPRequestHandler).serve_forever()"])
            .dynamic_port("WEB_PORT")
            .health(HealthCheck::Tcp { timeout_secs: 15 })],
        sink.clone(),
        dir.path(),
    );

    manager.start("web").await.unwrap();
    wait_for_state(
        &manager,
        "web",
        |s| matches!(s, SidecarState::Healthy),
        Duration::from_secs(15),
    )
    .await;

    let port = manager.port("web").unwrap().expect("port allocated");
    assert!(port > 0);

    // The port really answers.
    let conn = tokio::net::TcpStream::connect(("127.0.0.1", port)).await;
    assert!(conn.is_ok(), "allocated port should accept connections");

    manager.shutdown_all().await;
}

#[tokio::test]
async fn http_health_and_auth_token_injection() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingSink::default());
    // Server echoes 200 on /healthz and prints the injected token.
    let py = r#"
import http.server, os
print("TOKEN=" + os.environ.get("APP_TOKEN", "missing"), flush=True)
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200 if self.path == "/healthz" else 404)
        self.end_headers()
    def log_message(self, *a): pass
http.server.HTTPServer(("127.0.0.1", int(os.environ["WEB_PORT"])), H).serve_forever()
"#;
    let manager = launch(
        vec![SidecarConfig::new("api", "/usr/bin/python3")
            .args(["-c", py])
            .dynamic_port("WEB_PORT")
            .auth_token("APP_TOKEN")
            .health(HealthCheck::Http {
                path: "/healthz".into(),
                timeout_secs: 15,
            })],
        sink.clone(),
        dir.path(),
    );

    manager.start("api").await.unwrap();
    wait_for_state(
        &manager,
        "api",
        |s| matches!(s, SidecarState::Healthy),
        Duration::from_secs(15),
    )
    .await;

    let logs = manager.logs("api", 50).unwrap();
    let token_line = logs
        .iter()
        .find(|l| l.contains("TOKEN="))
        .expect("token logged");
    let token = token_line.split("TOKEN=").nth(1).unwrap().trim();
    assert_eq!(token.len(), 64, "32-byte hex token injected, got: {token}");

    manager.shutdown_all().await;
}

#[tokio::test]
async fn depends_on_gates_startup_order() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingSink::default());
    let manager = launch(
        vec![
            sh("first", "sleep 1; echo READY; sleep 300").health(HealthCheck::StdoutMarker {
                pattern: "READY".into(),
                timeout_secs: 10,
            }),
            sh("second", "sleep 300").depends_on("first"),
        ],
        sink.clone(),
        dir.path(),
    );

    manager.start_all().await;
    wait_for_state(
        &manager,
        "second",
        |s| matches!(s, SidecarState::Healthy),
        Duration::from_secs(15),
    )
    .await;

    // `second` must not start before `first` is healthy.
    let all = sink.states.lock().unwrap().clone();
    let first_healthy = all
        .iter()
        .position(|(n, s)| n == "first" && matches!(s, SidecarState::Healthy))
        .unwrap();
    let second_starting = all
        .iter()
        .position(|(n, s)| n == "second" && matches!(s, SidecarState::Starting))
        .unwrap();
    assert!(
        second_starting > first_healthy,
        "second started before first was healthy: {all:?}"
    );

    manager.shutdown_all().await;
}

#[tokio::test]
async fn dependency_cycle_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingSink::default());
    let result = SidecarManager::launch(
        vec![
            sh("a", "sleep 1").depends_on("b"),
            sh("b", "sleep 1").depends_on("a"),
        ],
        sink,
        dir.path(),
        <std::path::PathBuf as Clone>::clone,
    );
    assert!(result.is_err(), "cycles must be rejected at launch");
}

#[tokio::test]
async fn orphan_from_previous_run_is_killed() {
    let dir = tempfile::tempdir().unwrap();

    // Simulate a previous run with a *true* orphan: a shell backgrounds the
    // sleep and exits, so the sleep is reparented to init — exactly like a
    // sidecar surviving a crashed app. (A direct child would only zombie on
    // kill, which `kill(pid, 0)` still reports as alive.)
    let out = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            "setsid sleep 300 </dev/null >/dev/null 2>&1 & echo $!",
        ])
        .output()
        .unwrap();
    let orphan_pid: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
    std::fs::write(
        dir.path().join("sidecar-pids.json"),
        serde_json::json!({ "entries": { "ghost": [orphan_pid, "sleep"] } }).to_string(),
    )
    .unwrap();
    assert!(pid_alive(i32::try_from(orphan_pid).unwrap()));

    // A fresh manager launch must clean it up.
    let sink = Arc::new(RecordingSink::default());
    let _manager = launch(vec![sh("ghost", "sleep 300")], sink, dir.path());

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !pid_alive(i32::try_from(orphan_pid).unwrap()),
        "orphan from previous run must be killed at startup"
    );
}

#[tokio::test]
async fn graceful_http_hook_is_called_before_kill() {
    let dir = tempfile::tempdir().unwrap();
    let sink = Arc::new(RecordingSink::default());
    // Server prints SHUTDOWN_HOOK when POSTed, then exits 0 by itself.
    let py = r#"
import http.server, os, threading, sys
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200); self.end_headers()
    def do_POST(self):
        if self.path == "/shutdown":
            print("SHUTDOWN_HOOK", flush=True)
            self.send_response(200); self.end_headers()
            threading.Thread(target=lambda: (server.shutdown(), sys.exit(0))).start()
    def log_message(self, *a): pass
server = http.server.HTTPServer(("127.0.0.1", int(os.environ["WEB_PORT"])), H)
server.serve_forever()
"#;
    let manager = launch(
        vec![SidecarConfig::new("hooked", "/usr/bin/python3")
            .args(["-c", py])
            .dynamic_port("WEB_PORT")
            .health(HealthCheck::Http {
                path: "/".into(),
                timeout_secs: 15,
            })
            .shutdown(ShutdownPolicy {
                graceful: GracefulShutdown::HttpPost {
                    path: "/shutdown".into(),
                },
                grace_secs: 5,
            })],
        sink.clone(),
        dir.path(),
    );

    manager.start("hooked").await.unwrap();
    wait_for_state(
        &manager,
        "hooked",
        |s| matches!(s, SidecarState::Healthy),
        Duration::from_secs(15),
    )
    .await;
    manager.stop("hooked").await.unwrap();

    let logs = manager.logs("hooked", 50).unwrap();
    assert!(
        logs.iter().any(|l| l.contains("SHUTDOWN_HOOK")),
        "graceful hook should fire before the kill: {logs:?}"
    );
}
