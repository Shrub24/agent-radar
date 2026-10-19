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
 * Inputs for publishing an immutable handoff. The wire request ID carries the
 * stable communication identity; the daemon derives the sender execution.
 */
export interface HandoffPublishParamsDeserialize {
  content: string;
  correlation_id?: string | null;
  recipient_subject_id: string;
  sender_agent_id: string;
  summary: string;
}
/**
 * A bounded inbox page by stable handoff ID. Payloads are fetched one at a
 * time with `communication.get`, keeping each response within the wire bound.
 */
export interface HandoffInboxSerialize {
  handoff_ids: string[];
}
/**
 * Immutable handoff content. The request id is the stable communication id.
 */
export interface HandoffSerialize {
  content: string;
  correlation_id?: string | null;
  id: string;
  published_at_unix_ms: number;
  recipient_subject_id: string;
  sender: HandoffIdentity;
  summary: string;
  version: number;
}
/**
 * The current managed Pi execution that published or received a handoff.
 * `subject_id` is stable across verified successors; `execution_id` and
 * generation identify the exact execution that took the action.
 */
export interface HandoffIdentity {
  agent_id: string;
  execution_generation: number;
  execution_id: string;
  session_id: string;
  subject_id: string;
}
/**
 * Durable recipient-consumer receipt, stored separately from immutable content.
 */
export interface HandoffReceiptSerialize {
  handoff_id: string;
  received_at_unix_ms: number;
  recipient: HandoffIdentity;
  version: number;
}
/**
 * The current managed Pi execution that published or received a handoff.
 * `subject_id` is stable across verified successors; `execution_id` and
 * generation identify the exact execution that took the action.
 */
export interface HandoffIdentity {
  agent_id: string;
  execution_generation: number;
  execution_id: string;
  session_id: string;
  subject_id: string;
}
/**
 * Managed owner/parent metadata accepted with a spawn intent. Domain validation
 * and parent-binding checks remain in the daemon handler.
 */
export interface ManagedLaunchMetadataDeserialize {
  label?: string | null;
  owner?: string | null;
  parent_subject_id?: string | null;
  role?: string | null;
  run?: string | null;
}
/**
 * A non-secret Pi-session breadcrumb issued by Radar. This is correlation
 * evidence only and cannot establish lifecycle authority.
 */
export interface LaunchIdentityMarkerSerialize {
  execution_generation: number;
  launch_id: string;
  subject_id: string;
  version: number;
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
