//! A runnable demonstration of the core engine without Tauri: two real
//! sidecars where one depends on the other, with health gating, a token, and
//! clean shutdown. Run with `cargo run -p sidecar-core --example two_sidecars`
//! (Unix; uses python3).
//!
//! Examples print and unwrap freely — they are demos, not library code.
#![allow(clippy::print_stdout, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use sidecar_core::{EventSink, HealthCheck, LogLine, SidecarConfig, SidecarManager, SidecarState};

struct StdoutSink;

impl EventSink for StdoutSink {
    fn state_changed(&self, sidecar: &str, state: &SidecarState) {
        println!("  [{sidecar}] -> {state:?}");
    }
    fn log_line(&self, line: &LogLine) {
        println!("  [{}:{}] {}", line.sidecar, line.stream, line.line);
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber_init();

    let api = SidecarConfig::new("api", "/usr/bin/python3")
        .args([
            "-c",
            "import http.server,os;\
             print('TOKEN seen:', os.environ.get('API_TOKEN','none'), flush=True);\
             http.server.HTTPServer(('127.0.0.1',int(os.environ['API_PORT'])),\
             http.server.BaseHTTPRequestHandler.__subclasses__()[0] if False else \
             type('H',(http.server.BaseHTTPRequestHandler,),{'do_GET':lambda s:(s.send_response(200),s.end_headers()),'log_message':lambda *a:None})).serve_forever()",
        ])
        .dynamic_port("API_PORT")
        .auth_token("API_TOKEN")
        .health(HealthCheck::Tcp { timeout_secs: 10 })
        .emit_logs(true);

    let worker = SidecarConfig::new("worker", "/bin/sh")
        .args([
            "-c",
            "echo starting; sleep 0.3; echo WORKER_READY; sleep 600",
        ])
        .depends_on("api")
        .health(HealthCheck::StdoutMarker {
            pattern: "WORKER_READY".into(),
            timeout_secs: 10,
        })
        .emit_logs(true);

    let dir = std::env::temp_dir().join("sidecar-example");
    std::fs::create_dir_all(&dir).unwrap();

    println!("launching (worker waits for api to be healthy)...");
    let manager = SidecarManager::launch(
        vec![api, worker],
        Arc::new(StdoutSink),
        &dir,
        <std::path::PathBuf as Clone>::clone,
    )
    .expect("launch");

    manager.start_all().await;

    let port = manager.port("api").unwrap();
    println!("\napi is healthy on port {port:?}; both sidecars up. Holding 2s...\n");
    tokio::time::sleep(Duration::from_secs(2)).await;

    println!("shutting down (reverse dependency order, kill-tree)...");
    manager.shutdown_all().await;
    println!("done — no orphans.");
}

fn tracing_subscriber_init() {
    // Keep the example output readable; the engine logs via `tracing`.
    let _ = std::env::var("RUST_LOG");
}
