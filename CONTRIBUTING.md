# Contributing

Thanks for considering a contribution. This plugin aims to be the lifecycle
layer Tauri sidecars are missing ([tauri-apps/plugins-workspace#3062]), so the
bar for reliability and clarity is deliberately high.

## Project shape

```
crates/
  sidecar-core/        Tauri-agnostic supervision engine
    src/domain/        pure types & algorithms — no IO, fully unit-testable
    src/platform/      OS primitives (process tree, ports)
    src/infra/         IO at the edges (health probes, pid persistence)
    src/runtime/       async orchestration (supervisor task, manager)
    tests/             real-process integration tests
  tauri-plugin-sidecar/  the Tauri v2 plugin layer
```

One-way dependency rule: `runtime → domain + platform + infra`, and
**`domain` never imports `platform` / `infra` / `tauri`**. If `domain` needs
to interact with the outside world, define a trait there and inject the
implementation from a higher layer. This keeps the core unit-testable without
a Tauri runtime or a live process.

## Before you push

CI enforces all of this; running it locally first saves a round-trip:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Cross-platform matters here — the integration tests run real processes. If you
touch `platform/process.rs` (kill-tree, Job Objects), please note in the PR
whether you verified on Windows, Linux, or both. CI covers all three OSes.

## Lint policy

- **Denied (won't merge):** `todo!`/`unimplemented!`, `dbg!`, `println!`/
  `eprintln!` (use `tracing`), undocumented `unsafe` blocks, `Arc<T>` around
  non-`Send + Sync` types.
- **clippy::pedantic** runs at warn; CI's `-D warnings` makes it binding. A
  short "too opinionated" allow list lives in the workspace `Cargo.toml`,
  each entry with a one-line reason. Adding a new allow needs a reason in the
  same style — "the AI tool wrote it wrong" is a fix, not an allow.
- New shared state uses `parking_lot` mutexes; never hold a lock across an
  `await`.
- `unwrap`/`expect` are fine in tests, avoided on production paths.

## Commits

Conventional Commits (`feat:`, `fix:`, `chore:`, `docs:`, …). Keep the subject
under ~72 chars; put the "why" in the body.

## Scope

This plugin stays focused on **sidecar lifecycle**. Bundling/installer
concerns (large-runtime download, antivirus-aware extraction) are a planned
separate layer — discuss in an issue before building. Things explicitly out of
scope: being a process manager for non-sidecar processes, or a general task
runner.

[tauri-apps/plugins-workspace#3062]: https://github.com/tauri-apps/plugins-workspace/issues/3062
