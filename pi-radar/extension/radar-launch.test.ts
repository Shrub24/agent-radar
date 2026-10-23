import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { test } from "node:test";
import type { RadarClient } from "./radar-client.ts";
import { launchRadarWorker, type RadarWorkerLaunch } from "./radar-launch.ts";

function launch(): RadarWorkerLaunch {
	return {
		request_id: randomUUID(),
		pane_id: "parent-pane",
		parent_session_id: randomUUID(),
		executable: "/usr/bin/pi",
		argv: [],
		cwd: "/workspace",
		env: {},
	};
}

function publication(subject_id: string | undefined) {
	return {
		subject: async () => ({
			registration_id: randomUUID(),
			...(subject_id !== undefined ? { subject_id } : {}),
			location: { backend: "herdr", pane: "parent-pane" },
		}),
	};
}

test("worker launch takes its parent from the caller's own publication", async () => {
	const subject = randomUUID();
	let parent: string | undefined;
	const client = {
		spawn: async (request: { managed?: { parent_subject_id?: string } }) => {
			parent = request.managed?.parent_subject_id;
			return { ok: false, code: "refused", message: "fixture stop" };
		},
	} as unknown as RadarClient;
	const result = await launchRadarWorker({ client, publication: publication(subject), launch: launch() });
	assert.equal(result.kind, "failed");
	assert.equal(parent, subject);
});

test("a caller with no granted subject cannot launch", async () => {
	const client = { spawn: async () => { throw new Error("must not dispatch"); } } as unknown as RadarClient;
	const result = await launchRadarWorker({ client, publication: publication(undefined), launch: launch() });
	assert.equal(result.kind, "failed");
});

test("a bound child is not ready until presence is granted, or failed until withdrawal is confirmed", async () => {
	for (const terminal of ["granted", "withdrawn"] as const) {
		const subject = randomUUID();
		let reads = 0;
		const edge = {
			request_id: randomUUID(), parent: randomUUID(), created: "completed", launched: "completed",
			location: { backend: "tmux", pane: "%7" },
			bound: { registration_id: randomUUID(), subject_id: subject, source: "pi", incarnation: randomUUID() },
		};
		const client = {
			spawn: async () => ({ ok: true, value: edge }),
			spawnGet: async () => ({
				ok: true,
				value: { edge: {
					...edge,
					...(++reads < 2 ? {} : terminal === "granted"
						? { granted_at: "2026-01-01T00:00:01.000Z" }
						: { withdrawn: { at: "2026-01-01T00:00:30.000Z", reason: "the launch did not present within 30 seconds" } }),
				} },
			}),
		} as unknown as RadarClient;
		const result = await launchRadarWorker({ client, publication: publication(randomUUID()), launch: launch(), pollMs: 0 });
		assert.equal(reads, 2, "registration binding alone must not settle the launch");
		if (terminal === "granted") {
			assert.equal(result.kind, "ready");
			if (result.kind === "ready") assert.equal(result.value.subject_id, subject);
		} else {
			assert.deepEqual(result, { kind: "failed", message: "the daemon withdrew this launch: the launch did not present within 30 seconds" });
		}
	}
});
