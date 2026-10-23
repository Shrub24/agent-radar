import { setTimeout as delay } from "node:timers/promises";
import {
	isUuid,
	type RadarClient,
	type RadarErrorCode,
	type RadarLocation,
	type RadarResult,
} from "./radar-client.ts";
import type { RadarPublication } from "./radar-publication.ts";

/** A launch succeeds when Radar grants its child's verified first presence. */

/** Everything one fresh managed worker is launched with. */
export type RadarWorkerLaunch = {
	/** The caller's durable key, persisted before dispatch and never replaced. */
	request_id: string;
	/**
	 * Start this released child agent again rather than a new one. The daemon takes
	 * the session from its own record, so no session id or path is sent.
	 */
	continues?: string;
	/**
	 * The pane this launch splits. The parent is the calling process itself, read
	 * from its own publication, and must be registered on exactly this pane.
	 */
	pane_id: string;
	parent_session_id: string;
	executable: string;
	argv: string[];
	cwd: string;
	/** The environment overlay the child needs, not a replacement environment. */
	env: Record<string, string>;
};

export type RadarWorkerLaunched = {
	request_id: string;
	/** The child's durable identity: the key that survives its restart and resume. */
	subject_id: string;
	pane_id: string;
};

/**
 * `ready` is a child whose first presence the daemon granted. `failed` is a launch
 * the daemon withdrew (or that provably never began), so a retry is safe.
 * `unresolved` is the rare launch whose outcome cannot be read at all, which the
 * caller reports rather than retries.
 */
export type RadarLaunchOutcome =
	| { kind: "ready"; value: RadarWorkerLaunched }
	| { kind: "failed"; message: string }
	| { kind: "unresolved"; request_id: string; message: string };

/**
 * What the daemon's current read proves about the child of one launch: the exact
 * subject the edge is bound to and the pane it recorded, both taken from the
 * daemon's own answer rather than from this process's record of the launch.
 */
