import { randomUUID } from "node:crypto";
import type { RadarInspection } from "./radar-inspection.ts";
import { lstatSync } from "node:fs";
import { createConnection } from "node:net";
import { decodeMsgpack, MsgpackError } from "./radar-msgpack.ts";
import { dirname, join } from "node:path";
import type {
	DaemonPingSerialize,
	FactsSnapshotDeserialize,
	AttentionSerialize,
	MessagePageSerialize,
	MessagePublishParamsDeserialize,
	MessageReceiptSerialize,
	MessageSerialize,
	ManagedLaunchMetadataDeserialize,
	SpawnExitObservationSerialize,
	SpawnListParamsDeserialize,
	SpawnListPageSerialize,
	SubjectChildGetParams,
	SubjectChildCloseParams,
	SubjectChildrenParams,
	SubjectGetResult,
	SubjectListResult,
} from "./generated-contracts.ts";

/**
 * Transport for Radar's control plane (agent-radar `docs/control-plane.md`,
 * protocol version 2, and `docs/agent-registration.md` for `registry.v1`).
 *
 * One connection per call, one canonical-UUID request, exactly one matching
 * reply, bounded on both sides. The registry methods this port publishes
 * through, the managed spawn and verified child-pane close it launches and
 * retires children with, and the two mux methods its supervised-lead focus
 * action dispatches are implemented; nothing else is. Every call reports a
 * discriminated outcome instead of throwing, because the daemon is optional and
 * its absence must never reach a caller's control flow.
 *
 * Version 2 is a clean break: every method this port names is a version-2 name,
 * and a method the daemon has removed is answered `unknown_method` rather than
 * served under the old name.
 */
export const RADAR_PROTOCOL_VERSION = 2;
/**
 * Revision 5 names attempts by claim_id and requires exact-claim controls.
 * Revision 7 starts every managed launch through `radar run --launch` and lets a
 * parent continue or restart its own child by subject.
 * Earlier daemons expose different identity fields; require a coordinated update.
 */
export const RADAR_MINIMUM_REVISION = 11;
/** Public daemon contracts this client may preflight. */
export const RADAR_REGISTRY_CONTRACT = "registry.v1";
export const RADAR_REGISTRY_FACTS_CONTRACT = "registry.facts.v1";
export const RADAR_SPAWN_CONTRACT = "spawn.v1";
export const RADAR_SPAWN_CLOSE_CONTRACT = "spawn.close.v1";
export const RADAR_LIFECYCLE_RESTART_CONTRACT = "lifecycle.restart.v1";
export const RADAR_SPAWN_LAUNCH_CONTEXT_CONTRACT = "spawn.launch_context.v1";
export const RADAR_EVENTS_CONTRACT = "events.v1";
export const RADAR_COMMUNICATION_CONTRACT = "communication.messages.v1";
/** The default audit identity; callers can supply their own requester. */
export const RADAR_REQUESTER = "pi-radar";
/** The daemon's own line bound; nothing longer is sent or accepted. */
export const RADAR_MAX_LINE_BYTES = 1024 * 1024;
export const RADAR_CALL_TIMEOUT_MS = 2_000;

export type RadarChannel = "execution" | "assignment" | "facts";

/**
 * `absent` is this process's trusted-socket check refusing to dial; `timeout`,
 * `transport`, `protocol` and `unsupported` are local outcomes. Every other
 * value is the daemon's own refusal code passed through unchanged, because a
 * publisher has to tell fencing (`refused`) from a daemon that lost the record
 * (`not_found`) and from one that is merely missing (`absent`).
 *
 * `unknown` is local to a dispatched mutation: the daemon may have performed it
 * without leaving a trustworthy outcome. It is never proof of non-execution, so
 * a caller must not resend and must not fall back to the direct adapter.
 */
export type RadarErrorCode =
	| "absent"
	| "timeout"
	| "transport"
	| "protocol"
	| "unsupported"
	| "unknown"
	| "bad_version"
	| "bad_request"
	| "bad_params"
	| "unknown_method"
	| "refused"
	| "not_found"
	| "busy"
	| "backend_unavailable"
	| "internal";

/** The daemon's refusal codes (`control_plane::protocol::Code`). */
const DAEMON_ERROR_CODES: readonly RadarErrorCode[] = [
	"bad_version",
	"bad_request",
	"bad_params",
	"unknown_method",
	"refused",
	"not_found",
	"busy",
	"backend_unavailable",
	"internal",
];

/**
 * The daemon's dedicated dispatch-uncertainty code, read as this client's own
 * `unknown`.
 *
 * `outcome_unknown` says a managed close may have been dispatched and cannot be
 * confirmed, which is the same fact as the local `unknown` every caller already
 * refuses to retry. Translating it here keeps that one meaning in one word.
 */
const DAEMON_CODE_ALIASES: Record<string, RadarErrorCode> = {
	outcome_unknown: "unknown",
};

export type RadarResult<T> =
	| { ok: true; value: T }
	| { ok: false; code: RadarErrorCode; message: string };

export type RadarLocation = {
	backend: string;
	instance?: string;
	workspace?: string;
	tab?: string;
	pane?: string;
};

export type RadarPresenceReport = import("./generated-contracts.ts").PresenceReportDeserialize;
export type RadarPresenceAnswer = import("./generated-contracts.ts").PresenceAnswerSerialize;
export type RadarPresenceRelease = import("./generated-contracts.ts").PresenceReleaseDeserialize;

export type RadarProcessClaim = {
	boot_id: string;
	pid: number;
	start_ticks: number;
};

/** The immutable registration request. Nothing mutable belongs here. */
export type RadarRegistration = {
	source: string;
	incarnation: string;
	session?: string;
	owner?: string;
	run?: string;
	label?: string;
	location?: RadarLocation;
	process?: RadarProcessClaim;
	/**
	 * Private, and only for a daemon-spawned child: the one-use value this launch
	 * exported as `PI_RADAR_SPAWN_TOKEN`. It never leaves the registration
	 * request, so it is not part of the persisted subject record.
	 */
	spawn_token?: string;
};

export type RadarPublisherIdentity = {
	source: string;
	incarnation: string;
	reporting_owner?: string;
};

/**
 * One complete published snapshot. `actions` is part of the wire but this port
 * advertises none: the plan's action decision is still open, and a truncated
 * list would omit data the subject has.
 */
export type RadarSnapshot = {
	activity: string;
	waiting_reason?: string;
	last_outcome?: { result: string; detail?: string };
};

/**
 * This process's own facts (`registry.facts.v1`). Every field is optional and a
 * published snapshot replaces the whole value, so omission withdraws a fact.
 * Context usage is counts, not a producer-formatted percentage.
 */
export type RadarContextWindow = NonNullable<
	NonNullable<FactsSnapshotDeserialize["context_window"]>
>;
export type RadarBackgroundTask = NonNullable<
	FactsSnapshotDeserialize["background_tasks"]
>[number];
export type RadarFactsSnapshot = FactsSnapshotDeserialize;
export type RadarMessage = MessageSerialize;
export type RadarMessagePage = MessagePageSerialize;
export type RadarMessageReceipt = MessageReceiptSerialize;

/**
 * The assignment channel's interim fields. `request` is the owner's active
 * request; `awaited` lists the background tasks it is waiting on. Mailbox content
 * (assignment text, questions) is not part of this bridge.
 */
export type RadarAssignmentSnapshot = RadarSnapshot & {
	request?: string;
	awaited?: readonly string[];
};

/** Any snapshot a channel writer accepts. */
export type RadarChannelSnapshot =
	| RadarSnapshot
	| RadarAssignmentSnapshot
	| RadarFactsSnapshot;

export type RadarWriterBinding = {
	handle: string;
	source: string;
	incarnation: string;
	reporting_owner?: string;
	generation: number;
	sequence: number;
	retired_at?: string;
};

/**
 * The one mutable fact this process owns about itself: its current Pi session.
 * `null` is an explicit "no current session", which the daemon serves
 * differently from a record it never received.
 */
export type RadarContextValue = {
	session: string | null;
};

/**
 * The session identity a normalized observation reports for an agent.
 *
 * The daemon keeps only what its source proved: an explicit UUID identity stays
 * an opaque UUID, and every other reported reference keeps its source alongside
 * the exact value it was reported as (a Pi session file path, for Herdr).
 */
export type RadarObservedSession =
	| { kind: "uuid"; value: string }
	| { kind: "reported"; value: string; source?: string };

/** One agent a fresh observation places on one pane. */
export type RadarObservedAgent = {
	workspace_id: string;
	tab_id: string;
	pane_id: string;
	/** The reported agent name, e.g. `pi`. Never the managed worker's label. */
	name?: string;
	/** The runtime lifecycle as reported; unknown labels are preserved. */
	status?: string;
	/** A managed worker's own label and run, when the source published them. */
	managed_label?: string;
	managed_run?: string;
	session?: RadarObservedSession;
};

/** One pane a fresh observation reports, with its canonical containment. */
export type RadarObservedPane = {
	workspace_id: string;
	tab_id: string;
	pane_id: string;
};

/** One workspace a fresh observation reports. */
export type RadarObservedWorkspace = { workspace_id: string };

/** One tab a fresh observation reports, inside its workspace. */
export type RadarObservedTab = {
	tab_id: string;
	workspace_id: string;
};

/**
 * The normalized fleet observation, decoded whole.
 *
 * Every array is decoded even when it is empty. `agents` is the set the daemon
 * presents as facts, projected from its registry for every backend, so an empty
 * array means no registered child is on those panes; a backend's own records
 * never land there. A missing or malformed array is a malformed observation
 * rather than an empty fleet.
 */
export type RadarInventory = {
	workspaces: RadarObservedWorkspace[];
	tabs: RadarObservedTab[];
	panes: RadarObservedPane[];
	agents: RadarObservedAgent[];
};

/**
 * The durable outcome of one managed close. `completed` means the backend
 * confirmed the pane closed, `already_exited` that the edge's recorded pane
 * command had exited and its retained pane was cleaned up, `refused` a positive
 * refusal before any effect, and `unknown` that dispatch may have happened
 * without confirmation - which is never retried, and never read as process exit.
 */
export type RadarCloseOutcome = {
	outcome: "completed" | "already_exited" | "refused" | "unknown";
	pane?: string;
	/** The edge's own recorded exit, present only for `already_exited`. */
	exit_observation?: RadarSpawnExitObservation;
	message?: string;
};

/** What one effect of a spawn did, as the daemon's own record reports it. */
export type RadarEffectOutcome = "completed" | "refused" | "unknown";

/**
 * One durable spawn edge as the daemon publishes it (`spawn.get`).
 *
 * `bound.registration_id` is the exact registry identity the child obtained with the
 * launch's private token, which is the only thing that says *this* child belongs
 * to *this* spawn: a label, a pane or a session UUID would each be a guess.
 */
export type RadarSpawnEdge = {
	request_id: string;
	parent: string;
	created: RadarEffectOutcome;
	launched: RadarEffectOutcome;
	location?: RadarLocation;
	bound?: { source: string; incarnation: string; registration_id?: string; subject_id?: string };
	exit_observation?: RadarSpawnExitObservation;
	/** Set when the daemon withdrew the launch before its process presented. */
	withdrawn?: { at: string; reason: string };
	granted_at?: string;
};

