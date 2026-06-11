# tauri-plugin-sidecar

**Production-grade sidecar lifecycle for Tauri v2 — the docker-compose of sidecars.**

[![CI](https://github.com/timo9378/tauri-plugin-sidecar/actions/workflows/ci.yml/badge.svg)](https://github.com/timo9378/tauri-plugin-sidecar/actions/workflows/ci.yml)
[![MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

Tauri can *spawn* a sidecar. Keeping one alive in production is the hard part:
crash restarts, port collisions, health gating, auth, and — the one everyone
hits — **shutdown**. A sidecar that forks its own workers (a Python server, a
.NET host, a bundled `ollama`) leaves orphans holding ports and file handles
when the app quits, and then *the app won't launch a second time*.

This plugin makes all of that declarative. You describe each sidecar once; the
plugin owns the lifecycle.

> Built to answer [tauri-apps/plugins-workspace#3062][issue] — the open request
> for exactly this. If you landed here from that thread: yes, this is the
> "spawning, monitoring, health checks, auto-restart, graceful shutdown,
> orphan cleanup, cross-platform signals" list, implemented.

## Quick start

```rust
use tauri_plugin_sidecar::{Builder, SidecarConfig, HealthCheck, GracefulShutdown, ShutdownPolicy};

tauri::Builder::default()
    .plugin(
        Builder::new()
            // A .NET backend on a fixed port, with a graceful HTTP shutdown.
            .sidecar(
                SidecarConfig::new("backend", "binaries/backend/server")
                    .dynamic_port("BACKEND_PORT")        // injected; no hardcoded collisions
                    .auth_token("BACKEND_TOKEN")         // fresh 32-byte token per launch
                    .health(HealthCheck::Http { path: "/healthz".into(), timeout_secs: 30 })
                    .shutdown(ShutdownPolicy {
                        graceful: GracefulShutdown::HttpPost { path: "/shutdown".into() },
                        grace_secs: 5,
                    }),
            )
            // A Python ASR server that must not start until the backend is healthy.
            .sidecar(
                SidecarConfig::new("asr", "binaries/asr/asr-server")
                    .depends_on("backend")
                    .health(HealthCheck::StdoutMarker { pattern: "READY".into(), timeout_secs: 60 }),
            )
            .autostart(true)
            .build(),
    )
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
```

That's the whole integration. The two sidecars above are, almost verbatim, the
shape that took dozens of hand-rolled commits in a real shipping app —
collapsed into declarations.

## What it handles

| Concern | What you get |
|---|---|
| **Kill the whole tree** | Unix process groups (`killpg`) and Windows **Job Objects** (`KILL_ON_JOB_CLOSE`) — grandchildren die too, even if the app crashes. A `taskkill /T` sweep backs it up. |
| **Orphan cleanup** | Sidecars surviving a previous crash are killed on next launch (matched by pid **and** exe name, so recycled pids are safe). |
| **Crash restarts** | Configurable backoff schedule; a sustained healthy run resets it; a *requested* stop never restarts. |
| **Ports** | Dynamic allocation injected via env var, or a fixed port with fail-fast collision detection. |
| **Auth** | A fresh random token per session, injected at spawn — a sidecar not launched by your app never learns it. |
| **Health gating** | TCP connect, HTTP 2xx, or a stdout regex marker — with timeouts. Dependents only start once a sidecar is *healthy*, not merely *running*. |
| **Dependency order** | `depends_on` with topological startup and reverse-order shutdown; cycles rejected at launch. |
| **Logs** | Per-sidecar ring buffer (tail via the `logs` command); opt into `sidecar://log` events to stream to the webview. |

## Frontend

Typed bindings live in [`guest-js/`](guest-js/) (not yet on npm — same
feedback-first reasoning as crates.io):

```ts
import { status, restart, onStateChange, onLog } from "tauri-plugin-sidecar-api";

const fleet = await status();                 // [{ name, state, port }]
await restart("backend");
await onStateChange(({ name, state }) => console.log(name, state));
```

Under the hood these are plain Tauri commands and events
(`plugin:sidecar|status`, `sidecar://state`, `sidecar://log`), so raw
`invoke`/`listen` works too.

Default permissions grant read-only `status` and `logs`; `start`/`stop`/
`restart` are opt-in (add `allow-start`, etc. to your capability).

## Try it

[`examples/demo-app`](examples/demo-app/) is a runnable Tauri app (static
HTML UI, no Node needed) with two sidecars: an HTTP server that receives a
dynamic port + session token, and a dependent sidecar that deliberately
spawns child processes — stop it and watch the whole tree die, on Windows
included.

```sh
cd examples/demo-app/src-tauri && cargo run
```

## Design

Two crates:

- **`sidecar-core`** — the supervision engine, with no dependency on Tauri. It
  can be unit-tested against real processes (and reused outside Tauri).
- **`tauri-plugin-sidecar`** — the thin plugin layer that wires the engine to a
  Tauri app, events, and commands.

The core is layered with one-way dependencies (`runtime → domain + platform +
infra`); `domain` is pure and IO-free. See [CONTRIBUTING.md](CONTRIBUTING.md).

## Status

Early but real: the supervision engine is covered by integration tests that
drive actual OS processes (kill-tree reaching grandchildren, crash backoff,
health gating, dependency ordering, orphan cleanup), and CI runs clippy
`-D warnings` and the test suite on Linux, Windows, and macOS.

**Not yet covered** (and honest about it): the bundling layer (large-runtime
download, NSIS 2 GB limit, antivirus-aware extraction) is planned as a separate
opt-in, not in this crate yet. Windows Job Object behavior is verified to
compile and is exercised in CI; field reports on heavy real-world trees
(forking Python workers, bundled `ollama`) are very welcome.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your
option.

[issue]: https://github.com/tauri-apps/plugins-workspace/issues/3062
