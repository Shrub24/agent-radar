import { createHash, randomUUID } from "node:crypto";
import {
	closeSync,
	constants,
	fstatSync,
	fsyncSync,
	lstatSync,
	mkdirSync,
	openSync,
	readFileSync,
	renameSync,
	unlinkSync,
	writeSync,
} from "node:fs";
import { dirname, isAbsolute, join, parse, resolve, sep } from "node:path";
import type { AttentionSerialize } from "./generated-contracts.ts";
import type { RadarLocationResolution } from "./radar-location.ts";
import {
	boundedRadarAssignment,
	boundedRadarFacts,
	boundedRadarSnapshot,
	isUuid,
	RADAR_MAX_AWAITED,
	RADAR_MAX_BACKGROUND_TASKS,
	type RadarAssignmentSnapshot,
	type RadarChannel,
	type RadarChannelSnapshot,
	type RadarFactsSnapshot,
	type RadarClient,
	type RadarLocation,
	type RadarManagement,
	type RadarPublisherIdentity,
	type RadarRegistration,
	type RadarScheduler,
	type RadarSnapshot,
	type RadarTimer,
	systemScheduler,
} from "./radar-client.ts";

/** A rejected session claim requires a fresh Pi session load before reconsideration. */
const DUPLICATE_SESSION_DIAGNOSTIC =
	"Radar rejected this session claim as a duplicate; reload or resume the session before retrying";

/** The client members publication calls; consumers narrow to this rather than the whole client. */
export type RadarPublicationClient = Pick<
	RadarClient,
	"ping" | "presence" | "releasePresence" | "acquire" | "publish" | "retire"
>;

/**
 * Durable publication state for Radar's registry (`agent-radar`
 * `docs/agent-registration.md`, contract version 1): this process's immutable
 * registration record, the acquired writer binding per channel, the accepted
 * sequence and the exact unresolved request.
 *
 * Everything here is best-effort and asynchronous. An absent daemon, a lost
 * reply, a fenced writer or a timeout may not delay or alter a turn, an
 * assignment, a result or a settlement, so every entry point returns void and a
 * channel's single diagnostic is readable rather than thrown.
 */
export const RADAR_PUBLICATION_VERSION = 1 as const;
export const RADAR_LEASE_MS = 30_000;
export const RADAR_HEARTBEAT_MS = 15_000;

/** The kernel tuple that identifies one process incarnation. */
export type ProcessBirth = { boot_id: string; pid: number; start_ticks: number };

/**
 * The start tick out of `/proc/<pid>/stat`. The command name sits in parentheses
 * and may itself contain spaces and parentheses, so the parse starts after the
 * last `)` instead of splitting the whole line. Fields after the command name
 * begin at `state`, so `starttime` (field 22) is the twentieth of them.
 */
export function parseProcStat(
	text: string,
): { pid: number; start_ticks: number } | undefined {
	const close = text.lastIndexOf(")");
	const separator = text.indexOf(" ");
	if (close <= 0 || separator <= 0 || separator > close) return undefined;
	const pid = Number(text.slice(0, separator));
	const fields = text.slice(close + 1).trim().split(/\s+/);
	const startTicks = Number(fields[19]);
	if (!Number.isInteger(pid) || pid <= 0) return undefined;
	if (!Number.isInteger(startTicks) || startTicks <= 0) return undefined;
	return { pid, start_ticks: startTicks };
}

/**
 * This process's claimed birth identity, or `undefined` where the platform does
 * not report one. Unreadable identity stays omitted rather than guessed: the
 * claim is verifier evidence, never the source of subject identity, and the
 * daemon reports an omitted claim as `unavailable`, not `absent`.
 */
export function readProcessBirth(): ProcessBirth | undefined {
	try {
		const bootId = readFileSync("/proc/sys/kernel/random/boot_id", "utf8").trim();
		if (!/^[0-9a-f-]{8,64}$/i.test(bootId)) return undefined;
		const stat = parseProcStat(readFileSync("/proc/self/stat", "utf8"));
		return stat === undefined
			? undefined
			: { boot_id: bootId, pid: stat.pid, start_ticks: stat.start_ticks };
	} catch {
		return undefined;
	}
}

const RECORD_NAME = /^(?:registrations|presence|subjects|channels|context|bindings)\/[a-z0-9-]{1,64}\.json$/;
const MAX_RECORD_BYTES = 64 * 1024;
const PRIVATE_MODE = 0o700;
const RECORD_MODE = 0o600;
const PROCESS_SLOT = Symbol.for("pi-radar.radar.process-slot.v1");

/**
 * Private, atomic, bounded record storage. One root is one owner's state: files
 * are `0600` inside `0700` directories, replaced through a sibling temporary so
 * a reader never sees a partial record, and read only as bounded regular
 * non-symlink files.
 */
export type PublicationStore = {
	path(name: string): string;
	read(name: string): unknown;
	write(name: string, value: unknown): void;
	remove(name: string): void;
};

