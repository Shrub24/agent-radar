import {
	boundedRadarSnapshot,
	isUuid,
	type RadarScheduler,
	type RadarSnapshot,
} from "./radar-client.ts";
import type { RadarLocationResolution } from "./radar-location.ts";
import type { RadarFactsSnapshot } from "./radar-client.ts";
import {
	createRadarPublication,
	type RadarPublicationClient,
	radarProcessIncarnation,
	radarProcessSlot,
	readProcessBirth,
	type ProcessBirth,
	type PublicationStore,
	type RadarChildBinding,
	type RadarProcessSlot,
	type RadarPublication,
	type RadarChannelWriter,
} from "./radar-publication.ts";

/**
 * The minimal Pi surface this adapter uses. It is declared structurally so the
 * adapter stays independent of Herdsman's roles, assignments and Herdr: it can
 * be lifted into its own extension without touching this module.
 */
export type RadarExecutionEventApi = {
	on(name: string, handler: (event: any, context: any) => unknown): unknown;
	events?: {
		on(name: string, handler: (event: any) => unknown): unknown;
	};
};

export type RadarExecutionOptions = {
	client: RadarPublicationClient;
	/** Reuse an existing publication instead of building one. */
	publication?: RadarPublication;
	dataRoot: string;
	store?: PublicationStore;
	slot?: RadarProcessSlot;
	scheduler?: RadarScheduler;
	incarnation?: string;
	/** Present and `undefined` means the platform reported no claim. */
	birth?: ProcessBirth;
	/**
	 * The pane coordinates this process's trusted startup environment reported,
	 * or the read that resolves them. Absent where the environment names no
	 * location: registration then carries none, and a caller that needs one
	 * refuses instead of inventing it.
	 */
	location?: () => Promise<RadarLocationResolution>;
	/**
	 * A daemon-spawned child's one-use launch token, captured before this process
	 * could observe anything else. It travels with the registration request only.
	 */
	spawnToken?: string;
	/** Only a real fixed managed assignment is part of immutable registration. */
	managedBinding?: { key: string; run: string; owner: string; label: string };
	/**
	 * A second, idempotent blocking signal (`rpiv:ask-user:blocked`): a
	 * questionnaire is one wait, so its own flag cannot count it twice. Absent
	 * where no such event exists.
	 */
	questionnaireEvent?: string;
};

export type RadarExecutionAdapter = {
	publication: RadarPublication;
	/** The exact subject a managed child registered, for its owner to correlate. */
	binding(): RadarChildBinding | undefined;
	/** Whether this process is blocked on a questionnaire or a permission prompt. */
	blocked(): boolean;
	stop(): void;
};

/** The publisher name of a Pi process's own execution facts. */
export const RADAR_SUBJECT_SOURCE = "herdsman-pi";

/**
 * The identity this process registered under: the source its registration names
 * and the incarnation the process slot holds. The slot is what tells two
 * processes apart - an extension reload keeps its incarnation, while another
 * process occupying the same mailbox, run, pane and session has its own, even
 * when it resumes the same Pi session.
 */
export function radarLocalRegistration(): {
	source: string;
	incarnation: string;
} {
	return {
		source: RADAR_SUBJECT_SOURCE,
		incarnation: radarProcessIncarnation(),
	};
}

function dialogLabel(event: unknown): string {
	if (typeof event !== "object" || event === null) return "";
	const label = (event as { label?: unknown }).label;
	return typeof label === "string" && label.trim()
		? label.trim().slice(0, 256)
		: "";
}

function assistantStop(event: unknown): { stopReason?: unknown } {
	const message =
		typeof event === "object" && event !== null && "message" in event
			? (event as { message?: unknown }).message
			: event;
	if (typeof message !== "object" || message === null) return {};
	const value = message as {
		role?: unknown;
		stopReason?: unknown;
		errorMessage?: unknown;
	};
	return value.role === "assistant" ? { stopReason: value.stopReason } : {};
}

function unavailablePublication(): RadarPublication {
	const writer: RadarChannelWriter = {
		update: () => undefined,
		retire: () => undefined,
		stop: () => undefined,
		reopen: () => undefined,
		diagnostic: () => "Radar private storage is unavailable",
	};
	return {
		execution: writer,
		facts: { ...writer, update: () => undefined },
		context: {
			update: () => undefined,
			stop: () => undefined,
			reopen: () => undefined,
			diagnostic: () => "Radar private storage is unavailable",
		},
		assignment: () => writer,
		binding: () => undefined,
		subject: async () => undefined,
		retireAssignment: () => undefined,
		diagnostic: writer.diagnostic,
		stop: () => undefined,
		reopen: () => undefined,
	};
}