/**
 * What a topology read learned about the child's own process exit. The daemon
 * records the first typed exit it observed while it still had the pane; a pane
 * that disappears is never an exit, and `unknown` is the only value that says
 * no exit was observed.
 */
export type RadarSpawnExitObservation = SpawnExitObservationSerialize;

/** Durable Radar-owned metadata accepted with a managed launch intent. */
export type RadarManagedLaunchMetadata = ManagedLaunchMetadataDeserialize;

/**
 * One edge with what *that read* learned about the location it recorded.
 *
 * `spawn.get` and `spawn.list` answer with this, and its two derived fields
 * belong to the read, not to the record: `freshness: "fresh"` means the
 * backend's inventory reported the recorded pane when this answer was built,
 * while `stale` covers both a pane it did not report and an inventory that
 * could not be consulted. `state: "unresolved"` is a recorded location the
 * backend no longer reports - never process exit.
 */
export type RadarSpawnTopology = {
	edge: RadarSpawnEdge;
	state?: "unbound" | "bound" | "unresolved";
	freshness?: "fresh" | "stale";
};

/**
 * What the daemon verified about a registered child's own process birth
 * identity (`agent.get`). Only `absent` and `mismatched` say the registered
 * process is no longer there; `unavailable` covers no claim, timeout and every
 * unreadable answer, and is never evidence of absence.
 */
export type RadarProcessVerdict =
	| "verified"
	| "absent"
	| "mismatched"
	| "unavailable";

export type RadarAgentProcess = {
	claimed: boolean;
	verification: RadarProcessVerdict;
	/** The registration's own location, when it was registered with one. */
	location?: RadarLocation;
	/** The registration's session, the one it was registered with. */
	session?: string;
	/** The session the registration currently publishes, when it publishes one. */
	contextSession?: string;
	/**
	 * The child's own published activity, present only while its execution
	 * report is fresh. Absent, stale or unreported is unknown, never a state.
	 */
	activity?: string;
};

export type RadarHandshake = DaemonPingSerialize;

export type RadarTimer = { unref?: () => void };
export type RadarScheduler = {
	setTimeout(handler: () => void, ms: number): RadarTimer;
	clearTimeout(timer: RadarTimer): void;
	setInterval(handler: () => void, ms: number): RadarTimer;
	clearInterval(timer: RadarTimer): void;
	now(): number;
};

// Captured when the module loads: a control-socket deadline is wall-clock I/O,
// and a consumer that fakes the global timers must not fire it before the reply
// it is waiting for can arrive.
const wallClock = { setTimeout, clearTimeout, setInterval, clearInterval };

export const systemScheduler: RadarScheduler = {
	setTimeout: (handler, ms) => wallClock.setTimeout(handler, ms),
	clearTimeout: (timer) =>
		wallClock.clearTimeout(timer as ReturnType<typeof setTimeout>),
	setInterval: (handler, ms) => wallClock.setInterval(handler, ms),
	clearInterval: (timer) =>
		wallClock.clearInterval(timer as ReturnType<typeof setInterval>),
	now: () => Date.now(),
};

export type RadarSocket = {
	write(line: string): unknown;
	destroy(): void;
	on(
		event: "connect" | "error" | "close" | "data",
		listener: (value?: unknown) => void,
	): unknown;
};

export type RadarDial = (path: string) => RadarSocket;

/** `RADAR_CONTROL_SOCKET`, then the runtime directory, then the per-uid fallback. */
export function radarControlSocketPath(
	env: NodeJS.ProcessEnv,
	uid: number,
): { path: string; explicit: boolean } {
	const override = env.RADAR_CONTROL_SOCKET?.trim();
	if (override) return { path: override, explicit: true };
	const runtime = env.XDG_RUNTIME_DIR?.trim();
	const directory = runtime
		? join(runtime, "agent-radar")
		: `/tmp/agent-radar-${uid}`;
	return { path: join(directory, "control.sock"), explicit: false };
}

/**
 * Why this path may not be dialled, or `undefined` when it may.
 *
 * Filesystem trust is not authentication between same-uid processes, but a path
 * another process could have planted must not receive publication content. A
 * path that fails the check is simply not a target: nothing is dialled and no
 * timer is held for it.
 */
export function untrustedControlSocket(
	path: string,
	uid: number,
): string | undefined {
	const directory = dirname(path);
	try {
		const parent = lstatSync(directory);
		if (parent.isSymbolicLink() || !parent.isDirectory())
			return `${directory} is not a real directory`;
		if (parent.uid !== uid)
			return `${directory} is not owned by uid ${uid}`;
		if ((parent.mode & 0o777) !== 0o700)
			return `${directory} is not mode 0700`;
		const socket = lstatSync(path);
		if (socket.isSymbolicLink()) return `${path} is a symlink`;
		if (!socket.isSocket()) return `${path} is not a socket`;
		if (socket.uid !== uid) return `${path} is not owned by uid ${uid}`;
		return undefined;
	} catch (error) {
		const code = (error as NodeJS.ErrnoException)?.code ?? "unavailable";
		return `${path} is unavailable (${code})`;
	}
}

function protocol(message: string): RadarResult<never> {
	return { ok: false, code: "protocol", message };
}

function bounded(text: string, bytes: number): string {
	if (Buffer.byteLength(text, "utf8") <= bytes) return text;
	let end = text.length;
	while (end > 0 && Buffer.byteLength(text.slice(0, end), "utf8") > bytes)
		end -= 1;
	return text.slice(0, end);
}

function decodeReply(
	line: string,
	requestId: string,
): RadarResult<unknown> {
	if (Buffer.byteLength(line, "utf8") > RADAR_MAX_LINE_BYTES)
		return protocol("reply exceeds the control-plane line bound");
	let parsed: unknown;
	try {
		parsed = JSON.parse(line);
	} catch {
		return protocol("reply is not JSON");
	}
	if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed))
		return protocol("reply is not an object");
	const answer = parsed as Record<string, unknown>;
	if (answer.id !== requestId)
		return protocol("reply does not match the request id");
	if (answer.error !== undefined) {
		const error = answer.error as Record<string, unknown> | undefined;
		const code = error?.code;
		const message = error?.message;
		return {
			ok: false,
			code:
				typeof code === "string"
					? (DAEMON_CODE_ALIASES[code] ??
						(DAEMON_ERROR_CODES.includes(code as RadarErrorCode)
							? (code as RadarErrorCode)
							: "internal"))
					: "internal",
			message: `${typeof code === "string" ? code : "error"}: ${
				typeof message === "string" ? message : "no message"
			}`,
		};
	}
	if (!("result" in answer)) return protocol("reply carries no result");
	return { ok: true, value: answer.result };
}

const UUID =
	/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function recordAt(value: unknown, field: string): unknown {
	if (typeof value !== "object" || value === null || Array.isArray(value)) return undefined;
	return (value as Record<string, unknown>)[field];
}

export function isUuid(value: unknown): value is string {
	return typeof value === "string" && UUID.test(value);
}

function decodePresenceAnswer(value: unknown): RadarResult<RadarPresenceAnswer> {
	const registrationId = recordAt(value, "registration_id");
	const claimId = recordAt(value, "claim_id");
	const standing = recordAt(value, "standing");
	const subjectId = recordAt(value, "subject_id");
	if (
		!isUuid(registrationId) ||
		!isUuid(claimId) ||
		!["reserved", "pending", "granted", "duplicate", "released"].includes(String(standing)) ||
		(subjectId !== undefined && !isUuid(subjectId)) ||
		(standing === "granted" && !isUuid(subjectId))
	)
		return protocol("presence reply carries no valid claim standing");
	return { ok: true, value: value as RadarPresenceAnswer };
}

/**
 * How Radar stands on a publisher's current session: adopted as a managed Pi
 * session, rejected as a duplicate, still pending evidence/recovery, or not managed (yet).
 */
export type RadarManagement =
	| { state: "managed"; subject_id: string }
	| { state: "duplicate" }
	| { state: "pending" }
	| { state: "unmanaged" };

function managementAt(value: unknown): RadarManagement | undefined {
	const field = recordAt(value, "management");
	const state = stringAt(field, "state");
	if (state === "managed") {
		const subjectId = stringAt(field, "subject_id");
		return subjectId === undefined ? undefined : { state, subject_id: subjectId };
	}
	if (state === "duplicate" || state === "pending" || state === "unmanaged") return { state };
	return undefined;
}

function stringAt(value: unknown, field: string): string | undefined {
	if (typeof value !== "object" || value === null) return undefined;
	const found = (value as Record<string, unknown>)[field];
	return typeof found === "string" ? found : undefined;
}

function numberAt(value: unknown, field: string): number | undefined {
	if (typeof value !== "object" || value === null) return undefined;
	const found = (value as Record<string, unknown>)[field];
	return typeof found === "number" && Number.isFinite(found)
		? found
		: undefined;
}

/** The execution that produced or received a message. */
function identityAt(value: unknown): boolean {
	return isUuid(stringAt(value, "registration_id")) &&
		isUuid(stringAt(value, "session_id")) &&
		isUuid(stringAt(value, "subject_id")) &&
		isUuid(stringAt(value, "claim_id"));
}

function messageAt(value: unknown): MessageSerialize | undefined {
	const version = numberAt(value, "version");
	const id = stringAt(value, "id");
	const recipients = (value as Record<string, unknown> | null)?.recipients;
	const body = recordAt(value, "body");
	const created = numberAt(value, "created_at_unix_ms");
	if (
		version !== 1 || !isUuid(id) || !identityAt(recordAt(value, "producer")) ||
		!Array.isArray(recipients) || recipients.length === 0 || recipients.some((one) => !isUuid(one)) ||
		stringAt(body, "kind") !== "handoff" || stringAt(body, "summary") === undefined ||
		stringAt(body, "content") === undefined || created === undefined ||
		!Number.isSafeInteger(created) || created < 0
	) return undefined;
	return value as MessageSerialize;
}

function messageReceiptAt(value: unknown): MessageReceiptSerialize | undefined {
	const version = numberAt(value, "version");
	const messageId = stringAt(value, "message_id");
	const received = numberAt(value, "received_at_unix_ms");
	if (
		version !== 1 || !isUuid(messageId) || !identityAt(recordAt(value, "recipient")) ||
		received === undefined || !Number.isSafeInteger(received) || received < 0
	) return undefined;
	return value as MessageReceiptSerialize;
}

/** A writer binding a channel or context reply hands back to its publisher. */
function writerBinding(value: unknown): RadarWriterBinding | undefined {
	const handle = stringAt(value, "handle");
	const source = stringAt(value, "source");
	const incarnation = stringAt(value, "incarnation");
	const generation = numberAt(value, "generation");
	const sequence = numberAt(value, "sequence");
	if (
		!isUuid(handle) ||
		source === undefined ||
		incarnation === undefined ||
		generation === undefined ||
		sequence === undefined
	)
		return undefined;
	const reportingOwner = stringAt(value, "reporting_owner");
	const retiredAt = stringAt(value, "retired_at");
	return {
		handle,
		source,
		incarnation,
		...(reportingOwner !== undefined ? { reporting_owner: reportingOwner } : {}),
		generation,
		sequence,
		...(retiredAt !== undefined ? { retired_at: retiredAt } : {}),
	};
}