export function createPublicationStore(root: string): PublicationStore {
	const uid = process.getuid?.();
	if (uid === undefined) throw new Error("private Radar storage requires an effective uid");
	const ensureDirectory = (directory: string): void => {
		let existed = true;
		try {
			lstatSync(directory);
		} catch (error) {
			if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
			existed = false;
		}
		mkdirSync(directory, { recursive: true, mode: PRIVATE_MODE });
		const absolute = resolve(directory);
		const rootPath = parse(absolute).root;
		let current = rootPath;
		for (const part of absolute.slice(rootPath.length).split(/[\\/]+/u).filter(Boolean)) {
			current = join(current, part);
			const info = lstatSync(current);
			if (info.isSymbolicLink() || !info.isDirectory())
				throw new Error(`unsafe Radar storage directory: ${current}`);
		}
		const info = lstatSync(absolute);
		if (info.uid !== uid) throw new Error(`Radar storage directory has wrong owner: ${absolute}`);
		if (existed && (info.mode & 0o777) !== PRIVATE_MODE)
			throw new Error(`Radar storage directory is not mode 0700: ${absolute}`);
	};
	const pathOf = (name: string): string => {
		if (!RECORD_NAME.test(name))
			throw new Error(`invalid radar record name: ${name}`);
		return join(root, name);
	};
	const ensureTrustedDirectory = (directory: string): void => {
		const absoluteRoot = resolve(root);
		const absoluteDirectory = resolve(directory);
		if (absoluteDirectory !== absoluteRoot && !absoluteDirectory.startsWith(`${absoluteRoot}${sep}`))
			throw new Error(`unsafe Radar storage directory: ${directory}`);
		const rootInfo = lstatSync(absoluteRoot);
		if (
			rootInfo.isSymbolicLink() ||
			!rootInfo.isDirectory() ||
			rootInfo.uid !== uid ||
			(rootInfo.mode & 0o777) !== PRIVATE_MODE
		)
			throw new Error(`unsafe Radar storage directory: ${absoluteRoot}`);
		if (absoluteDirectory !== absoluteRoot) {
			const info = lstatSync(absoluteDirectory);
			if (
				info.isSymbolicLink() ||
				!info.isDirectory() ||
				info.uid !== uid ||
				(info.mode & 0o777) !== PRIVATE_MODE
			)
				throw new Error(`unsafe Radar storage directory: ${absoluteDirectory}`);
		}
	};
	ensureDirectory(root);
	return {
		path: pathOf,
		read(name) {
			const path = pathOf(name);
			const directory = dirname(path);
			try {
				ensureTrustedDirectory(directory);
			} catch (error) {
				if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
				throw error;
			}
			let descriptor: number | undefined;
			try {
				descriptor = openSync(
					path,
					constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK,
				);
				const info = fstatSync(descriptor);
				if (!info.isFile() || info.uid !== uid || (info.mode & 0o777) !== RECORD_MODE)
					throw new Error(`unsafe Radar record: ${path}`);
				if (info.size > MAX_RECORD_BYTES)
					throw new Error(`oversized Radar record: ${path}`);
				const contents = readFileSync(descriptor, { encoding: "utf8" });
				if (Buffer.byteLength(contents, "utf8") > MAX_RECORD_BYTES)
					throw new Error(`oversized Radar record: ${path}`);
				return JSON.parse(contents) as unknown;
			} catch (error) {
				if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
				throw error;
			} finally {
				if (descriptor !== undefined) closeSync(descriptor);
			}
		},
		write(name, value) {
		const path = pathOf(name);
			const directory = dirname(path);
			ensureDirectory(directory);
			const temporary = join(
				directory,
				`.${name.slice(name.lastIndexOf("/") + 1)}.${randomUUID()}.tmp`,
			);
			const bytes = Buffer.from(JSON.stringify(value), "utf8");
			if (bytes.length > MAX_RECORD_BYTES)
				throw new Error(`Radar record exceeds ${MAX_RECORD_BYTES} bytes`);
			let descriptor: number | undefined;
			try {
				descriptor = openSync(temporary, "wx", RECORD_MODE);
				let offset = 0;
				while (offset < bytes.length)
					offset += writeSync(descriptor, bytes, offset, bytes.length - offset);
				fsyncSync(descriptor);
				closeSync(descriptor);
				descriptor = undefined;
				const info = lstatSync(directory);
				if (info.isSymbolicLink() || !info.isDirectory() || info.uid !== uid || (info.mode & 0o777) !== PRIVATE_MODE)
					throw new Error(`unsafe Radar storage directory: ${directory}`);
				renameSync(temporary, path);
				const directoryFd = openSync(directory, constants.O_RDONLY);
				try {
					fsyncSync(directoryFd);
				} finally {
					closeSync(directoryFd);
				}
			} finally {
				if (descriptor !== undefined) closeSync(descriptor);
				try {
					unlinkSync(temporary);
				} catch (error) {
					if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
				}
			}
		},
		remove(name) {
			try {
				unlinkSync(pathOf(name));
			} catch (error) {
				if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
			}
		},
	};
}

/**
 * A Pi extension can be reloaded without ending its Node process. This slot is
 * the process-lifetime seam: a reload reuses the incarnation, the registration
 * and the publisher binding it holds, while another OS process starts with an
 * empty slot, a fresh UUID and therefore a distinct subject — even when it
 * resumes the same Pi session. Birth identity is never part of this choice.
 */
export type RadarProcessSlot = {
	incarnation?: string;
	launchRegistrationId?: string;
	publications: Map<string, RadarPublication>;
};

export function createRadarProcessSlot(): RadarProcessSlot {
	return { publications: new Map() };
}

export function radarProcessSlot(): RadarProcessSlot {
	const host = globalThis as typeof globalThis & {
		[PROCESS_SLOT]?: RadarProcessSlot;
	};
	if (!host[PROCESS_SLOT]) {
		const slot = createRadarProcessSlot();
		slot.launchRegistrationId = process.env.PI_RADAR_REGISTRATION_ID;
		delete process.env.PI_RADAR_REGISTRATION_ID;
		host[PROCESS_SLOT] = slot;
	}
	return host[PROCESS_SLOT];
}

/** The process incarnation, created on first use and kept for the process. */
export function radarProcessIncarnation(
	slot: RadarProcessSlot = radarProcessSlot(),
): string {
	return (slot.incarnation ??= randomUUID());
}

/**
 * The sidecar key a managed child and its owner both derive from the same
 * durable managed tuple. A relaunch is a new run and therefore a new key, so a
 * new subject never overwrites the binding of the process it replaced.
 */
export function radarBindingKey(run: string, owner: string, label: string): string {
	return createHash("sha256")
		.update(`${run}\0${owner}\0${label}`)
		.digest("hex")
		.slice(0, 32);
}

type PendingPublish = {
	sequence: number;
	snapshot: RadarChannelSnapshot;
	lease_ms: number;
	observed_at: string;
};

type ChannelRecord = {
	version: typeof RADAR_PUBLICATION_VERSION;
	target_id: string;
	channel: RadarChannel;
	publisher: RadarPublisherIdentity;
	writer?: { handle: string; generation: number };
	retired_at?: string;
	sequence: number;
	accepted?: PendingPublish & { sent_at: number };
	pending?: PendingPublish;
	diagnostic?: string;
};

/** One current-session report, before or after the daemon accepted it. */
type ContextReport = {
	sequence: number;
	session: string | null;
	lease_ms: number;
	observed_at: string;
};

/**
 * The persisted state of one process's current-session record. It is separate
 * from the channel records because the daemon keeps it in its own per-subject
 * file, with its own writer binding and sequence.
 */
type ContextRecord = {
	version: typeof RADAR_PUBLICATION_VERSION;
	registration_id: string;
	publisher: RadarPublisherIdentity;
	writer?: { handle: string; generation: number };
	sequence: number;
	accepted?: ContextReport & { sent_at: number };
	pending?: ContextReport;
	diagnostic?: string;
};

/** What a managed child tells its owner: which subject to address, once granted. */
export type RadarChildBinding = {
	version: typeof RADAR_PUBLICATION_VERSION;
	registration_id: string;
	subject_id?: string;
	incarnation: string;
	run: string;
	owner: string;
	label: string;
	observed_at: string;
};

