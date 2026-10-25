import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { test } from "node:test";
import { createRadarChildren } from "./radar-children.ts";

const subject = randomUUID();

function child(state: "starting" | "live" | "detached" | "unknown") {
	return {
		subject_id: subject,
		session_id: randomUUID(),
		managed: {},
		state,
	};
}

function harness(children: object[]) {
	const published: Array<{ target: string; text: unknown }> = [];
	const retired: string[] = [];
	const client = {
		subjectChildren: async () => ({ ok: true as const, value: { subjects: children, next: null } }),
		childGet: async () => ({ ok: true as const, value: children[0] }),
	};
	return {
		published,
		retired,
		children: createRadarChildren(client as never, {
			assignment: (target: string) => ({ update: (snapshot: unknown) => published.push({ target, text: snapshot }) }),
			retireAssignment: (target: string) => retired.push(target),
			subject: async () => ({ registration_id: randomUUID(), subject_id: randomUUID() }),
		} as never),
	};
}

test("children are listed and observed by subject, including a retained one", async () => {
	const live = harness([child("live")]);
	const listed = await live.children.list();
	assert.deepEqual(listed.ok && listed.value.map((entry) => entry.subject_id), [subject]);
	assert.equal((await live.children.observe(subject)).kind, "live");

	const retained = harness([child("detached")]);
	const observed = await retained.children.observe(subject);
	assert.equal(observed.kind, "gone");
	assert.equal(observed.kind === "gone" && observed.subject.subject_id, subject);
	assert.equal((await retained.children.observe(subject)).kind === "gone", true);

	const unreadable = harness([child("unknown")]);
	assert.equal((await unreadable.children.observe(subject)).kind, "unknown");
});

test("an assignment is published for the subject Radar resolves, never for a child process", async () => {
	const h = harness([child("live")]);
	const assignment = h.children.assignment(subject, "owner");
	assignment.update({ text: "task" } as never);
	assert.deepEqual(h.published, [{ target: subject, text: { text: "task" } }]);
	h.children.retire(subject);
	assert.deepEqual(h.retired, [subject]);
});

test("closing a child names only its subject and reports what Radar did", async () => {
	const sent: object[] = [];
	const h = createRadarChildren({
		childClose: async (request: object) => { sent.push(request); return { ok: true as const, value: { outcome: "completed" as const, pane: "p" } }; },
	} as never, {} as never);
	const id = randomUUID();
	assert.deepEqual(await h.close(subject, "complete", id), { outcome: "completed", pane: "p" });
	assert.deepEqual(sent, [{ request_id: id, subject_id: subject, intent: "complete" }]);

	const refused = createRadarChildren({
		childClose: async () => ({ ok: false as const, code: "refused", message: "subject is not this caller's child" }),
	} as never, {} as never);
	assert.equal((await refused.close(subject, "cancel")).outcome, "refused");
});

test("a close whose answer was lost is unknown, never refused", async () => {
	const lost = createRadarChildren({
		childClose: async () => ({ ok: false as const, code: "absent", message: "socket gone" }),
	} as never, {} as never);
	assert.equal((await lost.close(subject, "cancel")).outcome, "unknown");
});