/**
 * One observed agent, decoded from the normalized inventory.
 *
 * Only the location and the session identity are kept: they are what an exact
 * identity proof needs, and a partially understood record must fail closed
 * rather than be guessed at.
 */
function observedAgent(value: unknown): RadarResult<RadarObservedAgent> {
	const location = recordAt(value, "location");
	const workspaceId = stringAt(location, "workspace_id");
	const tabId = stringAt(location, "tab_id");
	const paneId = stringAt(location, "pane_id");
	if (!workspaceId || !tabId || !paneId)
		return protocol("observe reply carries a malformed agent location");
	const observed = recordAt(value, "session");
	let session: RadarObservedSession | undefined;
	if (observed !== undefined && observed !== null) {
		const kind = stringAt(observed, "kind");
		const identity = stringAt(observed, "value");
		if (kind === "uuid" && isUuid(identity)) session = { kind: "uuid", value: identity };
		else if (kind === "reported") {
			// A source-qualified reference keeps the exact value it was reported
			// as, beside the source that reported it.
			const reported = recordAt(observed, "value");
			const reference = stringAt(reported, "value");
			if (reference === undefined || reference === "")
				return protocol("observe reply carries a malformed session reference");
			const source = stringAt(reported, "source");
			session = {
				kind: "reported",
				value: reference,
				...(source !== undefined ? { source } : {}),
			};
		} else return protocol("observe reply carries an unrecognized session identity");
	}
	const facts = recordAt(value, "facts");
	const name = stringAt(value, "name");
	const status = runtimeStatus(value);
	const managedLabel = stringAt(facts, "label");
	const managedRun = stringAt(facts, "run");
	return {
		ok: true,
		value: {
			workspace_id: workspaceId,
			tab_id: tabId,
			pane_id: paneId,
			...(name !== undefined ? { name } : {}),
			...(status !== undefined ? { status } : {}),
			...(managedLabel !== undefined ? { managed_label: managedLabel } : {}),
			...(managedRun !== undefined ? { managed_run: managedRun } : {}),
			...(session === undefined ? {} : { session }),
		},
	};
}

/**
 * The reported lifecycle word, with an unrecognized label preserved verbatim.
 *
 * The model serializes a known status as its own snake_case word and anything
 * else as `{"other":"…"}`, so a source that invents a status never fails an
 * otherwise valid inventory.
 */
function runtimeStatus(value: unknown): string | undefined {
	const status = recordAt(value, "status");
	if (typeof status === "string") return status;
	return stringAt(status, "other");
}

export function radarLifecycleState(
	agent: Pick<RadarObservedAgent, "status">,
): "idle" | "working" | "blocked" | "done" | "unknown" {
	const status = agent.status;
	return status === "idle" ||
		status === "working" ||
		status === "blocked" ||
		status === "done"
		? status
		: "unknown";
}

/** One canonical pane row: its identity and the containment it was reported in. */
function observedPane(value: unknown): RadarResult<RadarObservedPane> {
	const location = recordAt(value, "location");
	const workspaceId = stringAt(location, "workspace_id");
	const tabId = stringAt(location, "tab_id");
	const paneId = stringAt(location, "pane_id");
	if (!workspaceId || !tabId || !paneId)
		return protocol("observe reply carries a malformed pane location");
	return {
		ok: true,
		value: { workspace_id: workspaceId, tab_id: tabId, pane_id: paneId },
	};
}

/**
 * The failure codes that prove the daemon never dispatched the effect. Every
 * other failure — a lost reply, a write that may have landed, the daemon's own
 * `internal` — leaves a dispatched mutation unresolved (`control-plane.md`,
 * Durable operations).
 */
const DISPATCH_REFUSED_CODES: readonly RadarErrorCode[] = [
	"absent",
	"unsupported",
	"bad_version",
	"bad_request",
	"bad_params",
	"unknown_method",
	"refused",
	"not_found",
	"busy",
	"backend_unavailable",
];

function unresolvedOperation(requestId: string, detail: string): RadarResult<never> {
	return {
		ok: false,
		code: "unknown",
		message: `focus ${requestId} is unresolved (${detail})`,
	};
}

export const RADAR_REGISTRATION_ENV = "PI_RADAR_REGISTRATION_ID";
const RADAR_MANAGED_IDENTITY_ENV = new Set(["PI_RADAR_MANAGED_PARENT_SUBJECT_ID"]);

/**
 * A spawn this process may still have caused.
 *
 * It carries the request id so the caller keeps reconciling that one edge, and
 * it is never a reason to send the spawn again or to reach for another
 * launcher: the pane and the child may already exist under this id.
 */
function unresolvedSpawn(requestId: string, detail: string): RadarResult<never> {
	return {
		ok: false,
		code: "unknown",
		message: `spawn ${requestId} is unresolved (${detail})`,
	};
}

const EFFECT_OUTCOMES: readonly RadarEffectOutcome[] = [
	"completed",
	"refused",
	"unknown",
];

function effectAt(
	value: unknown,
	field: string,
): RadarEffectOutcome | undefined {
	const found = stringAt(value, field);
	return found !== undefined &&
		EFFECT_OUTCOMES.includes(found as RadarEffectOutcome)
		? (found as RadarEffectOutcome)
		: undefined;
}

/** The location a create named, or nothing when the edge recorded none. */
function locationAt(value: unknown): RadarLocation | undefined {
	const location = recordAt(value, "location");
	const backend = stringAt(location, "backend");
	if (backend === undefined || backend === "") return undefined;
	const instance = stringAt(location, "instance");
	const workspace = stringAt(location, "workspace");
	const tab = stringAt(location, "tab");
	const pane = stringAt(location, "pane");
	return {
		backend,
		...(instance !== undefined ? { instance } : {}),
		...(workspace !== undefined ? { workspace } : {}),
		...(tab !== undefined ? { tab } : {}),
		...(pane !== undefined ? { pane } : {}),
	};
}

/**
 * One durable edge, decoded strictly. The bound child's `registration_id` is kept only
 * when it is a canonical UUID: an unreadable one is not an identity to correlate with.
 */
function spawnEdge(value: unknown): RadarResult<RadarSpawnEdge> {
	const requestId = stringAt(value, "request_id");
	const parent = stringAt(value, "parent");
	const created = effectAt(value, "created");
	const launched = effectAt(value, "launched");
	if (
		!isUuid(requestId) ||
		!isUuid(parent) ||
		created === undefined ||
		launched === undefined
	)
		return protocol("spawn reply carries no durable edge");
	const location = locationAt(value);
	const bound = recordAt(value, "bound");
	const source = stringAt(bound, "source");
	const incarnation = stringAt(bound, "incarnation");
	const agentId = stringAt(bound, "registration_id");
	const subjectId = stringAt(bound, "subject_id");
	const exit = exitAt(recordAt(value, "exit_observation"));
	const withdrawnRecord = recordAt(value, "withdrawn");
	const withdrawnAt = stringAt(withdrawnRecord, "at");
	const withdrawnReason = stringAt(withdrawnRecord, "reason");
	return {
		ok: true,
		value: {
			request_id: requestId,
			parent,
			created,
			launched,
			...(location !== undefined ? { location } : {}),
			...(source !== undefined && incarnation !== undefined
				? {
						bound: {
							source,
							incarnation,
							...(isUuid(agentId) ? { registration_id: agentId } : {}),
							...(isUuid(subjectId) ? { subject_id: subjectId } : {}),
						},
					}
				: {}),
			...(exit !== undefined ? { exit_observation: exit } : {}),
			...(stringAt(value, "granted_at") !== undefined ? { granted_at: stringAt(value, "granted_at") } : {}),
			...(withdrawnAt !== undefined && withdrawnReason !== undefined
				? { withdrawn: { at: withdrawnAt, reason: withdrawnReason } }
				: {}),
		},
	};
}

/**
 * The exit observation as this client reads it.
 *
 * An observation that is present but says neither `unknown` nor a decodable
 * `exited` is reported as an exit: reading an unreadable observation as "no exit
 * observed" would turn a maybe-dead child into a live one.
 */
const PROCESS_VERDICTS: readonly RadarProcessVerdict[] = [
	"verified",
	"absent",
	"mismatched",
	"unavailable",
];

function subjectAt(value: unknown): RadarResult<SubjectGetResult["subject"]> {
	const subjectId = recordAt(value, "subject_id");
	const session = recordAt(value, "session_id");
	const state = recordAt(value, "state");
	if (!isUuid(subjectId) || (session != null && !isUuid(session)) ||
		!["starting", "live", "detached", "unknown"].includes(String(state)))
		return protocol("subject read carries no valid identity or attachment state");
	return { ok: true, value: value as SubjectGetResult["subject"] };
}

function agentProcessAt(agent: unknown): RadarResult<RadarAgentProcess> {
	const process = recordAt(agent, "process");
	const claimed = recordAt(process, "claimed");
	const verification = stringAt(process, "verification");
	if (
		typeof claimed !== "boolean" ||
		verification === undefined ||
		!PROCESS_VERDICTS.includes(verification as RadarProcessVerdict)
	)
		return protocol("agent.get reply carries no process verdict");
	const registration = recordAt(agent, "registration");
	const location = locationAt(registration);
	const session = stringAt(registration, "session");
	// The pinned reply nests the current session one level down, and it is
	// current only while the daemon reports that context fresh.
	const context = recordAt(recordAt(agent, "context"), "context");
	const contextSession =
		stringAt(context, "freshness") === "fresh" ? stringAt(context, "session") : undefined;
	const execution = recordAt(recordAt(agent, "execution"), "snapshot");
	const activity =
		stringAt(execution, "freshness") === "fresh"
			? stringAt(recordAt(execution, "snapshot"), "activity")
			: undefined;
	return {
		ok: true,
		value: {
			claimed,
			verification: verification as RadarProcessVerdict,
			...(location !== undefined ? { location } : {}),
			...(session !== undefined ? { session } : {}),
			...(contextSession !== undefined ? { contextSession } : {}),
			...(activity !== undefined ? { activity } : {}),
		},
	};
}

function exitAt(value: unknown): RadarSpawnExitObservation | undefined {
	if (value === undefined) return undefined;
	if (stringAt(value, "state") === "unknown") return { state: "unknown" };
	if (stringAt(value, "state") !== "exited") return undefined;
	const code = recordAt(value, "code");
	const signal = recordAt(value, "signal");
	const observedAt = stringAt(value, "observed_at");
	const isInt32OrNull = (found: unknown) =>
		found === null ||
		(typeof found === "number" && Number.isInteger(found) && found >= -2147483648 && found <= 2147483647);
	if (!isInt32OrNull(code) || !isInt32OrNull(signal) || observedAt === undefined)
		return undefined;
	return {
		state: "exited",
		code: code as number | null,
		signal: signal as number | null,
		observed_at: observedAt,
	};
}

