import { setTimeout as delay } from "node:timers/promises";
import {
	isUuid,
	type RadarClient,
	type RadarErrorCode,
	type RadarLocation,
	type RadarResult,
	type RadarSpawnEdge,
} from "./radar-client.ts";
import { radarPaneHandle } from "./radar-location.ts";

/** The startup acknowledgement a launch waits for, named by its identity. */
export type StartupExpectation = {
	generation: string;
	runId: string;
	ownerSessionId: string;
	workspaceId: string;
	agentLabel: string;
	paneId: string;
};

/**
 * The mailbox readers a launch needs. The mailbox belongs to the caller (Herdsman
 * supplies its own state shape and readers); this module only takes the path it
 * is handed and asks these questions of it.
 */
export type LaunchState = { paneId: string };

export type LaunchMailbox<State extends LaunchState> = {
	readState(path: string): State | undefined;
	waitForState(
		path: string,
		predicate: (state: State) => boolean,
		options: { timeoutMs: number; signal?: AbortSignal },
	): Promise<State>;
	startupAckMatches(state: State | undefined, expected: StartupExpectation): boolean;
};

/**
 * Launching one fresh managed worker through Radar's spawn operation.
 *
 * A spawn is one durable effect behind the caller's own request UUID, and three
 * separate facts decide whether a child can take an assignment:
 *
 * 1. the daemon created a pane under this process's registered pane, launched
 *    the child's exact command into it, and still reports that pane in its own
 *    inventory;
 * 2. a child registered with the launch's private token, so the edge names the
 *    exact `agent_id` that child obtained — never a label, pane or session;
 * 3. that child wrote a fresh startup acknowledgment for this launch generation
 *    after Herdsman startup finished, naming the pane the daemon created as its
 *    own location.
 *
 * Only the third is application readiness; the first two are prerequisites. A
 * lost reply is reconciled against the same request UUID and never re-sent, so a
 * failed launch cannot leave a second pane or a second child behind.
 *
 * The child's location is a fact about the pane the daemon created, not about
 * the environment it was launched with: the daemon passes an environment overlay
 * to a mux-created pane and cannot remove what that pane already carries, so a
 * child that inherited its parent's pane coordinates would claim the parent's
 * pane. The daemon's fresh record of the created pane and the child's own claim
 * must therefore name the same pane, and either one being stale, missing or
 * different leaves the launch unresolved rather than placed.
 */

/** The exact parent this launch may use, and nothing inferred about it. */
export type RadarLaunchParent = {
	agent_id: string;
	location?: RadarLocation;
};

/** Everything one fresh managed worker is launched with. */
export type RadarWorkerLaunch = {
	/** The caller's durable key, persisted before dispatch and never replaced. */
	request_id: string;
	/** The nonce the child must repeat in its startup acknowledgment. */
	generation: string;
	parent: RadarLaunchParent;
	parent_session_id: string;
	executable: string;
	argv: string[];
	cwd: string;
	/** The environment overlay the child needs, not a replacement environment. */
	env: Record<string, string>;
	/** The child's mailbox: its own state is the readiness evidence. */
	mailbox: string;
	/** The identity the child's state and acknowledgment must repeat exactly. */
	identity: {
		runId: string;
		ownerSessionId: string;
		workspaceId: string;
		agentLabel: string;
	};
	/**
	 * The child's own published registration, once it has one. It is the second
	 * witness that the edge's bound id is this launch's child, and it is read
	 * through the owner's publication store rather than assumed from the pane.
	 */
	observedChildAgent?: () => string | undefined;
	/**
	 * Persist the daemon's own binding of this launch's edge, the subject it bound
	 * and the pane it recorded, before readiness is waited for. This is the only
	 * moment those facts are authoritative: the launch can end without ever
	 * reaching an acknowledgment, and a reload in that window still has to read
	 * where the daemon put this child. Throwing leaves the launch unresolved,
	 * because a binding that was not persisted is one no later reader can hold the
	 * child by.
	 */
	onBound?: (bound: { agent_id: string; pane_id: string }) => void;
};