function recordAt(value: unknown, key: string): unknown {
	if (typeof value !== "object" || value === null) return undefined;
	return (value as Record<string, unknown>)[key];
}

function textAt(value: unknown, key: string): string | undefined {
	const found = recordAt(value, key);
	return typeof found === "string" ? found : undefined;
}

function numberAt(value: unknown, key: string): number | undefined {
	const found = recordAt(value, key);
	return typeof found === "number" && Number.isFinite(found) ? found : undefined;
}

const PENDING_KEYS = "lease_ms,observed_at,sequence,snapshot";
const ACCEPTED_KEYS = "lease_ms,observed_at,sequence,sent_at,snapshot";
const SNAPSHOT_KEYS = "activity,last_outcome,waiting_reason";
const OUTCOME_KEYS = "detail,result";
const OUTCOME_KEYS_WITHOUT_DETAIL = "result";
const WRITER_KEYS = "generation,handle";

function exactKeys(value: unknown, expected: string): boolean {
	return (
		typeof value === "object" &&
		value !== null &&
		!Array.isArray(value) &&
		Object.keys(value as Record<string, unknown>).sort().join(",") === expected
	);
}

const SNAPSHOT_KEY_LIST = ["activity", "last_outcome", "waiting_reason"];
const ASSIGNMENT_KEY_LIST = [...SNAPSHOT_KEY_LIST, "request", "awaited"];
const FACTS_KEY_LIST = [
	"model",
	"provider",
	"thinking",
	"context_window",
	"session_name",
	"background_tasks",
	"background_started_unix_ms",
];

function snapshotAt(
	value: unknown,
	keys: readonly string[] = SNAPSHOT_KEY_LIST,
): RadarSnapshot | undefined {
	if (
		typeof value !== "object" ||
		value === null ||
		Array.isArray(value) ||
		!Object.keys(value as Record<string, unknown>)
			.every((key) => keys.includes(key))
	)
		return undefined;
	const activity = textAt(value, "activity");
	if (activity === undefined) return undefined;
	const snapshot: RadarSnapshot = { activity };
	const reason = textAt(value, "waiting_reason");
	if (reason !== undefined) snapshot.waiting_reason = reason;
	const outcome = recordAt(value, "last_outcome");
	if (outcome !== undefined) {
		if (
			!exactKeys(outcome, OUTCOME_KEYS) &&
			!exactKeys(outcome, OUTCOME_KEYS_WITHOUT_DETAIL)
		)
			return undefined;
		const result = textAt(outcome, "result");
		if (result === undefined) return undefined;
		const detail = textAt(outcome, "detail");
		snapshot.last_outcome = {
			result,
			...(detail !== undefined ? { detail } : {}),
		};
	}
	return snapshot;
}

/**
 * Per-channel snapshot codec. Execution keeps its own parse and bound; the
 * assignment and facts channels add their fields without changing how an
 * execution record is read back.
 */
type ChannelCodec = {
	parse(value: unknown): RadarChannelSnapshot | undefined;
	bound(snapshot: RadarChannelSnapshot): RadarChannelSnapshot;
};

const executionCodec: ChannelCodec = {
	parse: (value) => snapshotAt(value),
	bound: (snapshot) => boundedRadarSnapshot(snapshot as RadarSnapshot),
};

function assignmentAt(value: unknown): RadarAssignmentSnapshot | undefined {
	const base = snapshotAt(value, ASSIGNMENT_KEY_LIST);
	if (base === undefined) return undefined;
	const request = textAt(value, "request");
	const awaitedValue = recordAt(value, "awaited");
	if (
		awaitedValue !== undefined &&
		(!Array.isArray(awaitedValue) ||
			awaitedValue.length > RADAR_MAX_AWAITED ||
			!awaitedValue.every((item) => typeof item === "string"))
	)
		return undefined;
	if (
		recordAt(value, "request") !== undefined &&
		request === undefined
	)
		return undefined;
	return {
		...base,
		...(request !== undefined ? { request } : {}),
		...(awaitedValue !== undefined
			? { awaited: awaitedValue as string[] }
			: {}),
	};
}

const assignmentCodec: ChannelCodec = {
	parse: (value) => assignmentAt(value),
	bound: (snapshot) =>
		boundedRadarAssignment(snapshot as RadarAssignmentSnapshot),
};

function factsAt(value: unknown): RadarFactsSnapshot | undefined {
	if (typeof value !== "object" || value === null || Array.isArray(value))
		return undefined;
	const record = value as Record<string, unknown>;
	if (!Object.keys(record).every((key) => FACTS_KEY_LIST.includes(key)))
		return undefined;
	const facts: Record<string, unknown> = {};
	for (const key of FACTS_KEY_LIST) {
		const found = record[key];
		if (found === undefined) continue;
		if (key === "context_window") {
			if (typeof found !== "object" || found === null || Array.isArray(found))
				return undefined;
			const window = found as Record<string, unknown>;
			if (Object.keys(window).sort().join(",") !== "tokens,window") return undefined;
			const tokens = window.tokens;
			const size = window.window;
			if (
				!Number.isSafeInteger(tokens) ||
				(tokens as number) < 0 ||
				!Number.isSafeInteger(size) ||
				(size as number) <= 0 ||
				(tokens as number) > (size as number)
			)
				return undefined;
			facts[key] = { tokens, window: size };
			continue;
		}
		if (key === "background_tasks") {
			if (!Array.isArray(found) || found.length > RADAR_MAX_BACKGROUND_TASKS)
				return undefined;
			const ids = new Set<string>();
			const tasks: { id: string; phase: string }[] = [];
			for (const item of found) {
				if (typeof item !== "object" || item === null || Array.isArray(item))
					return undefined;
				const task = item as Record<string, unknown>;
				if (
					Object.keys(task).sort().join(",") !== "id,phase" ||
					typeof task.id !== "string" || !task.id ||
					typeof task.phase !== "string" || !task.phase ||
					ids.has(task.id)
				)
					return undefined;
				ids.add(task.id);
				tasks.push({ id: task.id, phase: task.phase });
			}
			facts[key] = tasks;
			continue;
		}
		if (key === "background_started_unix_ms") {
			if (!Number.isSafeInteger(found) || (found as number) < 0) return undefined;
			facts[key] = found;
			continue;
		}
		if (typeof found !== "string") return undefined;
		facts[key] = found;
	}
	return facts as RadarFactsSnapshot;
}

const factsCodec: ChannelCodec = {
	parse: (value) => factsAt(value),
	bound: (snapshot) => boundedRadarFacts(snapshot as RadarFactsSnapshot),
};

