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
