import assert from "node:assert/strict";
import { test } from "node:test";
import type { RadarClient } from "./radar-client.ts";
import { createRadarHandoffPort } from "./radar-communication.ts";
import type { RadarPublication } from "./radar-publication.ts";

function harness(subject: { agent_id: string } | undefined, parentSubject?: string) {
	const calls: unknown[][] = [];
	const client = {
		communicationPublish: async (request: unknown) => {
			calls.push(["publish", request]);
			return { ok: true as const, value: { handoff: {} as never, created: true } };
		},
		communicationInbox: async (request: unknown) => {
			calls.push(["inbox", request]);
			return { ok: true as const, value: { handoff_ids: [] } };
		},
		communicationGet: async (request: unknown) => {
			calls.push(["get", request]);
			return { ok: true as const, value: { handoff: {} as never, receipt: null } };
		},
		communicationAck: async (request: unknown) => {
			calls.push(["ack", request]);
			return { ok: true as const, value: { receipt: {} as never, created: true } };
		},
	} as unknown as RadarClient;
	const publication = { subject: async () => subject } as unknown as RadarPublication;
	return { calls, port: createRadarHandoffPort(client, publication, () => parentSubject) };
}

test("handoff port binds sender to this registration and routes only to its managed parent", async () => {
	const ownAgent = "11111111-1111-4111-8111-111111111111";
	const parentSubject = "22222222-2222-4222-8222-222222222222";
	const { calls, port } = harness({ agent_id: ownAgent }, parentSubject);
	await port.publish({
		request_id: "33333333-3333-4333-8333-333333333333",
		summary: "result",
		content: "captured result",
	});
	await port.inbox();
	await port.get("44444444-4444-4444-8444-444444444444");
	await port.ack("44444444-4444-4444-8444-444444444444");
	assert.deepEqual(calls.map(([name]) => name), ["publish", "inbox", "get", "ack"]);
	assert.deepEqual(calls[0]?.[1], {
		request_id: "33333333-3333-4333-8333-333333333333",
		summary: "result",
		content: "captured result",
		sender_agent_id: ownAgent,
		recipient_subject_id: parentSubject,
	});
	assert.deepEqual(calls.slice(1).map(([, request]) => request), [
		{ agent_id: ownAgent },
		{ agent_id: ownAgent, handoff_id: "44444444-4444-4444-8444-444444444444" },
		{ agent_id: ownAgent, handoff_id: "44444444-4444-4444-8444-444444444444" },
	]);
});

test("handoff port refuses to act without a current registration or managed parent", async () => {
	const ownAgent = "11111111-1111-4111-8111-111111111111";
	const unregistered = harness(undefined, "22222222-2222-4222-8222-222222222222");
	const noRegistration = await unregistered.port.inbox();
	assert.equal(noRegistration.ok, false);
	assert.equal(unregistered.calls.length, 0);

	const root = harness({ agent_id: ownAgent });
	const noParent = await root.port.publish({
		request_id: "33333333-3333-4333-8333-333333333333",
		summary: "result",
		content: "captured result",
	});
	assert.equal(noParent.ok, false);
	assert.equal(root.calls.length, 0);
});
