// Generated from the checked-in Rust-derived JSON Schemas. Do not edit.

/**
 * `daemon.ping` result. Contract names remain strings so older clients can
 * preserve and ignore names introduced by newer daemon builds.
 */
export interface DaemonPingSerialize {
  backend: string;
  contracts: string[];
  instance: string | null;
  protocol: number;
  revision: number;
  tasks: TaskLaneStatus;
}
/**
 * Status of the daemon's volatile task-publication lane.
 */
export interface TaskLaneStatus {
  available: boolean;
  error: string | null;
}
/**
 * The agent's own process facts. Self-authored by any Pi session, with no owner
 * or assignment, and kept separate from execution and assignment. Every field
 * is optional and a published snapshot replaces the whole value, so omission
 * withdraws a fact. `context_window` is counts; a display derives the percentage.
 */
export interface FactsSnapshotDeserialize {
  background_started_unix_ms?: number | null;
  background_tasks?: BackgroundTask[];
  context_window?: ContextWindow | null;
  model?: string | null;
  provider?: string | null;
  session_name?: string | null;
  thinking?: string | null;
}
/**
 * One background task the agent process reports as unresolved.
 */
export interface BackgroundTask {
  id: string;
  phase: string;
}
/**
 * How much of a model's context window a session has used, as counts.
 *
 * `window` is positive and `tokens` does not exceed it; the percentage a display
 * shows is derived from these and never published. Wire counts stay exact in JS.
 */
export interface ContextWindow {
  tokens: number;
  window: number;
}
/**
 * What a spawn topology read can say about a child's process exit.
 */
export type SpawnExitObservationSerialize =
  | {
      state: "unknown";
    }
  | {
      code: number | null;
      observed_at: string;
      signal: number | null;
      state: "exited";
    };
