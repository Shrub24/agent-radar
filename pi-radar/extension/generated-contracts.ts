// Generated from the checked-in Rust-derived JSON Schemas. Do not edit.

export interface PresenceReportDeserialize {
  claim_id: string;
  incarnation: string;
  label?: string | null;
  location?: PresenceReportRegistryLocation | null;
  owner?: string | null;
  process: PresenceReportProcessIdentity;
  registration_id?: string | null;
  run?: string | null;
  session_id: string;
  source: string;
  subject_id?: string | null;
}
/**
 * Where an agent's incarnation lives on one backend, in that backend's own
 * coordinates.
 *
 * The backend name is free-form so a source Radar does not ship yet is
 * preserved rather than rejected; the rest are the coordinates as reported, and
 * each is optional because a backend need not name every level.
 */
export interface PresenceReportRegistryLocation {
  backend: string;
  instance?: string | null;
  pane?: string | null;
  tab?: string | null;
  workspace?: string | null;
}
/**
 * Which process incarnation a sample belongs to.
 *
 * The kernel reuses process ids, so a pid does not name a process: one
 * incarnation is the boot it started in, its pid, and when it started. Those
 * are what make two readings comparable — counters may only be subtracted
 * from an earlier reading of the same identity, and a difference taken across
 * two of them measures neither process.
 */
export interface PresenceReportProcessIdentity {
  /**
   * The boot this process started in, as the kernel reports it.
   */
  boot_id: string;
  /**
   * The process id, unique within one boot and one incarnation of it.
   */
  pid: number;
  /**
   * Start time since boot, in clock ticks (`starttime` in
   * `/proc/<pid>/stat`): the kernel's stamp for this incarnation.
   */
  start_ticks: number;
  [k: string]: unknown;
}
export type PresenceAnswerStanding = "reserved" | "pending" | "granted" | "duplicate" | "released";

/**
 * The small public receipt; session/location context stays inside Radar.
 */
export interface PresenceAnswerSerialize {
  claim_id: string;
  registration_id: string;
  standing: PresenceAnswerStanding;
  subject_id?: string | null;
}
export interface PresenceReleaseDeserialize {
  claim_id: string;
  process: PresenceReleaseProcessIdentity;
  registration_id: string;
}
/**
 * Which process incarnation a sample belongs to.
 *
 * The kernel reuses process ids, so a pid does not name a process: one
 * incarnation is the boot it started in, its pid, and when it started. Those
 * are what make two readings comparable — counters may only be subtracted
 * from an earlier reading of the same identity, and a difference taken across
 * two of them measures neither process.
 */
export interface PresenceReleaseProcessIdentity {
  /**
   * The boot this process started in, as the kernel reports it.
   */
  boot_id: string;
  /**
   * The process id, unique within one boot and one incarnation of it.
   */
  pid: number;
  /**
   * Start time since boot, in clock ticks (`starttime` in
   * `/proc/<pid>/stat`): the kernel's stamp for this incarnation.
   */
  start_ticks: number;
  [k: string]: unknown;
}
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
  sender_registration_id: string;
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
  claim_id: string;
  registration_id: string;
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
  claim_id: string;
  registration_id: string;
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
/**
 * Input accepted by `observation.get`.
 */
export interface ObservationGetParamsDeserialize {
  classes?: string[] | null;
  epoch?: string | null;
  since_revision?: number | null;
  wait_ms?: number | null;
}
/**
 * The result forms returned by `observation.get`.
 */
export type ObservationGetResponseSerialize =
  ObservationSnapshotResult | ObservationNotModifiedResult | ObservationExpiredResult | ObservationTooLargeResult;
/**
 * Why a retained association is reported as not current.
 */
export type RetainedBasis = "shell_foreground" | "unverified";
/**
 * Herdsman's projection of an agent's state, as an owner publishes it.
 *
 * This is a control projection, not a lifecycle: `Working` says a model turn
 * or tool call is in flight, `Blocked` says the assignment waits on its owner,
 * and `Waiting` says the turn has yielded while work it depends on is
 * unresolved. Values are additive, so an unrecognised one is preserved.
 */
export type SemanticState =
  | ("idle" | "working" | "waiting" | "blocked" | "settling" | "unknown" | "lost")
  | {
      other: string;
    };
/**
 * A validated canonical UUID (lowercase `8-4-4-4-12` hex).
 *
 * Only explicitly supplied UUID metadata is ever turned into this type —
 * session file paths are never scanned for embedded UUIDs.
 */
export type SessionUuid = string;
/**
 * Independent publisher channels.
 */
export type Channel = "execution" | "assignment" | "facts";
/**
 * Freshness is independent of process identity or completion.
 */