function pendingAt(
	value: unknown,
	keys = PENDING_KEYS,
	codec: ChannelCodec = executionCodec,
): PendingPublish | undefined {
	if (!exactKeys(value, keys)) return undefined;
	const sequence = numberAt(value, "sequence");
	const lease = numberAt(value, "lease_ms");
	const observed = textAt(value, "observed_at");
	const snapshot = codec.parse(recordAt(value, "snapshot"));
	if (sequence === undefined || sequence < 1) return undefined;
	if (lease === undefined || observed === undefined || snapshot === undefined)
		return undefined;
	return { sequence, snapshot, lease_ms: lease, observed_at: observed };
}


export type RadarFactsWriter = Omit<RadarChannelWriter, "update"> & {
	update(snapshot: RadarFactsSnapshot): void;
	/**
	 * The mailbox hint an accepted `agent.publish` reply carried, if any. An
	 * unmanaged process has no mailbox, so absence is normal and nothing fires.
	 */
	onAttention(listener: (attention: AttentionSerialize) => void): () => void;
};

export type RadarChannelWriter = {
	/** Publish the newest complete snapshot; never throws, never awaits a daemon. */
	update(snapshot: RadarChannelSnapshot): void;
	/** Best-effort retirement of this writer's right to update the channel. */
	retire(): void;
	/** Release this writer's timers. Facts already published stay. */
	stop(): void;
	/** Re-arm a stopped writer after an extension reload. */
	reopen(): void;
	diagnostic(): string | undefined;
};

type ChannelWriterInput = {
	store: PublicationStore;
	client: RadarPublicationClient;
	scheduler: RadarScheduler;
	recordName: string;
	channel: RadarChannel;
	publisher: RadarPublisherIdentity;
	codec?: ChannelCodec;
	/**
	 * The exact subject this channel reports about. `recheck` forgets a cached
	 * subject so the identical immutable content is registered again, which is
	 * what a daemon that lost its state root needs.
	 */
	subject(recheck: boolean): Promise<string | undefined>;
	leaseMs: number;
	heartbeatMs: number;
};

/** A writer whose accepted publish replies may carry a mailbox hint. */
type AttentionWriter = RadarChannelWriter & Pick<RadarFactsWriter, "onAttention">;

