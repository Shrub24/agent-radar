import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { test } from "node:test";
import { createRadarChildren } from "./radar-children.ts";

const subject = randomUUID();

function edge(registration: string, boundAt: string, extra: object = {}) {
	return {
		request_id: randomUUID(),
		launched: "completed",
		state: "bound",
		location: { backend: "tmux", pane: "%7" },
		bound: { source: "herdsman-pi", incarnation: randomUUID(), registration_id: registration, subject_id: subject, bound_at: boundAt },
		...extra,
	};
}

function harness(spawns: object[]) {
	const published: Array<{ target: string; text: unknown }> = [];
	const retired: string[] = [];
	const client = { spawnList: async () => ({ ok: true as const, value: { spawns } }) };
	const publication = {
		assignment: (target: string) => ({ update: (snapshot: unknown) => published.push({ target, text: snapshot }) }),
		retireAssignment: (target: string) => retired.push(target),
	};
	return {
		published,
		retired,
		children: createRadarChildren(client as never, publication as never),
	};
}

test("a child replaced under the same subject is reached by its subject and carries its assignment", async () => {
	const first = randomUUID();
	const second = randomUUID();
	const spawns = [edge(first, "2026-01-01T00:00:00.000Z")];
	const h = harness(spawns);
	const assignment = h.children.assignment(subject, "owner");
	assignment.update({ text: "task" } as never);
	await new Promise((resolve) => setImmediate(resolve));
	assert.deepEqual(h.published.map((entry) => entry.target), [first]);

	spawns.push(edge(second, "2026-01-02T00:00:00.000Z"));
	await assignment.refresh();
	assert.deepEqual(h.published.map((entry) => entry.target), [first, second]);
	assert.deepEqual(h.retired, [first]);
	assert.equal((await h.children.list() as { value: unknown[] }).value.length, 1);
});

test("an edge bound without a subject is not a child of this surface", async () => {
	const legacy = edge(randomUUID(), "2026-01-01T00:00:00.000Z");
	delete (legacy.bound as { subject_id?: string }).subject_id;
	const h = harness([legacy]);
	assert.deepEqual(await h.children.list(), { ok: true, value: [] });
});
