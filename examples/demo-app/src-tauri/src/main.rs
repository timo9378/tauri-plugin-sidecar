//! Minimal real Tauri app exercising tauri-plugin-sidecar end to end:
//! dynamic port + token injection, TCP/stdout health gating, `depends_on`
//! ordering, live state/log events, and — the part worth field-testing on
//! Windows — kill-tree over a sidecar that spawns its own children.
//!
//! Run with `cargo run` from this directory (or `cargo tauri dev`).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri_plugin_sidecar::{Builder, HealthCheck, SidecarConfig};

/// A tiny HTTP server: gets a dynamic port and a session token injected,
/// health-gated on the port actually accepting connections.
fn api_sidecar() -> SidecarConfig {
    #[cfg(unix)]
    let config = SidecarConfig::new("api", "/usr/bin/python3").args([
        "-c",
        "import http.server,os;\
         print('token:', os.environ.get('API_TOKEN','none')[:8] + '…', flush=True);\
         http.server.HTTPServer(('127.0.0.1',int(os.environ['API_PORT'])),\
         type('H',(http.server.BaseHTTPRequestHandler,),\
         {'do_GET':lambda s:(s.send_response(200),s.end_headers()),\
          'log_message':lambda *a:None})).serve_forever()",
    ]);

    #[cfg(windows)]
    let config = SidecarConfig::new("api", "powershell.exe").args([
        "-NoProfile",
        "-Command",
        "$l=New-Object Net.HttpListener;\
         $l.Prefixes.Add('http://localhost:'+$env:API_PORT+'/');\
         $l.Start();\
         Write-Output ('token: '+$env:API_TOKEN.Substring(0,8)+'...');\
         while($true){$c=$l.GetContext();$c.Response.StatusCode=200;$c.Response.Close()}",
    ]);

    config
        .dynamic_port("API_PORT")
        .auth_token("API_TOKEN")
        .health(HealthCheck::Tcp { timeout_secs: 15 })
        .emit_logs(true)
}

/// A sidecar that deliberately spawns child processes of its own, so Stop /
/// app exit must take down the *whole tree* — on Windows this is the Job
/// Object path (watch Task Manager: no ping.exe may survive).
fn tree_sidecar() -> SidecarConfig {
    #[cfg(unix)]
    let config = SidecarConfig::new("tree", "/bin/sh").args([
        "-c",
        "echo TREE_READY; sleep 600 & sleep 600 & wait",
    ]);

    #[cfg(windows)]
    let config = SidecarConfig::new("tree", "powershell.exe").args([
        "-NoProfile",
        "-Command",
        "Write-Output TREE_READY;\
         Start-Process -WindowStyle Hidden ping -ArgumentList '-t','127.0.0.1';\
         ping -t 127.0.0.1 | Out-Null",
    ]);

    config
        .depends_on("api")
        .health(HealthCheck::StdoutMarker {
            pattern: "TREE_READY".into(),
            timeout_secs: 15,
        })
        .emit_logs(true)
}

fn main() {
    tauri::Builder::default()
        .plugin(
            Builder::new()
                .sidecar(api_sidecar())
                .sidecar(tree_sidecar())
                .autostart(true)
                .build(),
        )
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