/**
 * One topology read, decoded strictly: the derived fields only matter in the
 * words the daemon writes, and a reply whose `spawn` is not an edge carrying a
 * nested edge is a protocol error rather than an edge with no liveness.
 */
function spawnTopology(value: unknown): RadarResult<RadarSpawnTopology> {
	const edge = spawnEdge(value);
	if (edge.ok === false) return edge;
	const state = stringAt(value, "state");
	const freshness = stringAt(value, "freshness");
	return {
		ok: true,
		value: {
			edge: edge.value,
			...(state === "unbound" || state === "bound" || state === "unresolved"
				? { state }
				: {}),
			...(freshness === "fresh" || freshness === "stale"
				? { freshness }
				: {}),
		},
	};
}

/**
 * The command and identity shape refused before anything is dialed.
 *
 * The daemon validates its own request shape, and its refusal would also land
 * before any effect; this is the same refusal one step earlier, where a launch
 * argument no supported backend can carry (a control character, a second line, a
 * reserved name) fails with the caller's own words instead of the far end's.
 */
function spawnRequestShape(request: {
	request_id: string;
	parent: string;
	parent_session_id: string;
	executable: string;
	argv: string[];
	cwd?: string;
	env?: Record<string, string>;
	managed?: RadarManagedLaunchMetadata;
	continue_subject_id?: string;
}): string | undefined {
	if (!isUuid(request.request_id))
		return "spawn requires a canonical request UUID";
	if (request.continue_subject_id !== undefined && !isUuid(request.continue_subject_id))
		return "continue_subject_id must be a canonical UUID";
	if (!isUuid(request.parent))
		return "spawn requires the parent's canonical agent UUID";
	if (!isUuid(request.parent_session_id))
		return "spawn requires a canonical parent session UUID";
	if (!request.executable.startsWith("/") || /\p{Cc}/u.test(request.executable))
		return "spawn requires an absolute, printable executable path";
	if (request.argv.some((argument) => /\p{Cc}/u.test(argument)))
		return "no supported backend carries a control character or a second line in an argument";
	if (request.cwd !== undefined && !request.cwd.startsWith("/"))
		return "spawn cwd must be an absolute path";
	const env = Object.entries(request.env ?? {});
	if (env.length > 256) return "spawn env names more than 256 entries";
	for (const [name, value] of env) {
		if (name === RADAR_REGISTRATION_ENV)
			return `spawn env cannot set the reserved ${RADAR_REGISTRATION_ENV}`;
		if (RADAR_MANAGED_IDENTITY_ENV.has(name))
			return `spawn env cannot set Radar-issued identity variable ${name}`;
		if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name) || /\p{Cc}/u.test(value))
			return `spawn env contains an invalid name or value: ${name}`;
	}
	if (request.managed !== undefined) {
		if (typeof request.managed !== "object" || request.managed === null)
			return "managed launch metadata must be an object";
		for (const [name, value] of Object.entries(request.managed)) {
			if (!["owner", "run", "label", "role", "parent_subject_id"].includes(name))
				return `unknown managed launch metadata field: ${name}`;
			if (value != null && (typeof value !== "string" || value.trim() === "" || new TextEncoder().encode(value).length > 256 || /\p{Cc}/u.test(value)))
				return `managed launch ${name} must be non-empty, control-free, and at most 256 UTF-8 bytes`;
		}
		if (
			request.managed.parent_subject_id != null &&
			!isUuid(request.managed.parent_subject_id)
		)
			return "managed parent_subject_id must be a canonical UUID";
	}
	return undefined;
}

/**
 * The outcome a durable operation record supports. Only `completed` is success
 * and only `refused` proves the effect did not run; a record that is still in
 * flight, or one with no terminal outcome, stays unresolved.
 */
function focusOutcome(
	record: unknown,
	requestId: string,
): RadarResult<{ request_id: string }> {
	if (typeof record !== "object" || record === null)
		return unresolvedOperation(requestId, "the daemon returned no operation record");
	const outcome = stringAt(record, "outcome");
	if (outcome === "completed") return { ok: true, value: { request_id: requestId } };
	if (outcome === "refused")
		return {
			ok: false,
			code: "refused",
			message: stringAt(record, "message") ?? `the daemon refused focus ${requestId}`,
		};
	return unresolvedOperation(
		requestId,
		`the record is ${stringAt(record, "state") ?? "unreadable"} with ${
			outcome === undefined ? "no outcome" : `outcome ${outcome}`
		}`,
	);
}

/**
 * One answer from the event journal. Events are pointers, so a page carries
 * only where it ends: `sequence` is the last delivered event, or the request's
 * own `after` when nothing was delivered.
 */
export type RadarEventsReply =
	| { kind: "page"; epoch: string; sequence: number; more: boolean }
	| { kind: "expired"; epoch: string; sequence: number };

export type RadarClient = {
	socketPath(): string;
	ping(): Promise<RadarResult<RadarHandshake>>;
	register(
		registration: RadarRegistration,
	): Promise<RadarResult<{ registration_id: string }>>;
	acquire(request: {
		registration_id?: string;
		subject_id?: string;
		channel: RadarChannel;
		publisher: RadarPublisherIdentity;
		replace?: { generation: number; handle: string };
	}): Promise<RadarResult<RadarWriterBinding>>;
	publish(request: {
		registration_id?: string;
		subject_id?: string;
		channel: RadarChannel;
		writer_handle: string;
		sequence: number;
		lease_ms?: number;
		observed_at?: string;
		snapshot: RadarChannelSnapshot;
	}): Promise<RadarResult<{ sequence: number }>>;
	presence(request: RadarPresenceReport): Promise<RadarResult<RadarPresenceAnswer>>;
	releasePresence(request: RadarPresenceRelease): Promise<RadarResult<RadarPresenceAnswer>>;
	context(request: {
		registration_id: string;
		publisher: RadarPublisherIdentity;
		writer_handle?: string;
		replace?: { generation: number; handle: string };
		sequence: number;
		lease_ms?: number;
		observed_at?: string;
		context: RadarContextValue;
	}): Promise<RadarResult<{ writer: RadarWriterBinding; warning?: string; management?: RadarManagement }>>;
	retire(request: {
		registration_id?: string;
		subject_id?: string;
		channel: RadarChannel;
		writer_handle: string;
	}): Promise<RadarResult<void>>;
	/**
	 * Close the pane of one of this process's own children, named by subject.
	 * Radar resolves the launch and the occupying process. `request_id` is this
	 * close's own durable key: a lost reply is reconciled by resending the
	 * identical request, never by a new key.
	 */
	childClose(request: {
		request_id: string;
		subject_id: string;
		intent: "complete" | "cancel";
	}): Promise<RadarResult<RadarCloseOutcome>>;
	/**
	 * Create and launch one managed child under a registered parent's own pane.
	 * `request_id` is the caller's durable key: it is what a lost reply is
	 * reconciled with, and it must never be replaced by a second attempt.
	 */
	spawn(request: {
		request_id: string;
		parent: string;
		parent_session_id: string;
		executable: string;
		argv: string[];
		cwd?: string;
		env?: Record<string, string>;
		managed?: RadarManagedLaunchMetadata;
		/** Start this released child agent again, in the session the daemon recorded for it. */
		continue_subject_id?: string;
	}): Promise<RadarResult<RadarSpawnEdge>>;
	/**
	 * Restart this caller's own live child under a new program. The daemon finds the
	 * child's current process from the subject and replaces it in its own pane and
	 * session; a program with a bare argument that may be a message is refused.
	 */
	restart(request: {
		request_id: string;
		parent: string;
		subject_id: string;
		executable: string;
		argv: string[];
		cwd?: string;
		env?: Record<string, string>;
	}): Promise<RadarResult<{ request_id: string; launch_request_id: string }>>;
	/** List launches owned by this client's registered process. */
	spawnList(request: Pick<SpawnListParamsDeserialize, "after" | "limit">): Promise<RadarResult<SpawnListPageSerialize>>;
	spawnGet(request_id: string): Promise<RadarResult<RadarSpawnTopology>>;
	subjectChildren(request?: Omit<SubjectChildrenParams, "registration_id">): Promise<RadarResult<SubjectListResult>>;
	childGet(subject_id: string): Promise<RadarResult<SubjectGetResult["subject"]>>;
	childInspect(subject_id: string): Promise<RadarResult<RadarInspection>>;
	/** The registered child's process verdict, read from its immutable claim. */
	agentGet(registration_id: string): Promise<RadarResult<RadarAgentProcess>>;
	observe(): Promise<RadarResult<RadarInventory>>;
	/** Read the durable event journal after `after`, waiting at most `wait_ms`. */
	messagePublish(request: MessagePublishParamsDeserialize & {
		request_id: string;
	}): Promise<RadarResult<{ message: MessageSerialize; created: boolean }>>;
	messagePage(request: {
		registration_id: string;
		after?: string;
		limit?: number;
	}): Promise<RadarResult<MessagePageSerialize>>;
	messageGet(request: {
		registration_id: string;
		message_id: string;
	}): Promise<RadarResult<{ message: MessageSerialize; receipt: MessageReceiptSerialize | null }>>;
	messageReceive(request: {
		registration_id: string;
		message_id: string;
	}): Promise<RadarResult<{ receipt: MessageReceiptSerialize; created: boolean }>>;
	/**
	 * Hold one recipient-attention subscription until the returned handle is
	 * closed. The baseline arrives before this resolves, and every later update
	 * reports this subject's mailbox only.
	 */
	watchAttention(request: {
		registration_id: string;
		onAttention(attention: AttentionSerialize): void;
	}): Promise<RadarResult<{ close(): void }>>;
	events(request: {
		after: number;
		epoch?: string;
		wait_ms: number;
	}): Promise<RadarResult<RadarEventsReply>>;
	focus(request: {
		/** The caller's fresh operation UUID; it is also the reconciliation key. */
		request_id: string;
		pane_id: string;
	}): Promise<RadarResult<{ request_id: string }>>;
	/** Bounded, non-consuming snapshot of a pane's recent output. */
	output(request: {
		pane_id: string;
		source: "recent_unwrapped";
		lines?: number;
	}): Promise<RadarResult<RadarOutputRead>>;
	/** Current foreground evidence for a pane; inconclusive is not proof of exit. */
	processInfo(request: { pane_id: string }): Promise<RadarResult<RadarForeground>>;
};

export type RadarOutputRead = {
	text: string;
	truncated: boolean;
	revision?: number;
};

/** The pinned `ForegroundEvidence` kinds, with the leader's facts when known. */
export type RadarForeground =
	| { kind: "shell" }
	| { kind: "non_shell"; pid: number; name?: string; command?: string }
	| { kind: "inconclusive" };

export type RadarClientOptions = {
	env?: NodeJS.ProcessEnv;
	uid?: number;
	dial?: RadarDial;
	scheduler?: RadarScheduler;
	timeoutMs?: number;
	requester?: string;
};

