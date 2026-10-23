import type { RadarClient, RadarChannelSnapshot, RadarResult } from "./radar-client.ts";
import type { SpawnListPageSerializePublicSpawnTopology as ListedEdge } from "./generated-contracts.ts";
import { inspectRadarChild, type RadarInspection } from "./radar-inspection.ts";
import { observeRadarChild, type RadarChildObservation } from "./radar-launch.ts";
import type { RadarPublication } from "./radar-publication.ts";

const PAGE = 100;
const MAX_PAGES = 20;

/** One child of the caller, named by the agent that survives its restarts. */
export type RadarChild = {
	subject_id: string;
	/** The launch that last bound this child. */
	request_id: string;
	pane_id?: string;
	state: "unbound" | "bound" | "unresolved";
	exited: boolean;
};

/** The assignment a child is held to: published again when its process is replaced. */
export type RadarChildAssignment = {
	update(snapshot: RadarChannelSnapshot): void;
	/** Publish the held assignment on the child's current process. Call after a replacement. */
	refresh(): Promise<void>;
	retire(): void;
};

export type RadarChildren = {
	list(): Promise<RadarResult<RadarChild[]>>;
	assignment(subject: string, owner: string): RadarChildAssignment;
	retire(subject: string): void;
	observe(
		subject: string,
		session_id: string | undefined,
	): Promise<RadarChildObservation>;
	inspect(subject: string, session_id?: string): Promise<RadarInspection>;
};

type Resolved = { child: RadarChild; registration_id: string };

/**
 * The caller's children, addressed by subject. A subject gains an edge per launch,
 * so the newest bound edge is the current process; its registration is read per
 * call and never remembered, because a remembered registration is the stale key
 * this surface exists to hide. An edge bound without a subject is not addressable.
 */
export function createRadarChildren(
	client: Pick<RadarClient, "spawnList" | "spawnGet" | "agentGet" | "output" | "processInfo">,
	publication: Pick<RadarPublication, "assignment" | "retireAssignment">,
): RadarChildren {
	const held = new Map<string, { writer: RadarChildAssignment; published?: string }>();

	async function resolveAll(): Promise<RadarResult<Map<string, Resolved & { at: number }>>> {
		const newest = new Map<string, Resolved & { at: number }>();
		let after: string | undefined;
		for (let page = 0; page < MAX_PAGES; page++) {
			const read = await client.spawnList({ ...(after !== undefined ? { after } : {}), limit: PAGE });
			if (read.ok === false) return read;
			for (const edge of read.value.spawns as ListedEdge[]) {
				const bound = edge.bound;
				if (!bound?.subject_id || !bound.registration_id || edge.launched !== "completed") continue;
				const at = Date.parse(bound.bound_at);
				if (Number.isNaN(at)) continue;
				const current = newest.get(bound.subject_id);
				if (current !== undefined && current.at >= at) continue;
				newest.set(bound.subject_id, {
					at,
					registration_id: bound.registration_id,
					child: {
						subject_id: bound.subject_id,
						request_id: edge.request_id,
						...(edge.location?.pane ? { pane_id: edge.location.pane } : {}),
						state: edge.state,
						exited: edge.exit_observation != null && edge.exit_observation.state !== "unknown",
					},
				});
			}
			if (!read.value.next) return { ok: true, value: newest };
			after = read.value.next;
		}
		return { ok: false, code: "protocol", message: "the caller's launches exceed the readable pages" };
	}

	async function resolve(subject: string): Promise<Resolved | undefined> {
		const all = await resolveAll();
		return all.ok ? all.value.get(subject) : undefined;
	}

	return {
		async list() {
			const all = await resolveAll();
			return all.ok
				? { ok: true, value: [...all.value.values()].map((entry) => entry.child) }
				: all;
		},
		assignment(subject, owner) {
			const existing = held.get(subject);
			if (existing !== undefined) return existing.writer;
			let latest: RadarChannelSnapshot | undefined;
			const entry: { writer: RadarChildAssignment; published?: string } = {
				writer: undefined as unknown as RadarChildAssignment,
			};
			const publish = async (): Promise<void> => {
				const target = await resolve(subject);
				if (target === undefined || latest === undefined) return;
				if (entry.published !== undefined && entry.published !== target.registration_id)
					publication.retireAssignment(entry.published);
				entry.published = target.registration_id;
				publication.assignment(target.registration_id, owner).update(latest);
			};
			entry.writer = {
				update(snapshot) {
					latest = snapshot;
					void publish();
				},
				refresh: publish,
				retire() {
					latest = undefined;
					if (entry.published !== undefined) publication.retireAssignment(entry.published);
					entry.published = undefined;
				},
			};
			held.set(subject, entry);
			return entry.writer;
		},
		retire(subject) {
			held.get(subject)?.writer.retire();
			held.delete(subject);
		},
		async observe(subject, session_id) {
			const target = await resolve(subject);
			if (target === undefined)
				return { kind: "unknown", diagnostic: "the daemon records no bound launch for this child" };
			return observeRadarChild(client as RadarClient, {
				request_id: target.child.request_id,
				pane_id: target.child.pane_id,
				registration_id: target.registration_id,
				session_id,
			});
		},
		async inspect(subject, session_id) {
			const target = await resolve(subject);
			if (target?.child.pane_id === undefined)
				throw new Error("Inspection target is not a bound child with a pane");
			return inspectRadarChild(client as RadarClient, {
				agentId: target.registration_id,
				paneId: target.child.pane_id,
				...(session_id !== undefined ? { session: session_id } : {}),
			});
		},
	};
}
