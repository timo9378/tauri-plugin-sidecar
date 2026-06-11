# demo-app

A minimal real Tauri v2 app that exercises the plugin end to end — no Node
toolchain needed, the UI is a static HTML page.

```sh
cd examples/demo-app/src-tauri
cargo run
```

What it demonstrates:

- **`api`** — a tiny HTTP server (`python3` on Unix, PowerShell `HttpListener`
  on Windows) that receives a **dynamic port** and a **session token** via env
  vars, health-gated on TCP accept.
- **`tree`** — depends on `api` (starts only after it is healthy) and
  deliberately **spawns child processes of its own** (`sleep` children on
  Unix, `ping -t` children on Windows).

Things to try:

1. Watch the fleet table: `api` goes `starting → healthy`, then `tree` leaves
   `waiting_for_deps`.
2. Click **stop** on `tree`, then check your process list (Task Manager on
   Windows): the parent *and all its children* are gone. On Windows this is
   the Job Object kill path — no `ping.exe` may survive.
3. Click **restart** on `api` — `tree` keeps running; only the target is
   recycled.
4. Close the window: the whole fleet shuts down in reverse dependency order.
   Re-run the app and check the log pane — orphan cleanup reports any pid it
   had to reap from a previous unclean exit.

> `cargo run` / `tauri dev` is enough here; `tauri build` would additionally
> require bundle icons, which this example intentionally omits.