function eventsReplyAt(
	after: number,
	value: unknown,
): RadarEventsReply | undefined {
	const expired = recordAt(value, "cursor_expired");
	if (expired !== undefined) {
		const epoch = stringAt(expired, "epoch");
		const sequence = numberAt(recordAt(expired, "cursor"), "sequence");
		return epoch !== undefined && sequence !== undefined
			? { kind: "expired", epoch, sequence }
			: undefined;
	}
	const epoch = stringAt(value, "epoch");
	const events = recordAt(value, "events");
	const more = recordAt(value, "more");
	if (epoch === undefined || !Array.isArray(events) || typeof more !== "boolean")
		return undefined;
	let sequence = after;
	for (const event of events) {
		const seq = numberAt(event, "seq");
		if (seq === undefined || seq <= sequence) return undefined;
		sequence = seq;
	}
	return { kind: "page", epoch, sequence, more };
}

/** The longest one journal read may wait: the daemon's own bound. */
const RADAR_EVENT_WAIT_MS = 1_000;
/** The pause before a journal read the daemon could not answer is retried. */
export const RADAR_EVENT_RETRY_MS = 1_000;

/**
 * Follow the daemon's event journal as a wake-up only. A page or an expiry says
 * that something changed and nothing more: `wake` carries no body, so no event
 * can set state. The caller reconciles from current reads, and its own periodic
 * reconciliation covers a wake that never arrives.
 *
 * A page that has more behind it is read again at once. The cursor only moves
 * to the last delivered sequence, never to the journal tail. A refused contract
 * is reported once per outage through `diagnose`, then retried. A throwing
 * `wake` is diagnosed once per run of failures and never ends the loop.
 */
export function watchRadarEvents(
	client: Pick<RadarClient, "events">,
	signal: AbortSignal,
	handlers: { wake: () => void; diagnose: (message: string) => void },
): void {
	let after = 0;
	let epoch: string | undefined;
	let refusing = false;
	let faulting = false;
	// A notice that cannot be delivered must not end the watcher: the periodic
	// scan keeps the system correct without it.
	const diagnose = (message: string): void => {
		try {
			handlers.diagnose(message);
		} catch {
			// The notice channel is gone, for example a stale extension context.
		}
	};
	const wake = (): void => {
		try {
			handlers.wake();
			faulting = false;
		} catch (error) {
			if (!faulting) diagnose(`event wake failed: ${String(error)}`);
			faulting = true;
		}
	};
	const pause = (ms: number): Promise<void> =>
		new Promise((resolve) => {
			const done = (): void => {
				clearTimeout(timer);
				signal.removeEventListener("abort", done);
				resolve();
			};
			const timer = setTimeout(done, ms);
			timer.unref?.();
			signal.addEventListener("abort", done, { once: true });
		});
	const follow = async (): Promise<void> => {
		let wait = RADAR_EVENT_WAIT_MS;
		while (!signal.aborted) {
			const result = await client.events({ after, epoch, wait_ms: wait });
			if (signal.aborted) return;
			if (result.ok === false) {
				if (result.code === "unsupported" && !refusing) {
					refusing = true;
					diagnose(result.message);
				}
				await pause(RADAR_EVENT_RETRY_MS);
				wait = RADAR_EVENT_WAIT_MS;
				continue;
			}
			refusing = false;
			const reply = result.value;
			epoch = reply.epoch;
			const moved = reply.kind === "expired" || reply.sequence > after;
			after = reply.sequence;
			if (moved) wake();
			wait = reply.kind === "page" && reply.more ? 0 : RADAR_EVENT_WAIT_MS;
		}
	};
	// Invariant: the loop ends only through `signal`, because `client.events`
	// resolves rather than rejects - `call` turns every fault into a result. If
	// that stops being true, this catch is the only signal. Nothing retries it,
	// and the periodic scan keeps state correct meanwhile.
	void follow().catch((error) =>
		diagnose(`event watch stopped: ${String(error)}`),
	);
}