export type RadarChildEvidence = {
	request_id: string;
	registration_id: string;
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
 * child is gone. A presence grant is startup evidence, not perpetual liveness.
 * Only a current topology read decides: `bound` has to name
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
		registration_id: string | undefined;
		/** The session this launch's mailbox records for the child. */
		session_id: string | undefined;
	},
): Promise<RadarChildObservation> {
	if (provenance.registration_id === undefined)
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
	if (edge.bound === undefined || edge.bound.registration_id === undefined)
		return unknownChild("not bound to a child by the daemon");
	if (edge.bound.registration_id !== provenance.registration_id)
		return unknownChild(
			`bound to another subject (${edge.bound.registration_id}), not to ${provenance.registration_id}`,
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
	const reply = await client.agentGet(edge.bound.registration_id);
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
			registration_id: edge.bound.registration_id,
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

export type RadarLaunchOptions = {
	client: RadarClient;
	/** This process's own publication, which names the parent of the launch. */
	publication: Pick<RadarPublication, "subject">;
	launch: RadarWorkerLaunch;
	/** How often the wait re-reads the launch; 0 in driven tests. */
	pollMs?: number;
	/** Stops the wait. The daemon's own deadline still withdraws the launch. */
	signal?: AbortSignal;
};

function refusedLaunch(message: string): Extract<RadarLaunchOutcome, { kind: "failed" | "unresolved" }> {
	return { kind: "failed", message };
}

function unresolvedLaunch(
	requestId: string,
	message: string,
): Extract<RadarLaunchOutcome, { kind: "failed" | "unresolved" }> {
	return {
		kind: "unresolved",
		request_id: requestId,
		message: `spawn ${requestId} is unresolved (${message})`,
	};
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

export function launchFailure(
	result: Extract<RadarResult<unknown>, { ok: false }>,
	requestId: string,
): Extract<RadarLaunchOutcome, { kind: "failed" | "unresolved" }> {
	return PRE_EFFECT_CODES.includes(result.code)
		? refusedLaunch(result.message)
		: unresolvedLaunch(requestId, result.message);
}

/**
 * Wait for the child's verified presence grant or the daemon's withdrawal. The
 * same request UUID is read throughout; a lost reply never starts another child.
 */
export async function launchRadarWorker(
	options: RadarLaunchOptions,
): Promise<RadarLaunchOutcome> {
	const { client, launch, signal } = options;
	const pollMs = options.pollMs ?? 250;
	const parent = await options.publication.subject();
	if (parent === undefined || !isUuid(parent.registration_id))
		return refusedLaunch(
			"this process has no Radar registration, so a worker cannot be spawned under it",
		);
	if (parent.subject_id === undefined || !isUuid(parent.subject_id))
		return refusedLaunch(
			"this process has no managed Radar subject, so a worker cannot be spawned under it",
		);
	// A spawn is created under the pane its parent registered, and this port
	// registers no location it was not given: without one there is nothing to
	// place the child beside, and a name or session lookup is not a substitute.
	const pane = parent.location?.pane;
	if (pane === undefined || pane === "")
		return refusedLaunch(
			"this process's Radar registration names no pane, so a worker cannot be placed beside it",
		);
	if (pane !== launch.pane_id)
		return refusedLaunch(
			`this process's Radar registration places it in pane ${pane}, not in the pane this launch would split`,
		);

	const spawned = await client.spawn({
		request_id: launch.request_id,
		parent: parent.registration_id,
		parent_session_id: launch.parent_session_id,
		executable: launch.executable,
		managed: { parent_subject_id: parent.subject_id },
		...(launch.continues !== undefined ? { continue_subject_id: launch.continues } : {}),
		argv: launch.argv,
		...(launch.cwd !== "" ? { cwd: launch.cwd } : {}),
		...(Object.keys(launch.env).length > 0 ? { env: launch.env } : {}),
	});
	if (spawned.ok === false) {
		const refusal = launchFailure(spawned, launch.request_id);
		if (refusal.kind === "failed") return refusal;
	}

	const bound = await waitForChild(client, launch.request_id, {
		pollMs,
		...(signal !== undefined ? { signal } : {}),
	});
	if (bound.kind !== "bound") return bound;
	return {
		kind: "ready",
		value: {
			request_id: launch.request_id,
			subject_id: bound.subject_id,
			pane_id: bound.pane_id,
		},
	};
}

type BoundChild =
	| {
			kind: "bound";
			subject_id: string;
			pane_id: string;
	  }
	| Extract<RadarLaunchOutcome, { kind: "failed" | "unresolved" }>;

/**
 * Poll the edge this request id authored until the daemon grants its child or
 * withdraws the launch. There is no timeout here: the daemon owns the deadline and
 * records a withdrawal on the edge, so waiting longer than it would only wait for
 * an outcome that is already decided.
 *
 * Registration binding alone is not success: a pending or duplicate presence
 * can bind an edge without ever acquiring attachment authority.
 */
export async function waitForChild(
	client: Pick<RadarClient, "spawnGet">,
	requestId: string,
	options: {
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
			const { edge } = answer.value;
			if (edge.withdrawn !== undefined)
				return refusedLaunch(`the daemon withdrew this launch: ${edge.withdrawn.reason}`);
			if (edge.granted_at === undefined) {
				await delay(options.pollMs);
				continue;
			}
			const pane = edge.location?.pane;
			if (pane === undefined || pane === "")
				return unresolvedLaunch(
					requestId,
					"the daemon's launch recorded no pane to verify the child against",
				);

			const agentId = edge.bound?.registration_id;
			if (agentId !== undefined && isUuid(agentId)) {
				// The edge names the agent as well as its process. Without it the child
				// has no durable key to be returned by, and a registration is never
				// offered in its place.
				const subjectId = edge.bound?.subject_id;
				if (subjectId === undefined || !isUuid(subjectId))
					return unresolvedLaunch(
						requestId,
						`the daemon bound the child ${agentId} without recording its subject`,
					);
				return {
					kind: "bound",
					subject_id: subjectId,
					pane_id: pane,
				};
			}
			last = "no child has registered with this launch's token yet";
		} else {
			return unresolvedLaunch(requestId, answer.message);
		}
		await delay(options.pollMs);
	}
}