/**
 * Generic Pi lifecycle adapter for one process's `execution` channel.
 *
 * It reports run activity, blocking, reload mid-run and a settled outcome kept
 * separate from activity. Nothing here reads or writes Herdr, requires a pane,
 * or restricts itself to TUI mode.
 *
 * Pi session association is deliberately absent from immutable registration: a
 * process can switch, resume or fork sessions without becoming another subject,
 * so the current session is the mutable context record instead.
 */
/** This session's model, thinking level, name and context usage, as Pi reports them. */
function factsFrom(context: any): RadarFactsSnapshot {
	const facts: RadarFactsSnapshot = {};
	const model = context?.model;
	if (typeof model?.id === "string" && model.id.length > 0)
		facts.model = model.id;
	if (typeof model?.provider === "string" && model.provider.length > 0)
		facts.provider = model.provider;
	const thinking = context?.thinkingLevel;
	if (typeof thinking === "string" && thinking.length > 0)
		facts.thinking = thinking;
	const name = context?.sessionManager?.getSessionName?.();
	if (typeof name === "string" && name.trim().length > 0)
		facts.session_name = name.trim();
	return facts;
}

export function createRadarExecutionAdapter(
	pi: RadarExecutionEventApi,
	options: RadarExecutionOptions,
): RadarExecutionAdapter {
	const slot = options.slot ?? radarProcessSlot();
	const incarnation =
		options.incarnation ?? radarProcessIncarnation(slot);
	const managed = options.managedBinding;
	// A claim read successfully here is persisted with the registration, so a
	// later read cannot change what this incarnation is registered as.
	const birth = "birth" in options ? options.birth : readProcessBirth();
	let publication: RadarPublication;
	try {
		publication = options.publication ?? createRadarPublication({
			dataRoot: options.dataRoot,
			store: options.store,
			client: options.client,
			scheduler: options.scheduler,
			slot,
			incarnation,
			birth,
			registration: {
				source: RADAR_SUBJECT_SOURCE,
				...(managed !== undefined
					? { owner: managed.owner, run: managed.run, label: managed.label }
					: {}),
			},
			...(options.location !== undefined
				? { location: options.location }
				: {}),
			...(managed !== undefined ? { binding: managed } : {}),
			...(options.spawnToken !== undefined
				? { spawnToken: options.spawnToken }
				: {}),
		});
	} catch {
		publication = unavailablePublication();
	}

	const disposers: Array<() => void> = [];
	const keep = (disposer: unknown): void => {
		if (typeof disposer === "function")
			disposers.push(disposer as () => void);
	};
	const on = (
		name: string,
		handler: (event: any, context: any) => void,
	): void => {
		keep(
			pi.on(name, (event, context) => {
				if (!stopped) handler(event, context);
			}),
		);
	};

	let stopped = false;
	let rootSession = false;
	let active = false;
	/** One questionnaire at a time per process, so a flag is its whole state. */
	let questionnaire: string | undefined;
	/** Permission asks Pi has shown and not yet decided, by their request ids. */
	const permissionPrompts = new Set<string>();
	let lastOutcome: RadarSnapshot["last_outcome"];
	let lastPublished: string | undefined;

	function blocked(): boolean {
		return (
			questionnaire !== undefined ||
			permissionPrompts.size > 0
		);
	}

	function currentSnapshot(): RadarSnapshot {
		const reason =
			questionnaire ?? (permissionPrompts.size > 0 ? "permission" : undefined);
		return {
			activity: blocked() ? "blocked" : active ? "working" : "idle",
			...(reason !== undefined ? { waiting_reason: reason } : {}),
			...(lastOutcome !== undefined ? { last_outcome: lastOutcome } : {}),
		};
	}

	/**
	 * Report the session this process is now in. A session Pi cannot name as a
	 * canonical UUID is left unreported: absent context is unknown, and publishing
	 * the explicit null would claim there is no session at all.
	 */
	function reportSession(context: any): void {
		const session = context?.sessionManager?.getSessionId?.();
		if (!isUuid(session)) return;
		try {
			publication.context.update(session);
		} catch {
			// Optional publication must not interfere with Pi's lifecycle handler.
		}
	}

	let lastFacts: string | undefined;

	/**
	 * Facts are a complete snapshot, so every change to a fact republishes the
	 * whole set. Unchanged content is not republished.
	 */
	function publishFacts(context: any): void {
		if (stopped || !rootSession) return;
		const facts = factsFrom(context);
		const encoded = JSON.stringify(facts);
		if (encoded === lastFacts) return;
		// Nothing known yet is not a fact to publish; a fact that disappears later
		// still withdraws through an empty snapshot.
		if (lastFacts === undefined && encoded === "{}") return;
		lastFacts = encoded;
		try {
			publication.facts.update(facts);
		} catch {
			// Optional publication must not interfere with Pi's lifecycle handler.
		}
	}
	for (const name of [
		"model_select",
		"thinking_level_select",
		"session_info_changed",
		"turn_end",
	]) {
		on(name, (_event, context) => publishFacts(context));
	}

	function emit(): void {
		if (stopped || !rootSession) return;
		const snapshot = boundedRadarSnapshot(currentSnapshot());
		const encoded = JSON.stringify(snapshot);
		if (encoded === lastPublished) return;
		lastPublished = encoded;
		try {
			publication.execution.update(snapshot);
		} catch {
			// Optional publication must not interfere with Pi's lifecycle handler.
		}
	}

	// Pi's permission bridge: one prompt is open from its ui_prompt until the
	// decision carrying the same request id. Decisions for prompts this process
	// never showed match nothing and change nothing.
	keep(
		pi.events?.on("permissions:ui_prompt", (event) => {
			const requestId = (event as { requestId?: unknown } | null)?.requestId;
			if (!rootSession || typeof requestId !== "string") return;
			permissionPrompts.add(requestId);
			emit();
		}),
	);
	keep(
		pi.events?.on("permissions:decision", (event) => {
			const requestId = (event as { requestId?: unknown } | null)?.requestId;
			if (typeof requestId !== "string" || !permissionPrompts.delete(requestId))
				return;
			emit();
		}),
	);

	if (options.questionnaireEvent !== undefined)
		keep(
			pi.events?.on(options.questionnaireEvent, (event) => {
				if (!rootSession || typeof event !== "object" || event === null)
					return;
				const value = event as { active?: unknown };
				if (typeof value.active !== "boolean") return;
				questionnaire = value.active
					? dialogLabel(event) || "questionnaire"
					: undefined;
				emit();
			}),
		);

	on("session_start", (_event, context) => {
		rootSession = true;
		// The session comes first: re-arming the publication must renew the session
		// this process is in now, never the one a previous session left behind.
		reportSession(context);
		// Facts follow the root flag, so they are published only after it is set.
		publishFacts(context);
		// A process can replace or resume its session without a new process, so the
		// same publication is re-armed rather than replaced.
		publication.reopen();
		// A reload can replace this extension mid-run without another agent_start,
		// so the run state comes from Pi's context rather than an assumed idle.
		active = context?.isIdle?.() === false;
		emit();
	});

	on("agent_start", () => {
		if (!rootSession) return;
		active = true;
		emit();
	});

	on("agent_settled", (event, context) => {
		if (!rootSession) return;
		// `aborted` is Pi's authoritative final outcome discriminator. A
		// non-retryable provider error also settles non-aborted, so the run's own
		// recorded outcome stands in that case.
		if (event?.aborted === true) lastOutcome = { result: "aborted" };
		// Queued or compacting work keeps the process working; only an idle
		// context ends the activity. Assignment state is never touched here: an
		// abort is not a cancelled assignment.
		if (context?.isIdle?.() === true) active = false;
		emit();
	});

	on("message_end", (event) => {
		if (!rootSession) return;
		const { stopReason } = assistantStop(event);
		// Tool calls, length cut-offs, non-assistant messages and unknown stop
		// reasons are not terminal assistant outcomes.
		if (stopReason === "error") {
			lastOutcome = { result: "error" };
		} else if (stopReason === "aborted") {
			lastOutcome = { result: "aborted" };
		} else if (stopReason === "stop") {
			lastOutcome = { result: "finished" };
		} else {
			return;
		}
		emit();
	});

	on("session_shutdown", () => {
		// A session can end or be replaced without ending this process, and Pi keeps
		// the same extension instance across that: the listeners stay registered so
		// the next session_start re-arms the same process publication. Only the
		// timers and this session's facts are released.
		suspend();
	});

	function suspend(): void {
		if (stopped) return;
		rootSession = false;
		active = false;
		questionnaire = undefined;
		permissionPrompts.clear();
		lastOutcome = undefined;
		lastPublished = undefined;
		// Facts already published stay; only the heartbeat and the retry driver stop.
		publication.stop();
	}

	/** Explicit disposal, for a caller that owns the adapter rather than Pi. */
	function stop(): void {
		if (stopped) return;
		suspend();
		stopped = true;
		for (const dispose of disposers.splice(0)) dispose();
	}

	return {
		publication,
		binding: () =>
			managed !== undefined ? publication.binding(managed.key) : undefined,
		blocked,
		stop,
	};
}
