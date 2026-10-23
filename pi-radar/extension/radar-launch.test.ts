import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { test } from "node:test";
import type { RadarClient } from "./radar-client.ts";
import { launchRadarWorker } from "./radar-launch.ts";
import type { RadarWorkerLaunch } from "./radar-launch.ts";

function launch(): RadarWorkerLaunch {
	return {
		request_id: randomUUID(),
		generation: randomUUID(),
		pane_id: "parent-pane",
		parent_session_id: randomUUID(),
		executable: "/usr/bin/pi",
		argv: [],
		cwd: "/workspace",
		env: {},
		mailbox: "/tmp/worker-mailbox",
		identity: {
			runId: randomUUID(),
			ownerSessionId: randomUUID(),
			workspaceId: randomUUID(),
			agentLabel: "worker",
		},
	};
}

function publication(subject_id: string | undefined, pane = "parent-pane") {
	return {
		subject: async () => ({
			registration_id: randomUUID(),
			...(subject_id !== undefined ? { subject_id } : {}),
			location: { backend: "herdr", pane },
		}),
	};
}

test("worker launch takes its parent from the caller's own publication", async () => {
	const parentSubject = randomUUID();
	const requests: Array<{ managed?: { parent_subject_id?: string } }> = [];
	const client = {
		spawn: async (request: (typeof requests)[number]) => {
			requests.push(request);
			return { ok: false as const, code: "refused" as const, message: "fixture stop" };
		},
	} as unknown as RadarClient;
	const result = await launchRadarWorker({
		client,
		publication: publication(parentSubject),
		launch: launch(),
		timeoutMs: 100,
		readers: {
			readState: () => undefined,
			waitForState: async () => { throw new Error("not reached"); },
			startupAckMatches: () => false,
		},
	});
	assert.equal(result.kind, "refused");
	assert.equal(requests[0]?.managed?.parent_subject_id, parentSubject);
});

test("worker launch refuses a caller with no managed subject before dispatch", async () => {
	let dispatched = false;
	const client = {
		spawn: async () => {
			dispatched = true;
			return { ok: false as const, code: "refused" as const, message: "unexpected" };
		},
	} as unknown as RadarClient;
	const result = await launchRadarWorker({
		client,
		publication: publication(undefined),
		launch: launch(),
		timeoutMs: 100,
		readers: {
			readState: () => undefined,
			waitForState: async () => { throw new Error("not reached"); },
			startupAckMatches: () => false,
		},
	});
	assert.equal(result.kind, "refused");
	assert.equal(dispatched, false);
});