export type RadarWorkerLaunched<State = unknown> = {
	request_id: string;
	agent_id: string;
	pane_id: string;
	/**
	 * The process the daemon bound this launch's edge to, taken from that same
	 * answer: the source and incarnation the child registered under. They are what
	 * an assignment to this child is addressed with, so the first task of a fresh
	 * launch is addressed exactly as a retained continuation is.
	 */
	bound_process: { source: string; incarnation: string };
	state: State;
};

/**
 * `refused` is a launch that provably did not begin; `unresolved` is one whose
 * pane, child or readiness cannot be established, which the caller reports
 * rather than retries.
 */
export type RadarLaunchOutcome<State = unknown> =
	| { kind: "ready"; value: RadarWorkerLaunched<State> }
	| { kind: "refused"; message: string }
	| { kind: "unresolved"; request_id: string; message: string };

/**
 * What the daemon's current read proves about the child of one launch: the exact
 * subject the edge is bound to and the pane it recorded, both taken from the
 * daemon's own answer rather than from this process's record of the launch.
 */
export type RadarChildEvidence = {
	request_id: string;
	agent_id: string;
	source: string;
	incarnation: string;
	pane_id: string;
};

/**
 * `live` is current evidence for the exact child; `unknown` is every other
 * answer, including the ones that look like death. A pane the daemon no longer
 * reports is not process exit, and a read that could not be made proves nothing.
 */
export type RadarChildObservation =
	| { kind: "live"; evidence: RadarChildEvidence }
	/**
	 * Unproved by location, but the registered process is proved gone by its
	 * verdict. Only read for a child with an outstanding assignment.
	 */
	| { kind: "gone"; verification: ProcessGone; diagnostic: string }
	| { kind: "unknown"; diagnostic: string };

export type ProcessGone = "absent" | "mismatched";

function unknownChild(reason: string): RadarChildObservation {
	return { kind: "unknown", diagnostic: trajectory(reason) };
}

function trajectory(reason: string): string {
	return `the daemon's current record of this launch's child is ${reason}`;
}

/**
 * Observe whether this launch's exact child is live now.
 *
 * The durable spawn record and the child's own binding say *which* subject this
 * launch created; they are provenance, not liveness, and they stay true after a
 * child is gone. The startup acknowledgment says the child is ready, which is
 * also not liveness. Only a current topology read decides: `bound` has to name
 * the same subject, the read has to record the pane this launch claims, no
 * process exit may have been observed, and the read's own `freshness` has to
 * report that the daemon's inventory currently places that pane. Anything else -
 * a read that could not be made, an edge bound to another child, an observed
 * exit, a pane the daemon does not report, a pane this launch never recorded -
 * leaves the child unproved.
 */