export function createRadarClient(options: RadarClientOptions = {}): RadarClient {
	const env = options.env ?? process.env;
	const uid = options.uid ?? process.getuid?.() ?? 0;
	const scheduler = options.scheduler ?? systemScheduler;
	const timeoutMs = options.timeoutMs ?? RADAR_CALL_TIMEOUT_MS;
	const requester = options.requester ?? RADAR_REQUESTER;
	const dial =
		options.dial ?? ((path: string) => createConnection(path) as unknown as RadarSocket);

	const currentPath = (): string => radarControlSocketPath(env, uid).path;

	/**
	 * One attention subscription: a JSON opener, then length-prefixed MessagePack
	 * frames of mailbox state. Resolves on the baseline, so a caller never tells
	 * "nothing waiting" from "not yet attached".
	 */
	const watch = (
		path: string,
		params: { registration_id: string },
		onAttention: (attention: AttentionSerialize) => void,
	): Promise<RadarResult<{ close(): void }>> => {
		const line = `${JSON.stringify({
			version: RADAR_PROTOCOL_VERSION,
			id: randomUUID(),
			method: "messages.watch",
			params,
		})}\n`;
		return new Promise<RadarResult<{ close(): void }>>((resolve) => {
			let settled = false;
			let socket: RadarSocket | undefined;
			let buffered = Buffer.alloc(0);
			let binary = false;
			let baseline = false;
			const close = (): void => {
				try {
					socket?.destroy();
				} catch {
					// A socket that will not close is already unusable.
				}
			};
			const finish = (result: RadarResult<{ close(): void }>): void => {
				if (settled) return;
				settled = true;
				if (result.ok === false) close();
				resolve(result);
			};
			const deliver = (attention: AttentionSerialize): void => {
				onAttention(attention);
				if (!baseline) {
					baseline = true;
					// The subscription is usable only once it holds a baseline.
					finish({ ok: true, value: { close } });
				}
			};
			let connection: RadarSocket;
			try {
				connection = dial(path);
				socket = connection;
			} catch (error) {
				return finish({
					ok: false,
					code: "transport",
					message: String((error as Error)?.message ?? error),
				});
			}
			connection.on("error", (error) =>
				finish({
					ok: false,
					code: "transport",
					message: String((error as Error)?.message ?? error),
				}),
			);
			connection.on("close", () => {
				// A closed watch ends delivery, never pending work: the mailbox keeps
				// it and the next heartbeat reports it.
				finish({ ok: true, value: { close } });
			});
			connection.on("connect", () => {
				try {
					connection.write(line);
				} catch (error) {
					finish({
						ok: false,
						code: "transport",
						message: String((error as Error)?.message ?? error),
					});
				}
			});
			connection.on("data", (value) => {
				buffered = Buffer.concat([
					buffered,
					Buffer.isBuffer(value) ? value : Buffer.from(String(value ?? ""), "utf8"),
				]);
				if (!binary) {
					const end = buffered.indexOf(0x0a);
					if (end < 0) {
						if (buffered.length > RADAR_MAX_LINE_BYTES)
							finish({ ok: false, code: "protocol", message: "attention opener is too long" });
						return;
					}
					let answer: unknown;
					try {
						answer = JSON.parse(buffered.subarray(0, end).toString("utf8"));
					} catch {
						return finish({ ok: false, code: "protocol", message: "attention opener reply is not JSON" });
					}
					buffered = buffered.subarray(end + 1);
					const error = recordAt(answer, "error");
					if (error !== undefined && error !== null) {
						return finish({
							ok: false,
							code: (stringAt(error, "code") as RadarErrorCode | undefined) ?? "refused",
							message: stringAt(error, "message") ?? "the daemon refused the attention subscription",
						});
					}
					binary = true;
				}
				for (;;) {
					if (buffered.length < 4) return;
					const length = buffered.readUInt32BE(0);
					if (length > 4096)
						return finish({ ok: false, code: "protocol", message: "attention frame exceeds its bound" });
					if (buffered.length < 4 + length) return;
					const frame = buffered.subarray(4, 4 + length);
					buffered = buffered.subarray(4 + length);
					let attention: unknown;
					try {
						attention = decodeMsgpack(frame);
					} catch (error) {
						return finish({
							ok: false,
							code: "protocol",
							message: error instanceof MsgpackError ? error.message : "attention frame is not MessagePack",
						});
					}
					const revision = stringAt(attention, "revision");
					const pending = numberAt(attention, "pending");
					if (revision === undefined || pending === undefined || !Number.isSafeInteger(pending) || pending < 0)
						return finish({ ok: false, code: "protocol", message: "attention frame carries no mailbox state" });
					deliver({ revision, pending });
				}
			});
		});
		};


	/**
	 * One envelope per connection. `id` is the caller's own operation UUID when a
	 * mutation has to be reconcilable after a lost reply; it is otherwise minted
	 * per call.
	 */
	async function call(
		method: string,
		params: Record<string, unknown>,
		id: string = randomUUID(),
	): Promise<RadarResult<unknown>> {
		const path = currentPath();
		const refused = untrustedControlSocket(path, uid);
		if (refused !== undefined)
			return { ok: false, code: "absent", message: refused };
		const line = `${JSON.stringify({
			version: RADAR_PROTOCOL_VERSION,
			id,
			method,
			params,
		})}\n`;
		if (Buffer.byteLength(line, "utf8") - 1 > RADAR_MAX_LINE_BYTES)
			return {
				ok: false,
				code: "bad_params",
				message: "request exceeds the control-plane line bound",
			};
		return new Promise<RadarResult<unknown>>((resolve) => {
			let settled = false;
			let socket: RadarSocket | undefined;
			let timer: RadarTimer | undefined;
			let received = 0;
			const chunks: Buffer[] = [];
			const finish = (result: RadarResult<unknown>): void => {
				if (settled) return;
				settled = true;
				if (timer !== undefined) scheduler.clearTimeout(timer);
				try {
					socket?.destroy();
				} catch {
					// A socket that will not close is already unusable.
				}
				resolve(result);
			};
			try {
				socket = dial(path);
			} catch (error) {
				finish({
					ok: false,
					code: "transport",
					message: String((error as Error)?.message ?? error),
				});
				return;
			}
			timer = scheduler.setTimeout(() => {
				finish({
					ok: false,
					code: "timeout",
					message: `${method} got no reply within ${timeoutMs}ms`,
				});
			}, timeoutMs);
			timer.unref?.();
			socket.on("error", (error) => {
				finish({
					ok: false,
					code: "transport",
					message: String((error as Error)?.message ?? error),
				});
			});
			socket.on("close", () => {
				finish({
					ok: false,
					code: "transport",
					message: `${method} connection closed without a reply`,
				});
			});
			socket.on("connect", () => {
				try {
					socket?.write(line);
				} catch (error) {
					finish({
						ok: false,
						code: "transport",
						message: String((error as Error)?.message ?? error),
					});
				}
			});
			socket.on("data", (value) => {
				const chunk = Buffer.isBuffer(value)
					? value
					: Buffer.from(String(value ?? ""), "utf8");
				received += chunk.length;
				if (received > RADAR_MAX_LINE_BYTES + 1) {
					finish(protocol("reply exceeds the control-plane line bound"));
					return;
				}
				chunks.push(chunk);
				const text = Buffer.concat(chunks).toString("utf8");
				const end = text.indexOf("\n");
				if (end < 0) return;
				finish(decodeReply(text.slice(0, end), id));
			});
		});
	}

	const socketPath = currentPath;

	const probe = async (): Promise<RadarResult<RadarHandshake>> => {
		const answer = await call("daemon.ping", {});
		if (answer.ok === false) return answer;
		const protocolVersion = numberAt(answer.value, "protocol");
		if (protocolVersion !== RADAR_PROTOCOL_VERSION)
			return {
				ok: false,
				code: "unsupported",
				message: `the daemon at ${currentPath()} speaks protocol ${
					protocolVersion ?? "unknown"
				}, not ${RADAR_PROTOCOL_VERSION}`,
			};
		const revision = numberAt(answer.value, "revision");
		if (
			revision === undefined ||
			!Number.isSafeInteger(revision) ||
			revision < RADAR_MINIMUM_REVISION
		)
			return {
				ok: false,
				code: "unsupported",
				message: `the daemon at ${currentPath()} does not support wire revision ${RADAR_MINIMUM_REVISION}; run systemctl --user restart radar-daemon`,
			};
		const backend = stringAt(answer.value, "backend");
		const instance = recordAt(answer.value, "instance");
		const rawContracts = recordAt(answer.value, "contracts");
		const rawTasks = recordAt(answer.value, "tasks");
		const taskError = recordAt(rawTasks, "error");
		const taskAvailable = recordAt(rawTasks, "available");
		if (
			backend === undefined ||
			(instance !== null && typeof instance !== "string") ||
			!Array.isArray(rawContracts) ||
			rawContracts.some((name) => typeof name !== "string") ||
			typeof taskAvailable !== "boolean" ||
			(taskError !== null && typeof taskError !== "string")
		)
			return protocol("daemon.ping reply does not match the public response schema");
		return {
			ok: true,
			value: {
				protocol: protocolVersion,
				revision,
				backend,
				instance,
				contracts: rawContracts as string[],
				tasks: { available: taskAvailable, error: taskError },
			},
		};
	};
	/** The public readiness check: a fresh handshake that also proves the registry. */
	const ping = async (): Promise<RadarResult<RadarHandshake>> => {
		const answer = await probe();
		if (answer.ok === false) return answer;
		if (!answer.value.contracts.includes(RADAR_REGISTRY_CONTRACT))
			return {
				ok: false,
				code: "unsupported",
				message: `the daemon at ${currentPath()} does not provide \`${RADAR_REGISTRY_CONTRACT}\``,
			};
		return answer;
	};
	let processRegistrationId: string | undefined;
	let handshake: Promise<RadarResult<RadarHandshake>> | undefined;
	const negotiated = async (): Promise<RadarResult<RadarHandshake>> => {
		handshake ??= probe();
		const attempt = handshake;
		try {
			const result = await attempt;
			if (result.ok === false && handshake === attempt) handshake = undefined;
			return result;
		} catch (error) {
			if (handshake === attempt) handshake = undefined;
			throw error;
		}
	};
	/** Check daemon service contracts before dispatching a dependent operation. */
	const withContract = async <T>(
		contract: string,
		operation: () => Promise<RadarResult<T>>,
	): Promise<RadarResult<T>> => {
		const ready = await negotiated();
		if (ready.ok === false) return ready;
		return ready.value.contracts.includes(contract)
			? operation()
			: {
					ok: false,
					code: "unsupported",
					message: `the daemon at ${currentPath()} does not provide \`${contract}\``,
				};
	};
	const withContracts = async <T>(
		needed: readonly string[],
		operation: () => Promise<RadarResult<T>>,
	): Promise<RadarResult<T>> => {
		const ready = await negotiated();
		if (ready.ok === false) return ready;
		const missing = needed.find((contract) => !ready.value.contracts.includes(contract));
		if (missing === undefined) return operation();
		return {
			ok: false,
			code: "unsupported",
			message: `the daemon at ${currentPath()} does not provide \`${missing}\``,
		};
	};
	const withRegistry = <T>(operation: () => Promise<RadarResult<T>>) =>
		withContract(RADAR_REGISTRY_CONTRACT, operation);
	const withRegistryChannel = <T>(
		channel: RadarChannel,
		operation: () => Promise<RadarResult<T>>,
	) => withContracts(
		[
			RADAR_REGISTRY_CONTRACT,
			...(channel === "facts" || channel === "assignment"
				? [RADAR_REGISTRY_FACTS_CONTRACT]
				: []),
		],
		operation,
	);
	/** Core mux reads/focus are methods on the negotiated protocol; the daemon
	 * reports backend_unavailable if its configured adapter cannot serve one. */
	const withDaemon = async <T>(operation: () => Promise<RadarResult<T>>) => {
		const ready = await negotiated();
		return ready.ok === false ? ready : operation();
	};

	return {
		socketPath,
		ping,
		async presence(request) {
			return withRegistry(async () => {
				const answer = await call("agent.presence", { ...request });
				if (!answer.ok) return answer;
				const decoded = decodePresenceAnswer(answer.value);
				// Presence names the caller's own registration, which is the
				// authority every subject-addressed child call is checked against.
				if (decoded.ok) processRegistrationId = decoded.value.registration_id;
				return decoded;
			});
		},
		async releasePresence(request) {
			return withRegistry(async () => {
				const answer = await call("agent.release", { ...request });
				if (!answer.ok) return answer;
				const decoded = decodePresenceAnswer(answer.value);
				if (!decoded.ok) return decoded;
				return decoded.value.standing === "released"
					? decoded
					: protocol("release reply did not confirm the claim release");
			});
		},
		async register(registration) {
			return withRegistry(async () => {
				const answer = await call("agent.register", {
					source: registration.source,
					...(registration.spawn_token !== undefined
						? { spawn_token: registration.spawn_token }
						: {}),
					incarnation: registration.incarnation,
					...(registration.session !== undefined
						? { session: registration.session }
						: {}),
					...(registration.owner !== undefined ? { owner: registration.owner } : {}),
					...(registration.run !== undefined ? { run: registration.run } : {}),
					...(registration.label !== undefined ? { label: registration.label } : {}),
					...(registration.location !== undefined ? { location: registration.location } : {}),
					...(registration.process !== undefined ? { process: registration.process } : {}),
				});
				if (answer.ok === false) return answer;
				const record = (answer.value as { registration?: unknown }).registration;
				const agentId = stringAt(record, "registration_id");
				if (!isUuid(agentId)) return protocol("registration reply carries no registration_id");
				processRegistrationId = agentId;
				return { ok: true, value: { registration_id: agentId } };
			});
		},
		async acquire(request) {
			return withRegistryChannel(request.channel, async () => {
				const assignment = request.channel === "assignment";
				if (assignment && processRegistrationId === undefined)
					return { ok: false, code: "refused", message: "assignment requires a granted parent" };
				if (assignment && request.subject_id === undefined)
					return { ok: false, code: "bad_params", message: "an assignment names a child subject" };
				const answer = await call(assignment ? "subject.assignment.acquire" : "agent.acquire", {
					...(assignment ? { registration_id: processRegistrationId, subject_id: request.subject_id } : { registration_id: request.registration_id, channel: request.channel }),
					publisher: request.publisher,
					...(request.replace !== undefined ? { replace: request.replace } : {}),
				});
				if (answer.ok === false) return answer;
				const writer = writerBinding(
					(answer.value as { writer?: unknown }).writer,
				);
				if (writer === undefined)
					return protocol("acquire reply carries no writer binding");
				return { ok: true, value: writer };
			});
		},
		async publish(request) {
			let snapshot = request.snapshot;
			if (request.channel === "facts") {
				try {
					snapshot = boundedRadarFacts(request.snapshot as RadarFactsSnapshot);
				} catch (error) {
					return {
						ok: false,
						code: "bad_params",
						message: error instanceof Error ? error.message : "invalid facts snapshot",
					};
				}
			}
			return withRegistryChannel(request.channel, async () => {
				const update = {
					writer_handle: request.writer_handle,
					sequence: request.sequence,
					...(request.lease_ms !== undefined ? { lease_ms: request.lease_ms } : {}),
					...(request.observed_at !== undefined ? { observed_at: request.observed_at } : {}),
					snapshot,
				};
				const assignment = request.channel === "assignment";
				if (assignment && processRegistrationId === undefined)
					return { ok: false, code: "refused", message: "assignment requires a granted parent" };
				if (assignment && request.subject_id === undefined)
					return { ok: false, code: "bad_params", message: "an assignment names a child subject" };
				const answer = await call(assignment ? "subject.assignment.publish" : "agent.publish", assignment
					? { registration_id: processRegistrationId, subject_id: request.subject_id, update }
					: { registration_id: request.registration_id, channel: request.channel, ...update });
				if (answer.ok === false) return answer;
				const channel = (answer.value as { channel?: unknown }).channel;
				const writer = (channel as { writer?: unknown } | undefined)?.writer;
				const sequence = numberAt(writer, "sequence");
				if (sequence === undefined) return protocol("publish reply carries no accepted sequence");
				return { ok: true, value: { sequence } };
			});
		},
		async context(request) {
			return withRegistry(async () => {
				const answer = await call("agent.context", {
					registration_id: request.registration_id,
					publisher: request.publisher,
					...(request.writer_handle !== undefined
						? { writer_handle: request.writer_handle }
						: {}),
					...(request.replace !== undefined ? { replace: request.replace } : {}),
					sequence: request.sequence,
					...(request.lease_ms !== undefined ? { lease_ms: request.lease_ms } : {}),
					...(request.observed_at !== undefined
						? { observed_at: request.observed_at }
						: {}),
					context: request.context,
				});
				if (answer.ok === false) return answer;
				const writer = writerBinding(
					(answer.value as { writer?: unknown }).writer,
				);
				if (writer === undefined)
					return protocol("context reply carries no writer binding");
				const warning = stringAt(answer.value, "warning");
				const management = managementAt(answer.value);
				return {
					ok: true,
					value: {
						writer,
						...(warning !== undefined ? { warning } : {}),
						...(management !== undefined ? { management } : {}),
					},
				};
			});
		},
		async retire(request) {
			return withRegistryChannel(request.channel, async () => {
				const assignment = request.channel === "assignment";
				if (assignment && processRegistrationId === undefined)
					return { ok: false, code: "refused", message: "assignment requires a granted parent" };
				if (assignment && request.subject_id === undefined)
					return { ok: false, code: "bad_params", message: "an assignment names a child subject" };
				const answer = await call(assignment ? "subject.assignment.retire" : "agent.retire", {
					...(assignment
						? { registration_id: processRegistrationId, subject_id: request.subject_id }
						: { registration_id: request.registration_id, channel: request.channel }),
					writer_handle: request.writer_handle,
				});
				return answer.ok === true ? { ok: true, value: undefined } : answer;
			});
		},
		async spawn(request) {
			const shape = spawnRequestShape(request);
			if (shape !== undefined)
				return { ok: false, code: "bad_params", message: shape };
			const carriesContext =
				request.cwd !== undefined ||
				(request.env !== undefined && Object.keys(request.env).length > 0);
			return withContracts(
				carriesContext
					? [RADAR_SPAWN_CONTRACT, RADAR_SPAWN_LAUNCH_CONTEXT_CONTRACT]
					: [RADAR_SPAWN_CONTRACT],
				async () => {
					const answer = await call(
						"spawn.create",
						{
							request: {
								parent: request.parent,
								parent_session_id: request.parent_session_id,
								executable: request.executable,
								argv: request.argv,
								...(request.cwd !== undefined ? { cwd: request.cwd } : {}),
								...(carriesContext && request.env !== undefined
									? { env: request.env }
									: {}),
								...(request.managed !== undefined
									? { managed: request.managed }
									: {}),
								...(request.continue_subject_id !== undefined
									? { continue_subject_id: request.continue_subject_id }
									: {}),
							},
							requester,
						},
						request.request_id,
					);
					if (answer.ok === true) return spawnEdge(recordAt(answer.value, "spawn"));
					if (DISPATCH_REFUSED_CODES.includes(answer.code)) return answer;
					// The reply is gone but the effects may not be. Read the edge this exact
					// request id authored: a create and launch the daemon completed is a spawn
					// that happened, and anything short of both stays unresolved. Resending the
					// spawn would only return this record, and a second request id is a second
					// pane and child.
					const readback = await call("spawn.get", {
						request_id: request.request_id,
					});
					if (readback.ok === false)
						return unresolvedSpawn(
							request.request_id,
							`${answer.message}; its edge could not be read back (${readback.code}: ${readback.message})`,
						);
					const recovered = spawnTopology(recordAt(readback.value, "spawn"));
					if (recovered.ok === false) return recovered;
					const recorded = recovered.value.edge;
					if (
						recorded.created === "completed" &&
						recorded.launched === "completed"
					)
						return { ok: true, value: recorded };
					return unresolvedSpawn(
						request.request_id,
						`${answer.message}; the recorded edge is created ${recorded.created} and launched ${recorded.launched}`,
					);
				},
			);
		},
		async restart(request) {
			if (!isUuid(request.request_id) || !isUuid(request.parent) || !isUuid(request.subject_id))
				return {
					ok: false,
					code: "bad_params",
					message: "restart requires canonical request, parent and subject UUIDs",
				};
			return withContract(RADAR_LIFECYCLE_RESTART_CONTRACT, async () => {
				const answer = await call(
					"lifecycle.restart",
					{
						request: {
							subject_id: request.subject_id,
							parent: request.parent,
							executable: request.executable,
							argv: request.argv,
							...(request.cwd !== undefined ? { cwd: request.cwd } : {}),
							...(request.env !== undefined ? { env: request.env } : {}),
						},
						requester,
					},
					request.request_id,
				);
				if (answer.ok === false) return answer;
				const settled = focusOutcome(recordAt(answer.value, "request"), request.request_id);
				if (settled.ok === false) return settled;
				const launch = recordAt(recordAt(answer.value, "request"), "launch_request_id");
				if (!isUuid(launch)) return protocol("lifecycle.restart reply names no launch");
				return { ok: true, value: { ...settled.value, launch_request_id: launch } };
			});
		},
		async subjectChildren(request = {}) {
			const registrationId = processRegistrationId;
			if (registrationId === undefined)
				return { ok: false, code: "refused", message: "this process has no Radar registration" };
			return withRegistry(async () => {
				const params: SubjectChildrenParams = { ...request, registration_id: registrationId };
				const answer = await call("subject.children", { ...params });
				if (!answer.ok) return answer;
				const subjects = recordAt(answer.value, "subjects");
				const next = recordAt(answer.value, "next");
				if (!Array.isArray(subjects) || (next != null && !isUuid(next)))
					return protocol("subject.children reply carries no valid page");
				for (const subject of subjects) {
					const decoded = subjectAt(subject);
					if (!decoded.ok) return decoded;
				}
				return { ok: true, value: answer.value as SubjectListResult };
			});
		},
		async childGet(subject_id) {
			const registrationId = processRegistrationId;
			if (registrationId === undefined)
				return { ok: false, code: "refused", message: "this process has no Radar registration" };
			if (!isUuid(subject_id))
				return { ok: false, code: "bad_params", message: "child requires a subject UUID" };
			return withRegistry(async () => {
				const params: SubjectChildGetParams = { registration_id: registrationId, subject_id };
				const answer = await call("subject.child.get", { ...params });
				return answer.ok ? subjectAt(recordAt(answer.value, "subject")) : answer;
			});
		},
		async childInspect(subject_id) {
			const registration_id = processRegistrationId;
			if (registration_id === undefined)
				return { ok: false, code: "refused", message: "this process has no Radar registration" };
			return withRegistry(async () => {
				const answer = await call("subject.child.inspect", { registration_id, subject_id });
				if (!answer.ok) return answer;
				const subject = subjectAt(recordAt(answer.value, "subject"));
				if (!subject.ok) return subject;
				const output = recordAt(answer.value, "output");
				const text = stringAt(output, "text");
				if (text === undefined || typeof recordAt(output, "truncated") !== "boolean")
					return protocol("child inspection has no output evidence");
				const foreground = recordAt(answer.value, "foreground");
				const activity = subject.value.execution?.snapshot?.snapshot;
				return { ok: true, value: {
					capturedAt: Date.now(), recentOutputTruncated: recordAt(output, "truncated") as boolean,
					...(text ? { recentOutput: text } : {}),
					...(foreground ? { foreground: foreground as RadarForeground } : {}),
					...(activity && "activity" in activity ? { activity: activity.activity } : {}),
				} };
			});
		},
		async spawnList(request) {
			const registrationId = processRegistrationId;
			if (registrationId === undefined)
				return { ok: false, code: "refused", message: "spawn.list requires this client's process registration" };
			return withRegistry(async () => {
				const params: SpawnListParamsDeserialize = {
					registration_id: registrationId,
					...(request.after !== undefined ? { after: request.after } : {}),
					...(request.limit !== undefined ? { limit: request.limit } : {}),
				};
				const answer = await call("spawn.list", { ...params });
				if (answer.ok === false) return answer;
				const spawns = recordAt(answer.value, "spawns");
				const next = recordAt(answer.value, "next");
				if (!Array.isArray(spawns) || (next != null && !isUuid(next)))
					return protocol("spawn.list reply carries no valid page");
				for (const spawn of spawns) {
					const decoded = spawnTopology(spawn);
					if (decoded.ok === false) return decoded;
					if (typeof recordAt(spawn, "version") !== "number" || typeof recordAt(spawn, "recorded_at") !== "string" ||
						!["unbound", "bound", "unresolved"].includes(String(recordAt(spawn, "state"))))
						return protocol("spawn.list reply carries an incomplete topology");
				}
				return { ok: true, value: answer.value as SpawnListPageSerialize };
			});
		},
		async spawnGet(requestId) {
			if (!isUuid(requestId))
				return {
					ok: false,
					code: "bad_params",
					message: "spawn.get requires a canonical request UUID",
				};
			return withRegistry(async () => {
				const answer = await call("spawn.get", { request_id: requestId });
				if (answer.ok === false) return answer;
				return spawnTopology(recordAt(answer.value, "spawn"));
			});
		},
		async agentGet(agentId) {
			if (!isUuid(agentId))
				return {
					ok: false,
					code: "bad_params",
					message: "agent.get requires a canonical agent UUID",
				};
			return withRegistry(async () => {
				const answer = await call("agent.get", { registration_id: agentId });
				if (answer.ok === false) return answer;
				return agentProcessAt(recordAt(answer.value, "agent"));
			});
		},
		async childClose(request) {
			const registration_id = processRegistrationId;
			if (registration_id === undefined)
				return { ok: false, code: "refused", message: "this process has no Radar registration" };
			if (!isUuid(request.request_id) || !isUuid(request.subject_id))
				return { ok: false, code: "bad_params", message: "child close requires canonical request and subject UUIDs" };
			return withContract(RADAR_SPAWN_CLOSE_CONTRACT, async (): Promise<RadarResult<RadarCloseOutcome>> => {
				// The caller's UUID is the envelope id: it is the durable record this
				// close is reconciled with, so a replayed call must carry it unchanged
				// rather than asking for a second close.
				const params: SubjectChildCloseParams = { registration_id, subject_id: request.subject_id, intent: request.intent };
				const answer = await call("subject.child.close", { ...params }, request.request_id);
				if (answer.ok === false) return answer;
				const close = recordAt(answer.value, "close");
				const outcome = stringAt(close, "outcome");
				const pane = stringAt(close, "pane");
				const detail = stringAt(close, "message");
				if (outcome === "completed")
					return pane !== undefined
						? { ok: true, value: { outcome, pane } }
						: protocol("child close reply does not name the closed pane");
				if (outcome === "already_exited") {
					const exit = exitAt(recordAt(close, "exit_observation"));
					return pane !== undefined && exit !== undefined
						? {
								ok: true,
								value: { outcome, pane, exit_observation: exit },
							}
						: protocol(
								"child close reply does not report the exit it cleaned up",
							);
				}
				if (outcome === "refused" || outcome === "unknown")
					return {
						ok: true,
						value: {
							outcome,
							...(pane !== undefined ? { pane } : {}),
							...(detail !== undefined ? { message: detail } : {}),
						},
					};
				return protocol("child close reply carries no close outcome");
			});
		},
		async messagePublish(request) {
			return withContract(RADAR_COMMUNICATION_CONTRACT, async () => {
				const answer = await call("communication.publish", {
					sender_registration_id: request.sender_registration_id,
					recipient_subject_id: request.recipient_subject_id,
					...(request.correlation_id !== undefined ? { correlation_id: request.correlation_id } : {}),
					summary: request.summary,
					content: request.content,
				}, request.request_id);
				if (answer.ok === false) return answer;
				const message = messageAt(recordAt(answer.value, "message"));
				const created = recordAt(answer.value, "created");
				if (message === undefined || typeof created !== "boolean")
					return protocol("communication.publish reply carries no message");
				return { ok: true, value: { message, created } };
			});
		},
		async messagePage(request) {
			return withContract(RADAR_COMMUNICATION_CONTRACT, async () => {
				const answer = await call("communication.inbox", request);
				if (answer.ok === false) return answer;
				const ids = recordAt(answer.value, "message_ids");
				const next = recordAt(answer.value, "next");
				if (!Array.isArray(ids) || ids.some((id) => !isUuid(id)))
					return protocol("communication.inbox reply carries malformed message IDs");
				if (next !== undefined && next !== null && typeof next !== "string")
					return protocol("communication.inbox reply carries a malformed cursor");
				return { ok: true, value: answer.value as MessagePageSerialize };
			});
		},
		async messageGet(request) {
			return withContract(RADAR_COMMUNICATION_CONTRACT, async () => {
				const answer = await call("communication.get", request);
				if (answer.ok === false) return answer;
				const message = messageAt(recordAt(answer.value, "message"));
				const receiptValue = recordAt(answer.value, "receipt");
				const receipt = receiptValue === null ? null : messageReceiptAt(receiptValue);
				if (message === undefined || (receiptValue !== null && receipt === undefined))
					return protocol("communication.get reply carries malformed content or receipt");
				return { ok: true, value: { message, receipt: receipt ?? null } };
			});
		},
		async messageReceive(request) {
			return withContract(RADAR_COMMUNICATION_CONTRACT, async () => {
				const answer = await call("communication.ack", request);
				if (answer.ok === false) return answer;
				const receipt = messageReceiptAt(recordAt(answer.value, "receipt"));
				const created = recordAt(answer.value, "created");
				if (receipt === undefined || typeof created !== "boolean")
					return protocol("communication.ack reply carries no receipt");
				return { ok: true, value: { receipt, created } };
			});
		},
		async watchAttention(request) {
			return withContract(RADAR_COMMUNICATION_CONTRACT, async () => {
				const path = currentPath();
				const refused = untrustedControlSocket(path, uid);
				if (refused !== undefined) return { ok: false, code: "absent", message: refused };
				const opened = await watch(path, { registration_id: request.registration_id }, request.onAttention);
				if (opened.ok === false) return opened;
				return { ok: true, value: { close: opened.value.close } };
			});
		},
		async events(request) {
			if (
				!Number.isSafeInteger(request.after) ||
				request.after < 0 ||
				!Number.isSafeInteger(request.wait_ms) ||
				request.wait_ms < 0 ||
				request.wait_ms > RADAR_EVENT_WAIT_MS
			)
				return {
					ok: false,
					code: "bad_params",
					message: "events.read requires a sequence and a wait of 0 to 1000 ms",
				};
			return withContract(RADAR_EVENTS_CONTRACT, async () => {
				const answer = await call("events.read", {
					after: request.after,
					wait_ms: request.wait_ms,
					...(request.epoch !== undefined ? { epoch: request.epoch } : {}),
				});
				if (answer.ok === false) return answer;
				const reply = eventsReplyAt(request.after, answer.value);
				return reply !== undefined
					? { ok: true, value: reply }
					: protocol("events.read reply carries no journal page or cursor_expired");
			});
		},
		async observe() {
			return withDaemon(async () => {
				const answer = await call("mux.observe", {});
				if (answer.ok === false) return answer;
				const inventory = recordAt(answer.value, "inventory");
				const workspaces = recordAt(inventory, "workspaces");
				const tabs = recordAt(inventory, "tabs");
				const panes = recordAt(inventory, "panes");
				const agents = recordAt(inventory, "agents");
				if (
					!Array.isArray(workspaces) ||
					!Array.isArray(tabs) ||
					!Array.isArray(panes) ||
					!Array.isArray(agents)
				)
					return protocol(
						"mux.observe reply carries no canonical fleet inventory",
					);
				const observed: RadarInventory = {
					workspaces: [],
					tabs: [],
					panes: [],
					agents: [],
				};
				for (const workspace of workspaces) {
					const id = stringAt(workspace, "workspace_id");
					if (id === undefined || id === "")
						return protocol("observe reply carries a malformed workspace");
					observed.workspaces.push({ workspace_id: id });
				}
				for (const tab of tabs) {
					const id = stringAt(tab, "tab_id");
					const workspaceId = stringAt(tab, "workspace_id");
					if (!id || !workspaceId)
						return protocol("observe reply carries a malformed tab");
					observed.tabs.push({ tab_id: id, workspace_id: workspaceId });
				}
				for (const pane of panes) {
					const decoded = observedPane(pane);
					if (decoded.ok === false) return decoded;
					observed.panes.push(decoded.value);
				}
				for (const agent of agents) {
					const decoded = observedAgent(agent);
					if (decoded.ok === false) return decoded;
					observed.agents.push(decoded.value);
				}
				return { ok: true, value: observed };
			});
		},
		async output(request) {
			return withDaemon(async () => {
				const answer = await call("mux.output", request);
				if (answer.ok === false) return answer;
				const read = recordAt(answer.value, "output");
				const text = stringAt(read, "text");
				const truncated = recordAt(read, "truncated");
				if (text === undefined || typeof truncated !== "boolean")
					return protocol("mux.output reply carries no output read");
				const revision = recordAt(read, "revision");
				return {
					ok: true,
					value: {
						text,
						truncated,
						...(typeof revision === "number" ? { revision } : {}),
					},
				};
			});
		},
		async processInfo(request) {
			return withDaemon(async (): Promise<RadarResult<RadarForeground>> => {
				const answer = await call("mux.process_info", request);
				if (answer.ok === false) return answer;
				const evidence = recordAt(answer.value, "evidence");
				const kind = stringAt(evidence, "kind");
				if (kind === "shell") return { ok: true, value: { kind: "shell" } };
				if (kind === "inconclusive")
					return { ok: true, value: { kind: "inconclusive" } };
				const pid = recordAt(evidence, "pid");
				if (kind === "non_shell" && typeof pid === "number")
					return {
						ok: true,
						value: {
							kind: "non_shell",
							pid,
							...(stringAt(evidence, "name") !== undefined
								? { name: stringAt(evidence, "name") }
								: {}),
							...(stringAt(evidence, "command") !== undefined
								? { command: stringAt(evidence, "command") }
								: {}),
						},
					};
				return protocol("mux.process_info reply carries no foreground evidence");
			});
		},
		async focus(request) {
			if (!isUuid(request.request_id))
				return {
					ok: false,
					code: "bad_params",
					message: "focus requires a canonical operation UUID",
				};
			return withDaemon(async () => {
				const answer = await call(
					"mux.focus",
					{
						target: request.pane_id,
						target_kind: "pane",
						requester,
					},
					request.request_id,
				);
				if (answer.ok === true)
					return focusOutcome(
						recordAt(answer.value, "request"),
						request.request_id,
					);
				if (DISPATCH_REFUSED_CODES.includes(answer.code)) return answer;
				// The reply is gone but the request may still have been recorded and
				// dispatched, so read that exact durable record back. Resending the same
				// id would only return the record, and the direct adapter would be a
				// second effect.
				const readback = await call("mux.request.get", {
					id: request.request_id,
				});
				if (readback.ok === false)
					return unresolvedOperation(
						request.request_id,
						readback.code === "not_found"
							? `${answer.message}; the daemon holds no record of it`
							: `${answer.message}; its record could not be read back (${readback.code}: ${readback.message})`,
					);
				return focusOutcome(
					recordAt(readback.value, "request"),
					request.request_id,
				);
			});
		},
	};
}