function createChannelWriter(input: ChannelWriterInput): AttentionWriter {
	const { store, client, scheduler, recordName, channel, publisher } = input;
	const codec = input.codec ?? executionCodec;
	let record = loadRecord();
	let desired: RadarChannelSnapshot | undefined;
	let running = false;
	let again = false;
	let recovering = false;
	let stopped = false;
	let fenced = false;
	let retired = false;
	let pump: RadarTimer | undefined;

	function blank(agentId: string): ChannelRecord {
		return {
			version: RADAR_PUBLICATION_VERSION,
			target_id: agentId,
			channel,
			publisher,
			sequence: 0,
		};
	}

	/** An assignment names a child subject; every other channel names a process. */
	function addressed(target: string): { registration_id: string } | { subject_id: string } {
		return channel === "assignment" ? { subject_id: target } : { registration_id: target };
	}

	function loadRecord(): ChannelRecord {
		const value = store.read(recordName);
		const agentId = textAt(value, "target_id");
		const storedPublisher = recordAt(value, "publisher");
		// A record written for another channel or publisher is not this writer's
		// binding: adopting it would publish under someone else's identity.
		if (
			numberAt(value, "version") !== RADAR_PUBLICATION_VERSION ||
			agentId === undefined ||
			!isUuid(agentId) ||
			textAt(value, "channel") !== channel ||
			textAt(storedPublisher, "source") !== publisher.source ||
			textAt(storedPublisher, "incarnation") !== publisher.incarnation ||
			textAt(storedPublisher, "reporting_owner") !== publisher.reporting_owner
		)
			return blank("");
		const writer = recordAt(value, "writer");
		const handle = textAt(writer, "handle");
		const generation = numberAt(writer, "generation");
		if (
			writer !== undefined &&
			(!exactKeys(writer, WRITER_KEYS) || !isUuid(handle) || generation === undefined)
		)
			return blank(agentId);
		const pending = pendingAt(recordAt(value, "pending"), PENDING_KEYS, codec);
		const acceptedValue = recordAt(value, "accepted");
		const accepted = pendingAt(acceptedValue, ACCEPTED_KEYS, codec);
		const sentAt = numberAt(acceptedValue, "sent_at");
		return {
			version: RADAR_PUBLICATION_VERSION,
			target_id: agentId,
			channel,
			publisher,
			...(handle !== undefined && generation !== undefined
				? { writer: { handle, generation } }
				: {}),
			...(textAt(value, "retired_at") !== undefined
				? { retired_at: textAt(value, "retired_at") }
				: {}),
			sequence: numberAt(value, "sequence") ?? 0,
			...(pending !== undefined ? { pending } : {}),
			...(accepted !== undefined && sentAt !== undefined
				? { accepted: { ...accepted, sent_at: sentAt } }
				: {}),
			...(textAt(value, "diagnostic") !== undefined
				? { diagnostic: textAt(value, "diagnostic") }
				: {}),
		};
	}

	/**
	 * One bounded diagnostic per channel. A diagnostic stream would be a second
	 * channel, and the first cause is the one a reader has to reconcile.
	 */
	function note(message: string): void {
		if (record.diagnostic !== undefined) return;
		record.diagnostic = message.slice(0, 256);
		save();
	}

	function save(): void {
		store.write(recordName, record);
	}

	function sameSnapshot(
		left: RadarChannelSnapshot,
		right: RadarChannelSnapshot,
	): boolean {
		return (
			JSON.stringify(codec.bound(left)) ===
			JSON.stringify(codec.bound(right))
		);
	}

	function fence(message: string): void {
		fenced = true;
		stopPump();
		note(`channel ${channel} stopped: ${message}`);
	}

	function startPump(): void {
		if (pump !== undefined || stopped || fenced || retired) return;
		pump = scheduler.setInterval(() => kick(), input.heartbeatMs);
		pump.unref?.();
	}

	function stopPump(): void {
		if (pump === undefined) return;
		scheduler.clearInterval(pump);
		pump = undefined;
	}

	async function ensureWriter(): Promise<boolean> {
		if (record.writer !== undefined) return true;
		// A subject the daemon no longer holds is registered again from the
		// identical immutable content that was persisted with it.
		const agentId = await input.subject(recovering);
		if (agentId === undefined) return false;
		if (record.target_id !== agentId) {
			// A different subject incarnation is a different channel: the previous
			// binding, sequence and unresolved request do not apply to it, so the
			// next drain publishes a complete snapshot instead of replaying one.
			record = blank(agentId);
			save();
		}
		const answer = await client.acquire({...addressed(agentId), channel, publisher});
		if (answer.ok === false) {
			if (answer.code === "not_found") {
				// The daemon has lost this subject. The identical immutable content is
				// registered again on the next heartbeat rather than in a hot retry
				// loop, so a daemon that never recovers is still asked once per lease.
				recovering = true;
				return false;
			}
			if (answer.code === "refused" || answer.code === "bad_params")
				// A refusal here is a real disagreement about writer ownership.
				// Replacement stays explicit: this publisher never takes over a
				// binding it did not observe.
				fence(answer.message);
			return false;
		}
		recovering = false;
		record.writer = {
			handle: answer.value.handle,
			generation: answer.value.generation,
		};
		record.sequence = Math.max(record.sequence, answer.value.sequence);
		save();
		startPump();
		return true;
	}

	function heartbeatDue(): boolean {
		return (
			record.accepted !== undefined &&
			scheduler.now() - record.accepted.sent_at >= input.heartbeatMs
		);
	}

	async function send(pending: PendingPublish): Promise<void> {
		const writer = record.writer;
		if (writer === undefined) return;
		const answer = await client.publish({...addressed(record.target_id), channel,
			writer_handle: writer.handle,
			sequence: pending.sequence,
			lease_ms: pending.lease_ms,
			observed_at: pending.observed_at,
			snapshot: pending.snapshot,});
		if (answer.ok === true) {
			recovering = false;
			record.sequence = Math.max(record.sequence, answer.value.sequence);
			if (answer.value.attention !== undefined)
				for (const listener of attention) listener(answer.value.attention);
			record.accepted = { ...pending, sent_at: scheduler.now() };
			record.pending = undefined;
			save();
			return;
		}
		if (answer.code === "refused" || answer.code === "bad_params") {
			fence(answer.message);
			return;
		}
		if (answer.code === "not_found") {
			// The subject or channel is gone. The identity cannot be republished
			// against, so the binding is dropped: the next heartbeat re-registers the
			// same immutable content, and a new subject gets a complete snapshot
			// rather than an unresolved request that belonged to the old one.
			record.writer = undefined;
			recovering = true;
			save();
			return;
		}
		// A timeout, a closed connection or a lost reply leaves the exact request
		// persisted, so the retry replays it byte for byte and the lease does not
		// silently advance.
	}

	async function stage(snapshot: RadarChannelSnapshot): Promise<void> {
		const pending: PendingPublish = {
			sequence: record.sequence + 1,
			snapshot: codec.bound(snapshot),
			lease_ms: input.leaseMs,
			observed_at: new Date(scheduler.now()).toISOString(),
		};
		// Persisted before it is sent, so a lost reply replays this exact request.
		record.pending = pending;
		save();
		await send(pending);
	}

	async function drainOnce(): Promise<void> {
		if (fenced || retired) return;
		if (!(await ensureWriter())) return;
		if (fenced) return;
		if (record.pending !== undefined) {
			// An unresolved request is never overwritten by newer content.
			await send(record.pending);
			return;
		}
		if (
			desired !== undefined &&
			(record.accepted === undefined ||
				!sameSnapshot(desired, record.accepted.snapshot))
		) {
			await stage(desired);
			return;
		}
		if (heartbeatDue() && record.accepted !== undefined)
			await stage(record.accepted.snapshot);
	}

	async function drain(): Promise<void> {
		if (running || stopped || fenced || retired) return;
		running = true;
		try {
			await drainOnce();
		} catch (error) {
			fenced = true;
			stopPump();
			try {
				note(`channel ${channel} storage failure: ${String(error).slice(0, 180)}`);
			} catch {
				// Persistence failure is diagnostic-only; never escape into the host.
			}
		} finally {
			running = false;
			if (again && !stopped && !fenced && !retired) {
				again = false;
				kick();
			}
		}
	}

	const attention = new Set<(attention: AttentionSerialize) => void>();

	function kick(): void {
		if (stopped || fenced || retired) return;
		startPump();
		if (running) {
			again = true;
			return;
		}
		void drain().catch(() => undefined);
	}

	return {
		update(snapshot) {
			if (stopped || fenced || retired) return;
			desired = snapshot;
			kick();
		},
		retire() {
			if (stopped || fenced || retired) return;
	retired = true;
			stopPump();
			const writer = record.writer;
			if (writer === undefined) return;
			const agentId = record.target_id;
			try {
				void client.retire({...addressed(agentId), channel, writer_handle: writer.handle})
					.then((answer) => {
				if (answer.ok === true) {
						try {
							record.retired_at = new Date(scheduler.now()).toISOString();
							save();
						} catch {
							// Local retirement persistence is best-effort after daemon success.
						}
					}
					})
					.catch(() => undefined);
			} catch {
				// Retirement must not escape a best-effort close path.
			}
		},
		stop() {
			stopped = true;
			stopPump();
		},
		reopen() {
			stopped = false;
			if (desired !== undefined || record.pending !== undefined) kick();
		},
		onAttention(listener) {
			attention.add(listener);
			return () => void attention.delete(listener);
		},
		diagnostic: () => record.diagnostic,
	};
}

export type RadarPresenceReporter = {
	/** Start a fresh session-load claim. Retries and heartbeats keep its claim ID. */
	report(session: string): void;
	/** Release this exact session claim on graceful shutdown. */
	stop(): void;
	/** Re-arm a stopped writer after an extension reload. */
	reopen(): void;
	/** Radar's standing on the session last published, once the daemon answered. */
	management(): RadarManagement | undefined;
	ensure(): Promise<string | undefined>;
	release(): Promise<void>;
	diagnostic(): string | undefined;
};

type PresenceReporterInput = {
	store: PublicationStore;
	client: RadarPublicationClient;
	scheduler: RadarScheduler;
	recordName: string;
	registration(): Promise<RadarRegistration | undefined>;
	/** The subject Radar granted this process's session, when it granted one. */
	granted?(subjectId: string): void;
	heartbeatMs: number;
};

/**
 * Owns this process's durable claim retry state; Radar owns the subject grant.
 * Session-load boundaries create claim IDs; heartbeats/reconnects reuse them.
 */