export async function observeRadarChild(
	client: RadarClient,
	provenance: {
		request_id: string;
		pane_id: string | undefined;
		agent_id: string | undefined;
		/** The session this launch's mailbox records for the child. */
		session_id: string | undefined;
	},
): Promise<RadarChildObservation> {
	if (provenance.agent_id === undefined)
		return unknownChild("not bound to a child by this launch");
	// A launch that recorded no pane has nothing to compare the daemon's read
	// against: the child's own claim about where it runs is not where the launch
	// put it, and a claim cannot supply the location a launch never recorded.
	if (provenance.pane_id === undefined)
		return unknownChild("without a pane this launch recorded for it");
	const read = await client.spawnGet(provenance.request_id);
	if (read.ok === false)
		return unknownChild(
			`not readable (${read.code}: ${read.message})`,
		);
	const topology = read.value;
	const edge = topology.edge;
	if (edge.bound === undefined || edge.bound.agent_id === undefined)
		return unknownChild("not bound to a child by the daemon");
	if (edge.bound.agent_id !== provenance.agent_id)
		return unknownChild(
			`bound to another subject (${edge.bound.agent_id}), not to ${provenance.agent_id}`,
		);
	if (edge.location?.pane === undefined)
		return unknownChild("recorded without a pane");
	if (edge.location.pane !== provenance.pane_id)
		return unknownChild(
			`recorded on pane ${edge.location.pane}, not on ${provenance.pane_id}`,
		);
	// An observed exit outranks a pane the inventory still reports: the pane's
	// shell can outlive the child, and this launch's child is the process.
	if (edge.exit_observation?.state !== "unknown")
		return unknownChild("recorded with an observed process exit");
	// Freshness first: an inventory that does not report the recorded pane is
	// the reason a read leaves unresolved, and it is stale both when the backend
	// did not report it and when it could not be asked.
	if (topology.freshness !== "fresh")
		return unknownChild(
			"on a pane the daemon's own inventory does not currently report",
		);
	if (topology.state !== "bound")
		return unknownChild(
			`not currently placed (state ${topology.state ?? "unreported"})`,
		);
	// The edge says where the child was bound, not that its process still runs.
	// The registry's verdict on the bound subject, and the location it registered
	// with, are what prove the child live.
	const reply = await client.agentGet(edge.bound.agent_id);
	if (reply.ok === false)
		return unknownChild(
			`process verdict not readable (${reply.code}: ${reply.message})`,
		);
	const { verification, location, contextSession } = reply.value;
	if (verification === "absent" || verification === "mismatched")
		return {
			kind: "gone",
			verification,
			diagnostic: trajectory(`the registered process is ${verification}`),
		};
	if (verification !== "verified")
		return unknownChild(`process verdict is ${verification}`);
	if (location?.pane !== provenance.pane_id)
		return unknownChild(
			`registered on ${location?.pane ?? "no pane"}, not on ${provenance.pane_id}`,
		);
	// The subject must currently publish the session this launch's mailbox names:
	// a live process in another session, or one publishing none, is not this child.
	if (provenance.session_id === undefined)
		return unknownChild("without a session this launch recorded for it");
	if (contextSession !== provenance.session_id)
		return unknownChild(
			`publishing session ${contextSession ?? "none"}, not ${provenance.session_id}`,
		);
	return {
		kind: "live",
		evidence: {
			request_id: provenance.request_id,
			agent_id: edge.bound.agent_id,
			source: edge.bound.source,
			incarnation: edge.bound.incarnation,
			pane_id: edge.location.pane,
		},
	};
}

/**
 * The daemon's positive statement that a registered child's process is gone, or
 * `undefined`. Only `absent` and `mismatched` establish that; `verified` means it
 * still runs, and `unavailable` (including a registration with no process claim)
 * and an unreadable or missing registration are not evidence of absence.
 */
export async function readProcessGone(
	client: Pick<RadarClient, "agentGet">,
	agentId: string,
): Promise<ProcessGone | undefined> {
	const read = await client.agentGet(agentId);
	if (read.ok === false) return undefined;
	const { verification } = read.value;
	return verification === "absent" || verification === "mismatched"
		? verification
		: undefined;
}

/**
 * Why a relaunch may not replace this registered child, or `undefined` when the
 * daemon positively says its registered process is gone (see `processGone`).
 */
export async function relaunchRefusalForChild(
	client: Pick<RadarClient, "agentGet">,
	agentId: string,
): Promise<string | undefined> {
	const read = await client.agentGet(agentId);
	if (read.ok === false)
		return `the child's process cannot be read (${read.code}: ${read.message})`;
	const { verification } = read.value;
	if (verification === "absent" || verification === "mismatched") return undefined;
	return `the child's registered process is ${verification === "verified" ? "still running" : "not proved gone"}`;
}

export type RadarLaunchOptions<State extends LaunchState = LaunchState> = {
	client: RadarClient;
	readers: LaunchMailbox<State>;
	launch: RadarWorkerLaunch;
	timeoutMs: number;
	/** How often the binding and readiness waits re-read; 0 in driven tests. */
	pollMs?: number;
	signal?: AbortSignal;
};

function refusedLaunch(message: string): Extract<RadarLaunchOutcome, { kind: "refused" | "unresolved" }> {
	return { kind: "refused", message };
}

function unresolvedLaunch(
	requestId: string,
	message: string,
): Extract<RadarLaunchOutcome, { kind: "refused" | "unresolved" }> {
	return {
		kind: "unresolved",
		request_id: requestId,
		message: `spawn ${requestId} is unresolved (${message})`,
	};
}

/** Whether the edge's own two effects already prove a launch happened. */
function edgeStanding(edge: RadarSpawnEdge): "launched" | "refused" | "unknown" {
	if (edge.created === "refused" || edge.launched === "refused")
		return "refused";
	return edge.created === "completed" && edge.launched === "completed"
		? "launched"
		: "unknown";
}

