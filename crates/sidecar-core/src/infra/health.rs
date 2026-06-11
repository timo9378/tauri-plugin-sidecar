//! Health probes. A sidecar is not "up" when the process exists — it's up
//! when it answers. Dependents and the frontend only proceed on `Healthy`.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};

const PROBE_INTERVAL: Duration = Duration::from_millis(250);
const PROBE_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Polls a TCP connect until it succeeds or `deadline` elapses.
pub async fn wait_tcp(port: u16, deadline: Duration) -> Result<(), String> {
    let result = timeout(deadline, async {
        loop {
            if timeout(
                PROBE_CONNECT_TIMEOUT,
                TcpStream::connect(("127.0.0.1", port)),
            )
            .await
            .is_ok_and(|r| r.is_ok())
            {
                return;
            }
            sleep(PROBE_INTERVAL).await;
        }
    })
    .await;
    result.map_err(|_| format!("tcp connect to 127.0.0.1:{port} never succeeded"))
}

/// Polls `GET http://127.0.0.1:{port}{path}` until it returns 2xx or the
/// deadline elapses. Hand-rolled HTTP/1.1 — a full client dependency buys
/// nothing for a localhost status-line check.
pub async fn wait_http(port: u16, path: &str, deadline: Duration) -> Result<(), String> {
    let result = timeout(deadline, async {
        loop {
            if http_get_ok(port, path).await {
                return;
            }
            sleep(PROBE_INTERVAL).await;
        }
    })
    .await;
    result.map_err(|_| format!("GET 127.0.0.1:{port}{path} never returned 2xx"))
}

async fn http_get_ok(port: u16, path: &str) -> bool {
    let Ok(Ok(mut stream)) = timeout(
        PROBE_CONNECT_TIMEOUT,
        TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    else {
        return false;
    };
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).await.is_err() {
        return false;
    }
    let mut buf = [0u8; 64];
    let Ok(Ok(n)) = timeout(PROBE_CONNECT_TIMEOUT, stream.read(&mut buf)).await else {
        return false;
    };
    // "HTTP/1.1 2xx ..."
    let head = String::from_utf8_lossy(&buf[..n]);
    head.split_whitespace()
        .nth(1)
        .is_some_and(|code| code.starts_with('2'))
}

/// Fires `POST http://127.0.0.1:{port}{path}` once — the graceful-shutdown
/// hook. Errors are ignored; the kill-tree follows regardless.
pub async fn post_shutdown_hook(port: u16, path: &str) {
    let Ok(Ok(mut stream)) = timeout(
        PROBE_CONNECT_TIMEOUT,
        TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    else {
        return;
    };
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    let _ = stream.write_all(request.as_bytes()).await;
    let mut buf = [0u8; 64];
    let _ = timeout(PROBE_CONNECT_TIMEOUT, stream.read(&mut buf)).await;
}