export type Freshness = "fresh" | "stale";
/**
 * One complete accepted snapshot, typed by the channel that carries it. Each
 * variant is strict about its own fields, so a field from another channel is
 * refused rather than silently dropped.
 */
export type Report = Snapshot | AssignmentSnapshot | FactsSnapshot;
export type Standing = "reserved" | "pending" | "granted" | "duplicate" | "released";
/**
 * How a registration relates to the live panes.
 */
export type Binding =
  | "joined"
  | {
      unjoined: Unjoined;
    };
/**
 * Why a registration did not join a pane.
 */
export type Unjoined =
  | "no_location"
  | "no_claim"
  | "pane_absent"
  | "location_mismatch"
  | "foreground_unreadable"
  | "identity_mismatch"
  | "ambiguous";
export type State = "pending" | "captured" | "bound" | "revoked";
/**
 * The state of one edge in the recorded topology: the strongest statement a
 * read can make about it, and nothing beyond the records and this read's own
 * evidence.
 */
export type SpawnState = "unbound" | "bound" | "unresolved";
/**
 * Reported session identity for an agent observation.
 *
 * Identity is separate from [`Location`]: the same session may move between
 * panes, and a pane may host different sessions over time.
 */
export type SessionIdentity =
  | {
      kind: "uuid";
      value: SessionUuid;
    }
  | {
      kind: "reported";
      value: {
        source: string | null;
        value: string;
      };
    };
/**
 * Runtime lifecycle exactly as the source reports it.
 *
 * This is not a semantic control state: `Idle` says nothing about
 * availability for new work, result delivery or safe lifecycle actions.
 * Unrecognized values are preserved verbatim in [`RuntimeStatus::Other`] so a
 * source change never fails an otherwise valid inventory.
 */
export type RuntimeStatus =
  | ("idle" | "working" | "done" | "unknown")
  | {
      other: string;
    };
/**
 * Evidence about a pane's foreground, decoded inside the connector.
 *
 * The reconciler treats only [`ForegroundEvidence::NonShell`] as positive
 * supersession evidence; absence or ambiguity retains the last observation.
 */
export type ForegroundEvidence =
  | {
      kind: "shell";
    }
  | {
      /**
       * The leader's command line as the operating system reports it.
       */
      command: string | null;
      kind: "non_shell";
      local: LocalFacts;
      name: string | null;
      pid: number;
    }
  | {
      kind: "inconclusive";
    };
/**
 * How a foreground process's running executable compares with the installed
 * program it would run.
 */
export type BinaryFreshness = "current" | "stale" | "unknown";
/**
 * Why a running executable could not be compared, when it could not.
 *
 * Each variant is something a reader can act on differently: a program that is
 * not installed, an installed entrypoint whose payload cannot be identified,
 * an executable that cannot be read, or a running file that is not the program
 * in question. The words a row uses are the row's, not this type's.
 */
export type BinaryUnknown = "no_counterpart" | "unsupported_launcher" | "unreadable" | "not_compared";
/**
 * Interval CPU use of one process, as a percentage of one CPU.
 *
 * Held in hundredths of a percent, as an integer: the value is a ratio of two
 * kernel counter readings, and an exact integer is what a row can show and a
 * test can assert. `12.5%` of one CPU is `1_250`. It measures the process
 * rather than the machine, so a process that used two CPUs for a whole
 * interval is `20_000`, not `100%`.
 */
export type CpuPercent = number;
/**
 * What a container is in the hierarchy.
 */
export type Kind = "workspace" | "tab" | "pane";
/**
 * Who made the container.
 */
export type Provenance = "created" | "observed";
/**
 * Source freshness included in an observation response.
 */
export type ObservationFreshness =
  | ("pending" | "current")
  | {
      stale: {
        diagnostic: string;
      };
    }
  | {
      unavailable: {
        diagnostic: string;
      };
    };

export interface ObservationSnapshotResult {
  epoch: string;
  revision: number;
  snapshot: ObservationSnapshot;
}
/**
 * One immutable observation revision on the public wire.
 */
export interface ObservationSnapshot {
  collected_at: string | null;
  epoch: string;
  observation: ObservationProjection;
  revision: number;
  sources: ObservationSource[];
  tasks: unknown;
}
/**
 * Public observation projection. The sampler's mutable state remains private;
 * this DTO names the fields the daemon publishes without serializing that state
 * type directly.
 */
export interface ObservationProjection {
  awaiting_evidence: string[];
  containment: {
    [k: string]: RetainedObservation;
  };
  foreground: {
    [k: string]: ForegroundEvidence;
  };
  inventory: FleetObservation | null;
  /**
   * Source freshness included in an observation response.
   */
  registry:
    | ("pending" | "current")
    | {
        stale: {
          diagnostic: string;
        };
      }
    | {
        unavailable: {
          diagnostic: string;
        };
      };
  retained: {
    [k: string]: RetainedObservation;
  };
  source: ObservationFreshness;
}
/**
 * Retained agent facts carried across an idle-shell gap.
 */