function edgeMessage(edge: RadarSpawnEdge): string {
	return `the daemon's spawn record is created ${edge.created} and launched ${edge.launched}`;
}

/**
 * The daemon's own refusals, which prove no effect was dispatched. Every other
 * failure — a lost reply, a timeout, an internal error, an unreadable answer —
 * leaves the spawn's effect unknown, and the only answer to that is to read the
 * same request id back rather than to report a launch that may be running as a
 * failure, or to attempt a second one.
 */
const PRE_EFFECT_CODES: readonly RadarErrorCode[] = [
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

function spawnFailure(
	result: Extract<RadarResult<RadarSpawnEdge>, { ok: false }>,
	requestId: string,
): Extract<RadarLaunchOutcome, { kind: "refused" | "unresolved" }> {
	return PRE_EFFECT_CODES.includes(result.code)
		? refusedLaunch(result.message)
		: unresolvedLaunch(requestId, result.message);
}

/**
 * Wait for the child's own registration to bind this edge, then for its startup
 * acknowledgment. The edge is re-read by the same request UUID: a spawn that
 * already answered is never sent again.
 */
export async function launchRadarWorker<State extends LaunchState>(
	options: RadarLaunchOptions<State>,
): Promise<RadarLaunchOutcome<State>> {
	const { client, launch, signal } = options;
	const pollMs = options.pollMs ?? 250;
	const deadline = Date.now() + options.timeoutMs;
	const remaining = (): number => Math.max(0, deadline - Date.now());
	const pane = launch.parent.location?.pane;
	if (!isUuid(launch.parent.agent_id))
		return refusedLaunch(
			"the launch has no exact parent registration to spawn under",
		);
	// A spawn is created under the pane its parent registered, and this port
	// registers no location it was not given: without one there is nothing to
	// place the child beside, and a name or session lookup is not a substitute.
	if (pane === undefined || pane === "")
		return refusedLaunch(
			"this process's Radar registration names no pane, so it cannot be a spawn parent",
		);

	const spawned = await client.spawn({
		request_id: launch.request_id,
		parent: launch.parent.agent_id,
		parent_session_id: launch.parent_session_id,
		executable: launch.executable,
		argv: launch.argv,
		...(launch.cwd !== "" ? { cwd: launch.cwd } : {}),
		...(Object.keys(launch.env).length > 0 ? { env: launch.env } : {}),
	});
	if (spawned.ok === false) {
		const refusal = spawnFailure(spawned, launch.request_id);
		if (refusal.kind === "refused") return refusal;
	}
	if (spawned.ok === true) {
		const standing = edgeStanding(spawned.value);
		if (standing === "refused")
			return refusedLaunch(edgeMessage(spawned.value));
		if (standing === "unknown")
			return unresolvedLaunch(launch.request_id, edgeMessage(spawned.value));
	}

	const bound = await waitForChild(client, launch.request_id, {
		remaining,
		pollMs,
		...(signal !== undefined ? { signal } : {}),
	});
	if (bound.kind !== "bound") return bound;
	const agentId = bound.agent_id;
	// The binding is recorded before the readiness wait, not after it: the child
	// may never acknowledge, and the subject and pane the daemon named are still
	// what a reload has to prove this child by.
	if (launch.onBound !== undefined) {
		try {
			launch.onBound({ agent_id: agentId, pane_id: bound.pane_id });
		} catch (error) {
			return unresolvedLaunch(
				launch.request_id,
				`the daemon's binding of the child ${agentId} could not be recorded (${String(error)})`,
			);
		}
	}

	try {
		const state = await options.readers.waitForState(
			launch.mailbox,
			(candidate) =>
				options.readers.startupAckMatches(candidate, {
					generation: launch.generation,
					runId: launch.identity.runId,
					ownerSessionId: launch.identity.ownerSessionId,
					workspaceId: launch.identity.workspaceId,
					agentLabel: launch.identity.agentLabel,
						// The child writes the raw TMUX_PANE of the pane it runs in. Requiring
						// the pane the daemon created is what refuses a child that inherited
						// another process's coordinates.
					paneId: radarPaneHandle(bound.pane_id),
				}) &&
				(launch.observedChildAgent === undefined ||
					launch.observedChildAgent() === agentId),
			{ timeoutMs: remaining(), ...(signal !== undefined ? { signal } : {}) },
		);
		return {
			kind: "ready",
			value: {
				request_id: launch.request_id,
				agent_id: agentId,
				pane_id: bound.pane_id,
				bound_process: {
					source: bound.source,
					incarnation: bound.incarnation,
				},
				state,
			},
		};
	} catch (error) {
		const claimed = options.readers.readState(launch.mailbox)?.paneId;
		return unresolvedLaunch(
			launch.request_id,
			claimed !== undefined && claimed !== radarPaneHandle(bound.pane_id)
				? `the bound child ${agentId} claims the pane ${claimed}, not the pane ${bound.pane_id} this launch created for it`
				: `the bound child ${agentId} has not acknowledged this launch's startup (${String(error)})`,
		);
	}
}

type BoundChild =
	| {
			kind: "bound";
			agent_id: string;
			pane_id: string;
			source: string;
			incarnation: string;
	  }
	| Extract<RadarLaunchOutcome, { kind: "refused" | "unresolved" }>;

/**
 * Poll the edge this request id authored until a child presents its token.
 *
 * The daemon binds the edge once, to the exact registration the token produced,
 * so `bound.agent_id` is the child identity. A refusal or an unknown launch
 * effect ends the wait at once: neither becomes a child by waiting longer. An
 * edge whose recorded pane the daemon no longer reports ends it too: waiting
 * cannot create the pane a launch claims, and a location read afterwards would
 * be that of whatever moved in.
 */
async function waitForChild(
	client: RadarClient,
	requestId: string,
	options: {
		remaining: () => number;
		pollMs: number;
		signal?: AbortSignal;
	},
): Promise<BoundChild> {
	let last = "the daemon has not answered";
	for (;;) {
		if (options.signal?.aborted)
			return unresolvedLaunch(requestId, `the launch was cancelled (${last})`);
		const answer = await client.spawnGet(requestId);
		if (answer.ok === true) {
			const { edge, state, freshness } = answer.value;
			const standing = edgeStanding(edge);
			if (standing === "refused")
				return refusedLaunch(`${edgeMessage(edge)}`);
			if (standing === "unknown")
				return unresolvedLaunch(requestId, edgeMessage(edge));
			const pane = edge.location?.pane;
			if (pane === undefined || pane === "")
				return unresolvedLaunch(
					requestId,
					"the daemon's launch recorded no pane to verify the child against",
				);
			// The recorded pane is the daemon's own statement about where it created
			// and launched this child, and `fresh` is its inventory confirming that
			// pane right now. Without that confirmation the launch has no verified
			// location to give the child, and no later read can supply one.
			if (freshness !== "fresh")
				return unresolvedLaunch(
					requestId,
					state === "unresolved"
						? `the daemon's inventory does not report the pane ${pane} its own record names for this child`
						: `the daemon cannot confirm the pane ${pane} it recorded for this child`,
				);
			// An observed exit is the child's process, and a pane's shell can outlive it.
			if (edge.exit_observation?.state === "exited")
				return unresolvedLaunch(
					requestId,
					"the daemon observed the launched child's process exit",
				);
			const agentId = edge.bound?.agent_id;
			if (agentId !== undefined && isUuid(agentId)) {
				// A child the daemon bound without the process it registered as is a
				// child no assignment can be addressed to, and an unaddressed Radar
				// assignment is exactly what addressing exists to prevent: the launch
				// stays unresolved rather than degrading to a V5 request.
				const source = edge.bound?.source;
				const incarnation = edge.bound?.incarnation;
				if (
					source === undefined ||
					source === "" ||
					incarnation === undefined ||
					incarnation === ""
				)
					return unresolvedLaunch(
						requestId,
						`the daemon bound the child ${agentId} without recording the process it registered as`,
					);
				return {
					kind: "bound",
					agent_id: agentId,
					pane_id: pane,
					source,
					incarnation,
				};
			}
			last = "no child has registered with this launch's token yet";
		} else {
			last = answer.message;
		}
		if (options.remaining() <= 0)
			return unresolvedLaunch(requestId, last);
		await delay(options.pollMs, undefined, {
			...(options.signal !== undefined ? { signal: options.signal } : {}),
		});
	}
}