/** The daemon's maximum facts-channel task list. */
export const RADAR_MAX_BACKGROUND_TASKS = 64;
/** The pinned `MAX_AWAITED` bound for one assignment snapshot. */
export const RADAR_MAX_AWAITED = 64;

export function boundedRadarFacts(facts: RadarFactsSnapshot): RadarFactsSnapshot {
	const allowed = new Set([
		"model", "provider", "thinking", "context_window", "session_name",
		"background_tasks", "background_started_unix_ms",
	]);
	const unknown = Object.keys(facts).find((key) => !allowed.has(key));
	if (unknown !== undefined) throw new RangeError(`unknown facts field ${unknown}`);
	const text = (value: string): string => {
		if (typeof value !== "string") throw new RangeError("facts fields must be strings");
		return bounded(value, 1024);
	};
	const contextWindow = facts.context_window;
	if (contextWindow !== undefined && contextWindow !== null) {
		if (
			typeof contextWindow !== "object" ||
			Object.keys(contextWindow).sort().join(",") !== "tokens,window" ||
			!Number.isSafeInteger(contextWindow.tokens) ||
			contextWindow.tokens < 0 ||
			!Number.isSafeInteger(contextWindow.window) ||
			contextWindow.window <= 0 ||
			contextWindow.tokens > contextWindow.window
		)
			throw new RangeError("context_window requires integer tokens within a positive window");
	}
	const backgroundTasks = facts.background_tasks;
	if (
		backgroundTasks !== undefined &&
		(!Array.isArray(backgroundTasks) || backgroundTasks.length > RADAR_MAX_BACKGROUND_TASKS)
	)
		throw new RangeError(`background_tasks exceeds ${RADAR_MAX_BACKGROUND_TASKS} valid entries`);
	const taskIds = new Set<string>();
	const boundedTasks = backgroundTasks?.map((task) => {
		if (
			typeof task !== "object" || task === null || Array.isArray(task) ||
			Object.keys(task).sort().join(",") !== "id,phase" ||
			typeof task.id !== "string" || !task.id ||
			typeof task.phase !== "string" || !task.phase
		)
			throw new RangeError("background_tasks requires non-empty id and phase fields");
		const id = text(task.id);
		if (taskIds.has(id)) throw new RangeError("background_tasks ids must be unique");
		taskIds.add(id);
		return { id, phase: text(task.phase) };
	});
	const started = facts.background_started_unix_ms;
	if (started !== undefined && started !== null && (!Number.isSafeInteger(started) || started < 0))
		throw new RangeError("background_started_unix_ms must be a non-negative integer");
	const optionalText = (value: string | null | undefined) =>
		value === undefined ? undefined : value === null ? null : text(value);
	return {
		...(facts.model !== undefined ? { model: optionalText(facts.model) } : {}),
		...(facts.provider !== undefined ? { provider: optionalText(facts.provider) } : {}),
		...(facts.thinking !== undefined ? { thinking: optionalText(facts.thinking) } : {}),
		...(contextWindow !== undefined
			? {
					context_window:
						contextWindow === null
							? null
							: { tokens: contextWindow.tokens, window: contextWindow.window },
				}
			: {}),
		...(facts.session_name !== undefined
			? { session_name: optionalText(facts.session_name) }
			: {}),
		...(boundedTasks !== undefined ? { background_tasks: boundedTasks } : {}),
		...(started !== undefined ? { background_started_unix_ms: started } : {}),
	};
}

export function boundedRadarAssignment(
	snapshot: RadarAssignmentSnapshot,
): RadarAssignmentSnapshot {
	const base = boundedRadarSnapshot(snapshot);
	return {
		...base,
		...(snapshot.request !== undefined
			? { request: bounded(snapshot.request, 1024) }
			: {}),
		...(snapshot.awaited !== undefined
			? {
					awaited: snapshot.awaited
						.slice(0, RADAR_MAX_AWAITED)
						.map((item) => bounded(item, 1024)),
				}
			: {}),
	};
}

export function boundedRadarSnapshot(snapshot: RadarSnapshot): RadarSnapshot {
	const text = (value: string): string => bounded(value, 1024);
	return {
		activity: text(snapshot.activity),
		...(snapshot.waiting_reason !== undefined
			? { waiting_reason: text(snapshot.waiting_reason) }
			: {}),
		...(snapshot.last_outcome !== undefined
			? {
					last_outcome: {
						result: text(snapshot.last_outcome.result),
						...(snapshot.last_outcome.detail !== undefined
							? { detail: text(snapshot.last_outcome.detail) }
							: {}),
					},
				}
			: {}),
	};
}