function createPresenceReporter(input: PresenceReporterInput): RadarPresenceReporter {
	const { store, client, scheduler, recordName } = input;
	type State = {
		version: 2;
		claim_id: string;
		session: string;
		registration_id?: string;
		subject_id?: string;
		releases: Array<{ registration_id: string; claim_id: string }>;
	};
	let state: State | undefined;
	let standing: RadarManagement | undefined;
	let diagnostic: string | undefined;
	try {
		const saved = store.read(recordName) as State | undefined;
		if (
			saved?.version === 2 &&
			isUuid(saved.claim_id) &&
			isUuid(saved.session) &&
			Array.isArray(saved.releases)
		)
			state = saved;
	} catch (error) {
		diagnostic = `presence storage unavailable: ${String(error)}`;
	}
	let timer: RadarTimer | undefined;
	let stopped = false;
	let running: Promise<string | undefined> | undefined;
	function save(): boolean {
		try {
			if (state) store.write(recordName, state);
			return true;
		} catch (error) {
			diagnostic = `presence storage unavailable: ${String(error)}`;
			return false;
		}
	}
	async function releasePending() {
		if (!state) return;
		const registration = await input.registration();
		if (!registration?.process) return;
		for (const release of [...state.releases]) {
			const answer = await client.releasePresence({ ...release, process: registration.process });
			if (!answer.ok) return;
			state.releases = state.releases.filter(r => r.claim_id !== release.claim_id);
			save();
		}
	}
	function schedule() {
		if (stopped || timer) return;
		timer = scheduler.setTimeout(() => { timer = undefined; void ensure(); }, input.heartbeatMs);
		timer.unref?.();
	}
	async function ensure(): Promise<string | undefined> {
		if (running) return running;
		if (stopped || !state) return undefined;
		const claim = state;
		if (!save()) return undefined;
		running = (async () => {
			await releasePending();
			const registration = await input.registration();
			if (!registration?.process || state !== claim || stopped) return undefined;
			const answer = await client.presence({
				...registration, process: registration.process,
				session_id: claim.session, claim_id: claim.claim_id,
				...(claim.registration_id ? { registration_id: claim.registration_id } : {}),
				...(claim.subject_id ? { subject_id: claim.subject_id } : {}),
			});
			if (state !== claim) return undefined;
			if (!answer.ok) {
				if (!["absent", "timeout", "transport"].includes(answer.code)) diagnostic = answer.message;
				return undefined;
			}
			claim.registration_id = answer.value.registration_id;
			claim.subject_id = answer.value.standing === "granted" ? answer.value.subject_id ?? undefined : undefined;
			if (claim.subject_id !== undefined) input.granted?.(claim.subject_id);
			standing = answer.value.standing === "granted"
				? { state: "managed", subject_id: claim.subject_id! }
				: answer.value.standing === "duplicate"
					? { state: "duplicate" }
					: answer.value.standing === "pending"
						? { state: "pending" }
						: { state: "unmanaged" };
			diagnostic = answer.value.standing === "duplicate" ? DUPLICATE_SESSION_DIAGNOSTIC : undefined;
			save();
			return claim.registration_id;
		})();
		try { return await running; }
		catch (error) { diagnostic = `presence unavailable: ${String(error)}`; return undefined; }
		finally {
			running = undefined;
			if (!stopped && state !== claim) void ensure();
			else schedule();
		}
	}
	return {
		report(session) {
			const releases = [...(state?.releases ?? [])];
			if (state?.registration_id && !releases.some(release => release.claim_id === state!.claim_id)) {
				releases.push({ registration_id: state.registration_id, claim_id: state.claim_id });
			}
			standing = undefined;
			state = { version: 2, session, claim_id: randomUUID(), releases };
			save(); stopped = false;
			void ensure();
		},
		ensure,
		async release() {
			const releasing = state;
			standing = undefined;
			stopped = true;
			if (timer) scheduler.clearTimeout(timer);
			timer = undefined;
			if (running) await running;
			if (state === releasing) standing = undefined;
			if (releasing?.registration_id && !releasing.releases.some(r => r.claim_id === releasing.claim_id)) {
				releasing.releases.push({ registration_id: releasing.registration_id, claim_id: releasing.claim_id });
				if (!save()) return;
			}
			await releasePending();
		},
		stop() { stopped = true; if (timer) scheduler.clearTimeout(timer); timer = undefined; },
		reopen() { stopped = false; void ensure(); },
		management: () => standing,
		diagnostic: () => diagnostic,
	};
}


export type RadarPublication = {
	/** This process's own execution channel. */
	execution: RadarChannelWriter;
	/** This process's own current session. */
	presence: RadarPresenceReporter;
	/** This process's own facts, a complete replacement on every update. */
	facts: RadarFactsWriter;
	/** The assignment channel of one owned child subject. */
	assignment(agentId: string, reportingOwner: string): RadarChannelWriter;
	/** The exact subject an owned child registered, when it published one. */
	binding(key: string): RadarChildBinding | undefined;
	/**
	 * The exact subject this process is registered as, once the daemon answered.
	 * Absent while the daemon is unreachable, which is a launch prerequisite's
	 * answer rather than a reason to register or spawn under another identity.
	 */
	subject(): Promise<RadarSubject | undefined>;
	/** Retire the assignment writer this process actually holds for a child. */
	retireAssignment(agentId: string): void;
	diagnostic(): string | undefined;
	/** Release every writer's timers; facts already published stay. */
	stop(): void;
	/** Re-arm after an extension reload that reused this publication. */
	reopen(): void;
};

/** What a process's own registration resolved to, and where it says it is. */
export type RadarSubject = {
	registration_id: string;
	/** Stable managed logical subject: a Radar-issued launch, or an adopted Pi session. */
	subject_id?: string;
	location?: RadarLocation;
};

export type RadarPublicationOptions = {
	/** The private state root. The caller owns its location; this package has no default. */
	dataRoot: string;
	store?: PublicationStore;
	client: RadarPublicationClient;
	/** The immutable registration content, apart from the incarnation, claim and location. */
	registration: Pick<RadarRegistration, "source" | "owner" | "run" | "label">;
	incarnation?: string;
	slot?: RadarProcessSlot;
	scheduler?: RadarScheduler;
	/** `undefined` omits the process claim rather than guessing one. */
	birth?: ProcessBirth;
	/** Current pane observation, refreshed for every presence report. Missing location
	 * leaves process identity reportable but pending subject grant; no pane is guessed.
	 */
	location?: () => Promise<RadarLocationResolution>;
	/** Set on a managed child: the sidecar its owner correlates the subject with. */
	binding?: { key: string; run: string; owner: string; label: string };
	/** A daemon-spawned child's one-use launch token; sent privately, never stored in the process record. */
	launchRegistrationId?: string;
	leaseMs?: number;
	heartbeatMs?: number;
};

/** The process registration and its daemon-issued registration ID. */
type RegistrationRecord = {
	version: typeof RADAR_PUBLICATION_VERSION;
	registration: RadarRegistration;
	registration_id?: string;
};

