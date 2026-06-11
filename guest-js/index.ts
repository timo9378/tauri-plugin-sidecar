/**
 * TypeScript bindings for tauri-plugin-sidecar.
 *
 * ```ts
 * import { status, restart, onStateChange } from "tauri-plugin-sidecar-api";
 *
 * const sidecars = await status();
 * await restart("backend");
 * const unlisten = await onStateChange(({ name, state }) => {
 *   console.log(name, state);
 * });
 * ```
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Lifecycle state of a sidecar, mirroring the Rust `SidecarState`. */
export type SidecarState =
  | { state: "idle" }
  | { state: "waiting_for_deps" }
  | { state: "starting" }
  | { state: "healthy" }
  | { state: "backoff"; attempt: number; delay_secs: number }
  | { state: "stopping" }
  | { state: "stopped" }
  | { state: "failed"; reason: string };

/** One sidecar's status as returned by {@link status}. */
export interface SidecarStatus {
  name: string;
  state: SidecarState;
  /** The allocated port, when a port strategy is configured. */
  port: number | null;
}

/** Payload of the `sidecar://state` event. */
export interface StateChangeEvent {
  name: string;
  state: SidecarState;
}

/** Payload of the `sidecar://log` event (sidecars with `emit_logs(true)`). */
export interface LogEvent {
  sidecar: string;
  stream: "stdout" | "stderr";
  line: string;
}

/** Returns the status of one sidecar, or all sidecars when `name` is omitted. */
export async function status(name?: string): Promise<SidecarStatus[]> {
  return invoke("plugin:sidecar|status", { name });
}

/** Starts a sidecar (and its dependencies, in order). No-op if already running. */
export async function start(name: string): Promise<void> {
  return invoke("plugin:sidecar|start", { name });
}

/** Stops a sidecar (graceful step, then whole-tree kill). Never auto-restarts. */
export async function stop(name: string): Promise<void> {
  return invoke("plugin:sidecar|stop", { name });
}

/** Stops then starts a sidecar with a fresh backoff budget. */
export async function restart(name: string): Promise<void> {
  return invoke("plugin:sidecar|restart", { name });
}

/** Tails the last `lines` (default 100) captured output lines of a sidecar. */
export async function logs(name: string, lines?: number): Promise<string[]> {
  return invoke("plugin:sidecar|logs", { name, lines });
}

/** Subscribes to lifecycle state changes for every sidecar. */
export async function onStateChange(
  handler: (event: StateChangeEvent) => void,
): Promise<UnlistenFn> {
  return listen<StateChangeEvent>("sidecar://state", (e) => handler(e.payload));
}

/** Subscribes to captured log lines (sidecars with `emit_logs(true)`). */
export async function onLog(handler: (event: LogEvent) => void): Promise<UnlistenFn> {
  return listen<LogEvent>("sidecar://log", (e) => handler(e.payload));
}
