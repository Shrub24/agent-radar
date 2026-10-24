import { randomUUID } from "node:crypto";
import type { RadarClient, RadarChannelSnapshot, RadarResult } from "./radar-client.ts";
import type { SubjectGetResult, SubjectChildrenParams } from "./generated-contracts.ts";
import type { RadarInspection } from "./radar-inspection.ts";
import { waitForChild, launchFailure, type RadarLaunchOutcome } from "./radar-launch.ts";
import type { RadarPublication } from "./radar-publication.ts";

/** An agent, including its retained identity when no process is attached. */
export type RadarChild = SubjectGetResult["subject"];
export type RadarChildAssignment = {
	update(snapshot: RadarChannelSnapshot): void;
	retire(): void;
};
export type RadarChildProgram = {
	executable: string;
	argv: string[];
	cwd?: string;
	env?: Record<string, string>;
};
export type RadarChildObservation =
	| { kind: "live"; subject: RadarChild }
	| { kind: "gone"; subject: RadarChild }
	| { kind: "unknown"; diagnostic: string };
export type RadarChildren = {
	list(selection?: SubjectChildrenParams["selection"]): Promise<RadarResult<RadarChild[]>>;
	restart(subject: string, program: RadarChildProgram, options?: { pollMs?: number; signal?: AbortSignal }): Promise<RadarLaunchOutcome>;
	assignment(subject: string, owner: string): RadarChildAssignment;
	retire(subject: string): void;
	observe(subject: string): Promise<RadarChildObservation>;
	inspect(subject: string): Promise<RadarInspection>;
};

/** All child lifecycle resolution and assignment ownership belong to Radar. */
export function createRadarChildren(
	client: Pick<RadarClient, "subjectChildren" | "childGet" | "childInspect" | "spawnGet" | "restart">,
	publication: Pick<RadarPublication, "assignment" | "retireAssignment" | "subject">,
): RadarChildren {
	return {
		async list(selection) {
			const children: RadarChild[] = [];
			let after: string | undefined;
			do {
				const page = await client.subjectChildren({ selection, after, limit: 100 });
				if (!page.ok) return page;
				children.push(...page.value.subjects);
				if (page.value.next === after)
					return { ok: false, code: "protocol", message: "child page did not advance" };
				after = page.value.next ?? undefined;
			} while (after !== undefined);
			return { ok: true, value: children };
		},
		async restart(subject, program, options) {
			const parent = await publication.subject();
			if (parent?.subject_id === undefined)
				return { kind: "failed", message: "this process has no granted subject" };
			const request_id = randomUUID();
			const requested = await client.restart({ request_id, parent: parent.registration_id, subject_id: subject, ...program });
			if (!requested.ok) return launchFailure(requested, request_id);
			const bound = await waitForChild(client, requested.value.launch_request_id, {
				pollMs: options?.pollMs ?? 250,
				...(options?.signal !== undefined ? { signal: options.signal } : {}),
			});
			if (bound.kind !== "bound") return bound;
			return { kind: "ready", value: { request_id: requested.value.launch_request_id, subject_id: bound.subject_id, pane_id: bound.pane_id } };
		},
		assignment: (subject, owner) => publication.assignment(subject, owner),
		retire: (subject) => publication.retireAssignment(subject),
		async observe(subject) {
			const read = await client.childGet(subject);
			if (!read.ok) return { kind: "unknown", diagnostic: `${read.code}: ${read.message}` };
			if (read.value.state === "live") return { kind: "live", subject: read.value };
			if (read.value.state === "detached") return { kind: "gone", subject: read.value };
			return { kind: "unknown", diagnostic: `child attachment is ${read.value.state}` };
		},
		async inspect(subject) {
			const read = await client.childInspect(subject);
			if (!read.ok) throw new Error(`Child inspection unavailable (${read.code}: ${read.message})`);
			return read.value;
		},
	};
}