/**
 * An injected store is a different state root from the derivation `dataRoot`
 * names, so reusing one publication across the two would republish another
 * root's records under this-incarnation records.
 */
const STORE_IDENTITY = new WeakMap<PublicationStore, string>();

function storeIdentity(store: PublicationStore): string {
	const known = STORE_IDENTITY.get(store);
	if (known !== undefined) return known;
	const created = randomUUID();
	STORE_IDENTITY.set(store, created);
	return created;
}

export function createRadarPublication(
	options: RadarPublicationOptions,
): RadarPublication {
	try {
		return createRadarPublicationInner(options);
	} catch (error) {
		return unavailablePublication(error);
	}
}

function unavailablePublication(error?: unknown): RadarPublication {
	const diagnostic = `Radar publication unavailable${error ? `: ${String(error).slice(0, 180)}` : ""}`;
	const writer: AttentionWriter = {
		update: () => undefined,
		retire: () => undefined,
		stop: () => undefined,
		reopen: () => undefined,
		onAttention: () => () => undefined,
		diagnostic: () => diagnostic,
	};
	return {
		execution: writer,
		presence: {
			report: () => undefined,
			ensure: async () => undefined,
			release: async () => undefined,
			stop: () => undefined,
			reopen: () => undefined,
			management: () => undefined,
			diagnostic: () => diagnostic,
		},
		facts: { ...writer, update: () => undefined },
		assignment: () => writer,
		binding: () => undefined,
		subject: async () => undefined,
		retireAssignment: () => undefined,
		diagnostic: () => diagnostic,
		stop: () => undefined,
		reopen: () => undefined,
	};
}