export interface RetainedObservation {
  basis: RetainedBasis;
  observation: AgentObservation;
}
/**
 * A normalized observation of one agent, keyed in the inventory by its pane.
 */
export interface AgentObservation {
  facts: HerdsmanFacts;
  /**
   * Reported display label (the stripped terminal title), if any.
   */
  label: string | null;
  /**
   * Explicit ownership lineage; `None` unless valid UUID tokens were supplied.
   */
  lineage: Lineage | null;
  location: Location;
  /**
   * Reported agent name (for example `"pi"`), if any.
   */
  name: string | null;
  /**
   * The daemon's registration for this agent, with how it binds to a pane.
   * `None` for an agent the registry does not know, and for every agent when
   * no registry is read. A registration that names a pane protects that pane
   * even when unjoined.
   */
  registry: RegistryView | null;
  /**
   * Reported session identity; `None` when the source reports none.
   */
  session: SessionIdentity | null;
  /**
   * Runtime lifecycle as reported; never a semantic control state.
   */
  status: RuntimeStatus | null;
}
/**
 * What Herdsman publishes about this agent. Empty unless the source
 * published metadata; never inferred from runtime facts.
 */
export interface HerdsmanFacts {
  /**
   * A lead's pending owner question.
   */
  ask: string | null;
  /**
   * How long the active assignment had been running when it was observed.
   * Measured at observation time from Herdsman's own start, because a
   * presentation that had to interpret a timestamp would need a clock.
   */
  assigned_for: number | null;
  /**
   * The active assignment's start, Unix milliseconds as published.
   */
  assigned_started_unix_ms: number | null;
  /**
   * The active assignment's display text.
   */
  assignment: string | null;
  /**
   * What this pane is waiting on: `agent:<label>` for each outstanding
   * child, and `owner` while its own question to its owner is outstanding.
   * Outstanding work, not state: it is published while working and while
   * stopped, and cleared when the last item resolves.
   */
  awaited: string[];
  /**
   * How many of the pane's background tasks are running, as its own
   * extension counts them. Another publisher owns this key, and the count
   * speaks only for the running ones: it is not the set of unresolved work.
   */
  background_running: number | null;
  /**
   * The oldest outstanding background task's start, ISO 8601 as published.
   */
  background_started: string | null;
  /**
   * The pane's unresolved background tasks as its own extension publishes
   * them, each an `<id>:<phase>` entry. The awaited facts are the union of
   * the owner's awaited items and these ids, because neither publisher can
   * express the whole pane alone.
   */
  background_tasks: string[];
  /**
   * The percentage a background-task token states, percent sign included. It carries no
   * counts, so it is never presented as a typed window.
   */
  context_usage: string | null;
  /**
   * The context window a registry publisher reports as counts.
   */
  context_window: ContextWindow | null;
  /**
   * The agent definition a pane reports as its display agent.
   */
  definition: string | null;
  /**
   * A managed worker's runtime label: the name its owner and its siblings
   * use for it, and the only faithful source for a worker's name. The backend's
   * own agent name truncates it and its title welds it to the task.
   */
  label: string | null;
  /**
   * The owner's last outcome for the assignment, as it published it.
   */
  last_outcome: string | null;
  /**
   * Whether this pane's tokens carried any `pi_herdsman_*` key.
   *
   * Presence, not readability: the owner's metadata is what makes a Pi pane
   * managed, so a key whose value this build cannot read still counts. The
   * decoder sets this at the adapter boundary, where the raw token map is,
   * because the named fields below discard keys this version does not know.
   */
  managed_metadata: boolean;
  /**
   * The session's model identity as published, provider prefix included.
   */
  model: string | null;
  /**
   * A lead's published session name.
   */
  name: string | null;
  provider: string | null;
  /**
   * The active assignment's request identity.
   */
  request: string | null;
  /**
   * The role the agent runs as: a lead role, or a worker's agent definition.
   */
  role: string | null;
  /**
   * A managed worker's run identity.
   */
  run: string | null;
  /**
   * The source's human session name.
   */
  session_name: string | null;
  /**
   * The owner's projection of this agent's state, while it is published.
   */
  state: SemanticState | null;
  thinking: string | null;
  /**
   * Why the owner says the assignment is waiting, as it published it.
   */
  waiting_reason: string | null;
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
 * Explicit ownership lineage from `pi_herdsman_session` /
 * `pi_herdsman_parent_session` UUID metadata.
 *
 * Used only for ownership joins (worker → owner). Lineage is never used as
 * session identity, and absent or malformed tokens yield no lineage at all.
 */
export interface Lineage {
  parent: SessionUuid | null;
  session: SessionUuid;
}
/**
 * Backend-scoped runtime location: workspace, tab and pane identifiers.
 *
 * Pane identity is a runtime location detail; it is never an agent identity.
 */
export interface Location {
  pane_id: string;
  tab_id: string;
  workspace_id: string;
}
/**
 * A registered agent with its binding, carried on the agent it projects to.
 */
export interface RegistryView {
  agent: RegistryAgent;
  binding: Binding;
  managed: ManagedSubject | null;
  spawn: SpawnBinding | null;
}
/**
 * One registered agent as the daemon's `agent.list` serves it.
 */
export interface RegistryAgent {
  assignment: PublicChannelFacts | null;
  /**
   * Durable claim decision, if this registration has claimed a session.
   */
  claim_standing?: Standing | null;
  /**
   * Mutable current-session context, kept separate from registration identity.
   */
  context: PublicContext | null;
  execution: PublicChannelFacts | null;
  facts: PublicChannelFacts | null;
  process: ProcessEvidence;
  registration: PublicRegistration;
  /**
   * Whether this process runs inside a Radar-established execution scope. A
   * process started outside the managed launch path has none and is observed.
   */
  scoped?: boolean | null;
}
/**
 * A channel read for a reader that is not its writer: the same identity,
 * freshness and report facts as [`PublicChannel`], with the fencing handle
 * dropped because it is a credential rather than a fact.
 */
export interface PublicChannelFacts {
  channel: Channel;
  registration_id: string;
  snapshot?: PublicSnapshotFacts | null;
  version: number;
  writer: PublicWriterFacts;
}
/**
 * The last accepted report as an unrelated reader may see it, leaving out the
 * handle that fences further writes.
 */
export interface PublicSnapshotFacts {
  expires_at: string;
  freshness: Freshness;
  generation: number;
  incarnation: string;
  lease_ms: number;
  observed_at?: string | null;
  received_at: string;
  reporting_owner?: string | null;
  restored: boolean;
  sequence: number;
  snapshot: Report;
  source: string;
}
/**
 * One complete snapshot for one channel. Vocabulary is free-form; shape is
 * strict. Unknown state/reason/action words survive, unknown fields do not.
 */
export interface Snapshot {
  actions?: string[];
  activity: string;
  last_outcome?: Outcome | null;
  waiting_reason?: string | null;
}
/**
 * A turn outcome kept distinct from current activity.
 */
export interface Outcome {
  detail?: string | null;
  /**
   * Producer vocabulary is deliberately free-form.
   */
  result: string;
}
/**
 * The owner's complete assignment snapshot: the shared base plus facts only an
 * owner knows. It is authored by the owner's writer and is never derived from
 * execution activity; the owner's identity is provenance, not authority.
 */
export interface AssignmentSnapshot {
  actions?: string[];
  activity: string;
  ask?: string | null;
  assigned_started_unix_ms?: number | null;
  assignment?: string | null;
  awaited?: string[];
  definition?: string | null;
  last_outcome?: Outcome | null;
  request?: string | null;
  role?: string | null;
  run?: string | null;
  waiting_reason?: string | null;
}
/**
 * The agent's own process facts. Self-authored by any Pi session, with no owner
 * or assignment, and kept separate from execution and assignment. Every field
 * is optional and a published snapshot replaces the whole value, so omission
 * withdraws a fact. `context_window` is counts; a display derives the percentage.
 */
export interface FactsSnapshot {
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
 * The current writer as an unrelated reader may see it: provenance and
 * generation, without the opaque handle an update must present.
 */
export interface PublicWriterFacts {
  generation: number;
  incarnation: string;
  reporting_owner?: string | null;
  retired_at?: string | null;
  sequence: number;
  source: string;
}
/**
 * One agent record's context as a client reads it. A `None` read of this whole
 * record means never published; a present record with a null session means no
 * current session.
 */
export interface PublicContext {
  context: PublicSession;
  registration_id: string;
  version: number;
  writer: PublicWriterFacts;
}
/**
 * The last accepted context as a client reads it.
 *
 * There is no handle and no serving epoch here by construction, and no other
 * field a private path or argument could be forgotten into.
 */
export interface PublicSession {
  expires_at: string;
  freshness: Freshness;
  generation: number;
  incarnation: string;
  lease_ms: number;
  observed_at?: string | null;
  received_at: string;
  reporting_owner?: string | null;
  restored: boolean;
  sequence: number;
  /**
   * The current session, or an explicit null for "no current session".
   *
   * Written even when null: a present record is what distinguishes "no
   * current session" from "never published", which is no context record at
   * all.
   */
  session: string | null;
  source: string;
}
/**
 * The process evidence `agent.list` serves beside a registration. Only the
 * claimed birth identity is read here; the registry's own verdict is not used
 * to join, because a join needs the claim to match the live foreground.
 */
export interface ProcessEvidence {
  claim: ProcessIdentity | null;
}
/**
 * Which process incarnation a sample belongs to.
 *
 * The kernel reuses process ids, so a pid does not name a process: one
 * incarnation is the boot it started in, its pid, and when it started. Those
 * are what make two readings comparable — counters may only be subtracted
 * from an earlier reading of the same identity, and a difference taken across
 * two of them measures neither process.
 */
export interface ProcessIdentity {
  /**
   * The boot this process started in, as the kernel reports it.
   */
  boot_id: string;
  /**
   * The process id, unique within one boot and one incarnation of it.
   */
  pid: number;
  /**
   * Start time since boot, in clock ticks (`starttime` in
   * `/proc/<pid>/stat`): the kernel's stamp for this incarnation.
   */
  start_ticks: number;
}
/**
 * A registration as a client reads it. The private launch fields are absent by
 * construction, not by redaction: there is no field here to forget to strip.
 */
export interface PublicRegistration {
  incarnation: string;
  label?: string | null;
  launch: PublicLaunch;
  location?: RegistryLocation | null;
  owner?: string | null;
  /**
   * Whether a process identity was supplied. Its verification is a separate
   * answer and is never asserted by this record.
   */
  process_claimed: boolean;
  registered_at: string;
  registration_id: string;
  run?: string | null;
  session?: string | null;
  source: string;
  version: number;
}
/**
 * What a public read says about private launch information: that something is
 * held, and which revision of it.
 */
export interface PublicLaunch {
  available: boolean;
  revision?: string | null;
}
/**
 * Where an agent's incarnation lives on one backend, in that backend's own
 * coordinates.
 *
 * The backend name is free-form so a source Radar does not ship yet is
 * preserved rather than rejected; the rest are the coordinates as reported, and
 * each is optional because a backend need not name every level.
 */
export interface RegistryLocation {
  backend: string;
  instance?: string | null;
  pane?: string | null;
  tab?: string | null;
  workspace?: string | null;
}
/**
 * A daemon-owned process lifecycle target, joined to one registration by exact
 * agent id and process birth identity.
 */
export interface ManagedSubject {
  child_close: ChildCloseTarget | null;
  claim_id: string;
  metadata: ManagedMetadata;
  process: ProcessIdentity;
  session: string | null;
  state: State;
  subject_id: string;
}
/**
 * Exact spawn-edge child identity required by `spawn.close`.
 */
export interface ChildCloseTarget {
  incarnation: string;
  source: string;
  spawn_request_id: string;
}
/**
 * One durable launch subject. The correlation is a private credential and is
 * spent at capture; no public read returns it.
 */
export interface ManagedMetadata {
  label?: string | null;
  owner?: string | null;
  parent_subject_id?: string | null;
  role?: string | null;
  run?: string | null;
}
/**
 * The spawn edge that binds this registration as a child, when one exists.
 *
 * Parentage and the edge's own state come from the topology read. The edge's
 * state is never a process-exit claim: an unresolved child is shown as such.
 */
export interface SpawnBinding {
  child_incarnation: string | null;
  child_source: string | null;
  claim_id: string | null;
  parent: string;
  request_id: string;
  state: SpawnState;
}
/**
 * What this machine knows about that process. Filled by the collector
 * from the operating system, never by the runtime's own report.
 */
export interface LocalFacts {
  binary: BinaryIdentity;
  /**
   * What the process is using, from the sample taken for it. `None` when no
   * sample was taken: this machine has no reader for that process, the
   * process could not be read, or the platform cannot sample at all. A
   * reading from an earlier refresh is never carried here.
   */
  resources: ProcessResources | null;
  /**
   * How long the process has been alive, measured from its own start time
   * rather than from when Radar first saw it.
   */
  running_for: number | null;
  /**
   * What it has done to the terminal.
   */
  terminal: "full_screen" | "line" | "unknown";
}
/**
 * How the foreground program's executable compares with the installed
 * program of its own name or package family.
 */
export interface BinaryIdentity {
  /**
   * The running executable's own path, when it could be read.
   */
  executable: string | null;
  freshness: BinaryFreshness;
  /**
   * The installation the installed counterpart's payload belongs to. `None`
   * when no counterpart was found or its payload could not be identified.
   */
  installed: string | null;
  /**
   * The installation the running executable belongs to. `None` when its
   * executable could not be read.
   */
  running: string | null;
  /**
   * Why nothing was compared, when nothing was.
   */
  unknown: BinaryUnknown | null;
}
/**
 * What one process is using, as the collector sampled it.
 *
 * These are the resources of that process and of nothing larger: a build
 * running beneath a pane is a descendant and is summed separately in
 * [`Self::descendants`], and nothing here says who owns the process, what work
 * it serves, or whether it is progressing. Every field comes from one read of
 * the process, so a sample never mixes two incarnations; RSS is that process's
 * resident set, in which pages shared with another process are counted here as
 * well.
 */
export interface ProcessResources {
  /**
   * Interval CPU since the previous sample of this same identity. `None`
   * until such a sample exists, and whenever the interval cannot be
   * measured.
   */
  cpu: CpuPercent | null;
  descendants: DescendantResources;
  identity: ProcessIdentity3;
  /**
   * Resident set size in bytes. `None` when this machine cannot convert the
   * kernel's page count.
   */
  rss_bytes: number | null;
  /**
   * The kernel's scheduler state when the sample was taken.
   */
  state:
    | {
        kind: "running";
      }
    | {
        kind: "sleeping";
      }
    | {
        kind: "disk_sleep";
      }
    | {
        kind: "stopped";
      }
    | {
        kind: "tracing_stop";
      }
    | {
        kind: "zombie";
      }
    | {
        kind: "dead";
      }
    | {
        kind: "idle";
      }
    | {
        kind: "other";
        value: string;
      };
}
/**
 * What the processes beneath this one are using, as the refresh's scan
 * observed them. Never merged with the fields above: a build's work is not
 * its launcher's.
 */
export interface DescendantResources {
  /**
   * Their interval CPU. Every one of them needs its own matching pair of
   * readings, so a descendant seen for the first time contributes nothing
   * and leaves this partial.
   */
  cpu:
    | {
        kind: "complete";
        value: CpuPercent;
      }
    | {
        kind: "partial";
        /**
         * @minItems 2
         * @maxItems 2
         */
        value: [unknown, unknown];
      }
    | {
        kind: "unknown";
        value: string;
      };
  /**
   * Those processes: one sample each, and exactly the ones `observed` counts
   * and the totals cover. Empty when none could be enumerated, and the
   * totals below say why.
   */
  members: DescendantSample[];
  /**
   * How many processes the scan observed beneath the root. `None` when none
   * could be enumerated at all.
   */
  observed: number | null;
  /**
   * The resident set of those of them whose size could be read.
   */
  rss_bytes:
    | {
        kind: "complete";
        value: number;
      }
    | {
        kind: "partial";
        /**
         * @minItems 2
         * @maxItems 2
         */
        value: [unknown, unknown];
      }
    | {
        kind: "unknown";
        value: string;
      };
}
/**
 * One process beneath a root, as one refresh's scan confirmed it.
 *
 * These are the members the totals of [`DescendantResources`] are made of, from
 * the same snapshot, the same reads and the same confirmation — never a second
 * walk and never a second reading of `/proc`. The parent is the identity of the
 * process this one was read beneath: what a table can order and fold by is the
 * incarnation, since a pid alone names a different process once the kernel
 * reuses it. The name is the kernel's own short name for the process, which is
 * neither the arguments it was started with nor a claim about what started it.
 */
export interface DescendantSample {
  /**
   * Interval CPU since the previous sample of this same identity. `None` when
   * no interval can be measured; a measured idle interval is `Some(0)`.
   */
  cpu: CpuPercent | null;
  identity: ProcessIdentity1;
  /**
   * The kernel's name for the process (`comm` in `/proc/<pid>/stat`), which
   * is not argv and says nothing about who started it.
   */
  name: string;
  parent: ProcessIdentity2;
  /**
   * Resident set size in bytes. `None` when this machine cannot convert the
   * kernel's page count.
   */
  rss_bytes: number | null;
  /**
   * The scheduler state the kernel reported for it.
   */
  state:
    | {
        kind: "running";
      }
    | {
        kind: "sleeping";
      }
    | {
        kind: "disk_sleep";
      }
    | {
        kind: "stopped";
      }
    | {
        kind: "tracing_stop";
      }
    | {
        kind: "zombie";
      }
    | {
        kind: "dead";
      }
    | {
        kind: "idle";
      }
    | {
        kind: "other";
        value: string;
      };
}
/**
 * Which process incarnation a sample belongs to.
 *
 * The kernel reuses process ids, so a pid does not name a process: one
 * incarnation is the boot it started in, its pid, and when it started. Those
 * are what make two readings comparable — counters may only be subtracted
 * from an earlier reading of the same identity, and a difference taken across
 * two of them measures neither process.
 */
export interface ProcessIdentity1 {
  /**
   * The boot this process started in, as the kernel reports it.
   */
  boot_id: string;
  /**
   * The process id, unique within one boot and one incarnation of it.
   */
  pid: number;
  /**
   * Start time since boot, in clock ticks (`starttime` in
   * `/proc/<pid>/stat`): the kernel's stamp for this incarnation.
   */
  start_ticks: number;
}
/**
 * Which process incarnation a sample belongs to.
 *
 * The kernel reuses process ids, so a pid does not name a process: one
 * incarnation is the boot it started in, its pid, and when it started. Those
 * are what make two readings comparable — counters may only be subtracted
 * from an earlier reading of the same identity, and a difference taken across
 * two of them measures neither process.
 */
export interface ProcessIdentity2 {
  /**
   * The boot this process started in, as the kernel reports it.
   */
  boot_id: string;
  /**
   * The process id, unique within one boot and one incarnation of it.
   */
  pid: number;
  /**
   * Start time since boot, in clock ticks (`starttime` in
   * `/proc/<pid>/stat`): the kernel's stamp for this incarnation.
   */
  start_ticks: number;
}
/**
 * Which process incarnation a sample belongs to.
 *
 * The kernel reuses process ids, so a pid does not name a process: one
 * incarnation is the boot it started in, its pid, and when it started. Those
 * are what make two readings comparable — counters may only be subtracted
 * from an earlier reading of the same identity, and a difference taken across
 * two of them measures neither process.
 */
export interface ProcessIdentity3 {
  /**
   * The boot this process started in, as the kernel reports it.
   */
  boot_id: string;
  /**
   * The process id, unique within one boot and one incarnation of it.
   */
  pid: number;
  /**
   * Start time since boot, in clock ticks (`starttime` in
   * `/proc/<pid>/stat`): the kernel's stamp for this incarnation.
   */
  start_ticks: number;
}
/**
 * One successful inventory: workspaces, tabs, panes, reported agents and
 * daemon lifecycle subjects not represented by a joined agent row.
 */
export interface FleetObservation {
  /**
   * The agents this observation presents as facts. The collector sets these
   * from the registry; a backend's own metadata records never land here.
   */
  agents: AgentObservation[];
  /**
   * The containers this daemon records, read beside the registry facts. A
   * backend's inventory never carries them, and an empty list means no
   * container was read — not that none exists.
   */
  containers?: PublicContainer[];
  /**
   * Public lifecycle records from the same daemon read as agent facts. The
   * tree suppresses subjects already represented by a joined agent row.
   */
  lifecycle_subjects?: LifecycleObservation[];
  /**
   * Agent records a backend reads from pane metadata. They are physical
   * evidence that protects a pane from unmanaged control, and are never
   * presented as agent facts.
   */
  metadata_agents?: AgentObservation[];
  panes: Pane[];
  tabs: Tab[];
  workspaces: Workspace[];
}
/**
 * One container as a consumer reads it.
 */
export interface PublicContainer {
  binding: Binding2 | null;
  container_id: string;
  created_at: string;
  /**
   * The operation that created it, for a caller reconciling its own requests.
   */
  created_by: string | null;
  kind: Kind;
  label: string | null;
  observed_at: string | null;
  parent: string | null;
  provenance: Provenance;
  /**
   * Whether a recipe is held for this container's pane, so a caller knows the
   * slot can be reloaded without reading private storage.
   */
  recipe: boolean;
  removed_at: string | null;
  unresolved: string | null;
}
/**
 * Where a container's handle lives, and which incarnation answered for it.
 */
export interface Binding2 {
  backend: string;
  handle: string;
  /**
   * The verified process incarnation serving the handle, when one was
   * established. A handle alone cannot be told apart from a later server
   * reusing it, so a binding that names no incarnation is never verified.
   */
  incarnation: ProcessIdentity | null;
}
/**
 * Public lifecycle identity carried into the display projection.
 */
export interface LifecycleObservation {
  bound_registration_id: string | null;
  process: ProcessIdentity | null;
  session: string | null;
  state: State;
  subject_id: string;
}
/**
 * A runtime pane: location plus optional display text.
 *
 * `label` is the pane's declared label, `title` its stripped terminal title;
 * either may be absent, and [`Pane::display_name`] defines the fallback used
 * by presentation rows.
 */
export interface Pane {
  /**
   * Current tmux pane directory, used as the target for a new launch.
   */
  current_path?: string | null;
  label: string | null;
  location: Location;
  title: string | null;
}
/**
 * A tab within a workspace; shown as location detail, not a tree level.
 */
export interface Tab {
  label: string | null;
  number: number | null;
  tab_id: string;
  workspace_id: string;
}
/**
 * A workspace: the first display grouping for the fleet tree.
 */
export interface Workspace {
  label: string | null;
  number: number | null;
  /**
   * tmux's workspace directory, used as the default target for a new launch.
   */
  session_path?: string | null;
  workspace_id: string;
}
/**
 * One source's freshness in a snapshot revision.
 */
export interface ObservationSource {
  last_success_at: string | null;
  name: string;
  state: ObservationFreshness;
}
export interface ObservationNotModifiedResult {
  not_modified: ObservationRevision;
}
export interface ObservationRevision {
  epoch: string;
  revision: number;
}
export interface ObservationExpiredResult {
  snapshot_expired: ObservationRevision;
}
export interface ObservationTooLargeResult {
  snapshot_too_large: ObservationTooLarge;
}
export interface ObservationTooLarge {
  epoch: string;
  limit_bytes: number;
  revision: number;
}
/**
 * Input accepted by `spawn.list`.
 *
 * A caller presents itself; the parent whose launches it reads is derived from
 * the claim it holds. There is deliberately no parent parameter: naming another
 * subject would let any client read its launches.
 */
export interface SpawnListParamsDeserialize {
  after?: string | null;
  limit?: number | null;
  registration_id?: string | null;
}
/**
 * What a completed request did.
 */
export type SpawnListPageSerializeRequestOutcome = "completed" | "refused" | "unknown";
/**
 * What a spawn topology read can say about a child's process exit.
 */
export type SpawnListPageSerializePublicSpawnExitObservation =
  | {
      state: "unknown";
    }
  | {
      code: number | null;
      observed_at: string;
      signal: number | null;
      state: "exited";
    };
/**
 * Freshness is independent of process identity or completion.
 */
export type SpawnListPageSerializeFreshness = "fresh" | "stale";
/**
 * The state of one edge in the recorded topology: the strongest statement a
 * read can make about it, and nothing beyond the records and this read's own
 * evidence.
 */
export type SpawnListPageSerializeSpawnState = "unbound" | "bound" | "unresolved";

/**
 * One page of spawn edges. `next` names the request id the page ended on.
 */
export interface SpawnListPageSerialize {
  next?: string | null;
  spawns: SpawnListPageSerializePublicSpawnTopology[];
}
/**
 * One edge as a topology read reports it: the stored facts, plus the two things
 * a read derives instead of inferring.
 */
export interface SpawnListPageSerializePublicSpawnTopology {
  bound?: SpawnListPageSerializeBoundChild | null;
  claim_id?: string | null;
  created: SpawnListPageSerializeRequestOutcome;
  exit_observation: SpawnListPageSerializePublicSpawnExitObservation;
  /**
   * Freshness of the recorded location against this read's evidence, in the
   * registry's one freshness vocabulary: `fresh` when the backend reports the
   * location now, `stale` when it does not — which is never evidence that the
   * child stopped. Absent when the edge recorded no location, because there is
   * nothing to verify.
   */
  freshness?: SpawnListPageSerializeFreshness | null;
  granted_at?: string | null;
  launched: SpawnListPageSerializeRequestOutcome;
  location?: SpawnListPageSerializeRegistryLocation | null;
  parent: string;
  parent_session_id: string;
  parent_subject_id?: string | null;
  recorded_at: string;
  request_id: string;
  state: SpawnListPageSerializeSpawnState;
  version: number;
  withdrawn?: SpawnListPageSerializeWithdrawn | null;
}
/**
 * The child a token bound: the registering incarnation's own registry identity.
 */
export interface SpawnListPageSerializeBoundChild {
  /**
   * When the edge bound to this incarnation.
   */
  bound_at: string;
  incarnation: string;
  /**
   * Exact registry identity returned to the registering child.
   */
  registration_id: string;
  source: string;
  /**
   * The durable agent the bound child attaches as. Absent for an edge without
   * a tracked launch and in records written before it was recorded.
   */
  subject_id?: string | null;
}
/**
 * Where an agent's incarnation lives on one backend, in that backend's own
 * coordinates.
 *
 * The backend name is free-form so a source Radar does not ship yet is
 * preserved rather than rejected; the rest are the coordinates as reported, and
 * each is optional because a backend need not name every level.
 */
export interface SpawnListPageSerializeRegistryLocation {
  backend: string;
  instance?: string | null;
  pane?: string | null;
  tab?: string | null;
  workspace?: string | null;
}
/**
 * Why and when a launch ended without ever presenting, so a reader never infers
 * failure from absence.
 */
export interface SpawnListPageSerializeWithdrawn {
  at: string;
  reason: string;
}