function createRadarPublicationInner(
	options: RadarPublicationOptions,
): RadarPublication {
	const dataRoot = options.dataRoot;
	const store = options.store ?? createPublicationStore(dataRoot);
	const scheduler = options.scheduler ?? systemScheduler;
	const client = options.client;
	const slot = options.slot ?? radarProcessSlot();
	const launchRegistrationId = options.launchRegistrationId ?? slot.launchRegistrationId;
	const incarnation = options.incarnation ?? radarProcessIncarnation(slot);

	const cacheKey = `${
		options.store === undefined ? dataRoot : storeIdentity(options.store)
	}\0${incarnation}`;
	const cached = slot.publications.get(cacheKey);
	if (cached !== undefined) {
		// A reload reuses the process's one publisher: a second binding for the same
		// incarnation would be a competing publisher of the same channels.
		cached.reopen();
		return cached;
	}

	const registrationName = `registrations/${createHash("sha256")
		.update(`${options.registration.source}\0${incarnation}`)
		.digest("hex")
		.slice(0, 32)}.json`;

	/**
	 * One channel record per publisher, not one per channel: the data root is the
	 * user's agent directory, so unrelated and managed processes share it and a
	 * fixed `channels/execution.json` would make each one clobber the other's
	 * binding, sequence and unresolved request.
	 */
	const channelRecord = (channel: RadarChannel, target?: string): string =>
		`channels/${createHash("sha256")
			.update(`${channel}\0${incarnation}\0${target ?? ""}`)
			.digest("hex")
			.slice(0, 32)}.json`;

	/** One current-session record per publisher, for the same reason as channels. */
	const presenceRecord = `presence/${createHash("sha256")
		.update(`presence\0${incarnation}`)
		.digest("hex")
		.slice(0, 32)}.json`;

	/**
	 * The immutable registration is written before it is ever sent, and the
	 * stored content wins over a later attempt to build it again: a claim read
	 * successfully once may not change what this incarnation is registered as.
	 */
	function readRegistration(): RegistrationRecord | undefined {
		const value = store.read(registrationName);
		const registration = recordAt(value, "registration");
		if (
			numberAt(value, "version") !== RADAR_PUBLICATION_VERSION ||
			textAt(registration, "source") !== options.registration.source ||
			textAt(registration, "incarnation") !== incarnation
		)
			return undefined;
		const storedAgentId = textAt(value, "registration_id");
		return {
			version: RADAR_PUBLICATION_VERSION,
			registration: registration as RadarRegistration,
			...(storedAgentId !== undefined && isUuid(storedAgentId)
				? { registration_id: storedAgentId }
				: {}),
		};
	}

	const stored = readRegistration();
	let processRecord: RegistrationRecord | undefined = stored;
	let registrationDiagnostic: string | undefined;
	let registrationRecord: Promise<RegistrationRecord | undefined> | undefined;
	/**
	 * The immutable registration is written before it is ever sent, and the stored
	 * content wins over a later attempt to build it again: a claim read
	 * successfully once may not change what this incarnation is registered as. The
	 * location is the one fact that can still be arriving when the adapter starts,
	 * so the record is built - and written - at that point rather than earlier. A
	 * store that cannot hold it leaves this process unregistered and says why,
	 * rather than throwing out of a lifecycle event.
	 *
	 * A location that is still pending writes nothing at all. This process has not
	 * been told where it is, and an immutable record that froze that gap could
	 * never be repaired without replacing the identity it belongs to; the next
	 * attempt asks the daemon again instead.
	 */
	async function ensureRegistration(): Promise<RegistrationRecord | undefined> {
		if (processRecord !== undefined) return processRecord;
		registrationRecord ??= (async () => {
			const resolution = options.location
				? await options.location()
				: ({ state: "absent", reason: "this process publishes no location" } as const);
			const built: RegistrationRecord = {
				version: RADAR_PUBLICATION_VERSION,
				registration: {
					...options.registration,
					...(resolution.state === "resolved"
						? { location: resolution.location }
						: {}),
					incarnation,
					...(options.birth !== undefined ? { process: options.birth } : {}),
				},
			};
			try {
				store.write(registrationName, built);
			} catch (error) {
				registrationDiagnostic ??=
					`private storage unavailable: ${String(error)}`.slice(0, 256);
				return undefined;
			}
			registrationDiagnostic = undefined;
			processRecord = built;
			return built;
		})();
		try {
			return await registrationRecord;
		} finally {
			registrationRecord = undefined;
		}
	}
	let agentId = stored?.registration_id;
	let subjectReady: Promise<string | undefined> | undefined;
	async function ensureSubject(recheck: boolean): Promise<string | undefined> {
		if (!recheck && agentId !== undefined) return agentId;
		subjectReady ??= (async () => {
			const ready = await client.ping();
			if (ready.ok === false) {
				if (ready.code !== "absent" && ready.code !== "timeout" && ready.code !== "transport")
					registrationDiagnostic ??= `daemon negotiation failed: ${ready.message}`.slice(0, 256);
				return undefined;
			}
			const record = await ensureRegistration();
			if (record === undefined) return undefined;
			const registered = await presence.ensure();
			if (registered === undefined) return undefined;
			agentId = registered;
			processRecord = { ...record, registration_id: agentId };
			store.write(registrationName, processRecord);
			if (options.binding !== undefined) writeBinding(options.binding, grantedSubject());
			return agentId;
		})();
		try {
			return await subjectReady;
		} finally {
			subjectReady = undefined;
		}
	}

	/** The subject Radar granted this process, once presence has been answered. */
	function grantedSubject(): string | undefined {
		const standing = presence.management();
		return standing?.state === "managed" ? standing.subject_id : undefined;
	}

	function writeBinding(binding: {
		key: string;
		run: string;
		owner: string;
		label: string;
	}, subjectId?: string): void {
		if (agentId === undefined) return;
		store.write(`bindings/${binding.key}.json`, {
			version: RADAR_PUBLICATION_VERSION,
			registration_id: agentId,
			...(subjectId !== undefined ? { subject_id: subjectId } : {}),
			incarnation,
			run: binding.run,
			owner: binding.owner,
			label: binding.label,
			observed_at: new Date(scheduler.now()).toISOString(),
		} satisfies RadarChildBinding);
	}

	const publisher = (
		source: string,
		reportingOwner?: string,
	): RadarPublisherIdentity => ({
		source,
		incarnation,
		...(reportingOwner !== undefined ? { reporting_owner: reportingOwner } : {}),
	});

	const execution = createChannelWriter({
		store,
		client,
		scheduler,
		recordName: channelRecord("execution"),
		channel: "execution",
		publisher: publisher("herdsman-pi"),
		subject: ensureSubject,
		leaseMs: options.leaseMs ?? RADAR_LEASE_MS,
		heartbeatMs: options.heartbeatMs ?? RADAR_HEARTBEAT_MS,
	});

	const presence = createPresenceReporter({
		store, client, scheduler, recordName: presenceRecord,
		registration: async () => {
			const record = await ensureRegistration();
			if (!record) return undefined;
			const location = await options.location?.();
			const { location: _initialLocation, ...identity } = record.registration;
			return { ...identity,
				...(location?.state === "resolved" ? { location: location.location } : {}),
				...(launchRegistrationId ? { registration_id: launchRegistrationId } : {}),
			};
		},
		granted: (subjectId) => {
			if (options.binding !== undefined) writeBinding(options.binding, subjectId);
		},
		heartbeatMs: options.heartbeatMs ?? RADAR_HEARTBEAT_MS,
	});

	const facts = createChannelWriter({
		store,
		client,
		scheduler,
		recordName: channelRecord("facts"),
		channel: "facts",
		publisher: publisher("pi"),
		codec: factsCodec,
		subject: ensureSubject,
		leaseMs: options.leaseMs ?? RADAR_LEASE_MS,
		heartbeatMs: options.heartbeatMs ?? RADAR_HEARTBEAT_MS,
	});

	const assignmentWriters = new Map<string, RadarChannelWriter>();

	const publication: RadarPublication = {
		execution,
		presence,
		facts,
		assignment(target, reportingOwner: string) {
			const existing = assignmentWriters.get(target);
			if (existing !== undefined) return existing;
			try {
				const writer = createChannelWriter({
					store,
					client,
					scheduler,
					recordName: channelRecord("assignment", target),
					channel: "assignment",
					publisher: publisher("herdsman-owner", reportingOwner),
					codec: assignmentCodec,
					// Radar owns this assignment across the child's process replacements.
					subject: async () => target,
					leaseMs: options.leaseMs ?? RADAR_LEASE_MS,
					heartbeatMs: options.heartbeatMs ?? RADAR_HEARTBEAT_MS,
				});
				assignmentWriters.set(target, writer);
				return writer;
			} catch (error) {
				registrationDiagnostic ??= `Radar assignment unavailable: ${String(error).slice(0, 180)}`;
				return unavailablePublication(error).execution;
			}
		},
		binding(key) {
			let value: unknown;
			try {
				value = store.read(`bindings/${key}.json`);
			} catch (error) {
				registrationDiagnostic ??= `Radar binding unavailable: ${String(error).slice(0, 180)}`;
				return undefined;
			}
			const bound = textAt(value, "registration_id");
			const subjectId = textAt(value, "subject_id");
			const boundIncarnation = textAt(value, "incarnation");
			const run = textAt(value, "run");
			const owner = textAt(value, "owner");
			const label = textAt(value, "label");
			const observedAt = textAt(value, "observed_at");
			// A sidecar is a correlation, not authority: only a canonical subject
			// identity is ever used as a channel target.
			if (
				!isUuid(bound) ||
				(subjectId !== undefined && !isUuid(subjectId)) ||
				!isUuid(boundIncarnation) ||
				run === undefined ||
				owner === undefined ||
				label === undefined ||
				observedAt === undefined
			)
				return undefined;
			return {
				version: RADAR_PUBLICATION_VERSION,
				registration_id: bound,
				...(subjectId !== undefined ? { subject_id: subjectId } : {}),
				incarnation: boundIncarnation,
				run,
				owner,
				label,
				observed_at: observedAt,
			};
		},
		retireAssignment(targetAgentId) {
			// Only a writer this process actually holds is retired; retiring by
			// acquiring first would be an implicit takeover of the channel.
			assignmentWriters.get(targetAgentId)?.retire();
		},
		async subject() {
			const agentId = await ensureSubject(false);
			if (agentId === undefined) return undefined;
			const record = await ensureRegistration();
			if (record === undefined) return undefined;
			const currentLocation = await options.location?.();
			const location = currentLocation?.state === "resolved" ? currentLocation.location : undefined;
			// Radar's answer to this process's current session names its subject.
			const standing = presence.management();
			const subjectId = standing?.state === "managed" ? standing.subject_id : undefined;
			return {
				registration_id: agentId,
				...(subjectId !== undefined ? { subject_id: subjectId } : {}),
				...(location !== undefined ? { location } : {}),
			};
		},
		diagnostic: () =>
			registrationDiagnostic ??
			execution.diagnostic() ??
			presence.diagnostic() ??
			facts.diagnostic(),
		stop() {
			for (const writer of assignmentWriters.values()) writer.stop();
			execution.stop();
			presence.stop();
			facts.stop();
		},
		reopen() {
			for (const writer of assignmentWriters.values()) writer.reopen();
			execution.reopen();
			presence.reopen();
			facts.reopen();
		},
	};

	slot.publications.set(cacheKey, publication);
	return publication;
}
