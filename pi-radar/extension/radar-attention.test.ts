import assert from "node:assert/strict";
import { test } from "node:test";
import { createRadarAttention, type RadarActivity } from "./radar-attention.ts";
import type { RadarMailboxPort } from "./radar-communication.ts";
import type { MessageSerialize } from "./generated-contracts.ts";

const MESSAGE = "11111111-1111-4111-8111-111111111111";
const MESSAGE_TWO = "22222222-2222-4222-8222-222222222222";

/** A clock the test advances, so an idle hold is observed without waiting. */
function clock() {
	let now = 0;
	let next = 1;
	const timers = new Map<number, { at: number; run: () => void }>();
	const scheduler = {
		setTimeout(handler: () => void, ms: number) {
			const id = next++;
			timers.set(id, { at: now + ms, run: handler });
			return { unref() {} };
		},
		clearTimeout(timer: { unref?: () => void }) {
			for (const [id, entry] of timers) if (entry === timers.get(id)) void entry;
			void timer;
		},
		setInterval() {
			return { unref() {} };
		},
		clearInterval(timer: { unref?: () => void }) {
			void timer;
		},
		now() {
			return now;
		},
	};
	function advance(ms: number) {
		now += ms;
		for (const [id, entry] of [...timers]) {
			if (entry.at <= now) {
				timers.delete(id);
				entry.run();
			}
		}
	}
	function pending() {
		return timers.size;
	}
	return { scheduler, advance, pending };
}

function message(id: string): MessageSerialize {
	return {
		version: 1,
		id,
		producer: {
			registration_id: "33333333-3333-4333-8333-333333333333",
			session_id: "44444444-4444-4444-8444-444444444444",
			subject_id: "55555555-5555-4555-8555-555555555555",
			claim_id: "66666666-6666-4666-8666-666666666666",
		},
		recipients: ["77777777-7777-4777-8777-777777777777"],
		body: { kind: "handoff", summary: "done", content: "body" },
		created_at_unix_ms: 1,
	};
}

type Recorded = { captured: string[]; received: string[]; failed: Set<string> };

function mailbox(ids: string[], recorded: Recorded, options: { failPage?: boolean } = {}): RadarMailboxPort {
	let pageCalls = 0;
	return {
		async publish() {
			throw new Error("not used");
		},
		async page() {
			pageCalls += 1;
			if (options.failPage === true) throw new Error("page failed");
			// One page, then a continuation the test can ignore.
			return pageCalls === 1
				? { ok: true, value: { message_ids: ids, next: null } }
				: { ok: true, value: { message_ids: [], next: null } };
		},
		async get(id) {
			if (recorded.failed.has(id)) return { ok: false, code: "refused", message: "get failed" } as never;
			return {
				ok: true,
				value: { message: message(id), receipt: null },
			};
		},
		async receive(id) {
			recorded.received.push(id);
			return {
				ok: true,
				value: {
					receipt: {
						version: 1,
						message_id: id,
						recipient: message(id).producer,
						received_at_unix_ms: 1,
					},
					created: true,
				},
			};
		},
	};
}

/** Lets the drain's promise chain run to completion. */
const settle = async (): Promise<void> => {
	for (let turn = 0; turn < 12; turn += 1) await new Promise((resolve) => setImmediate(resolve));
};

test("a captured message is received, and an unrecorded one is not", async () => {
	const recorded: Recorded = { captured: [], received: [], failed: new Set() };
	const attention = createRadarAttention({
		mailbox: mailbox([MESSAGE, MESSAGE_TWO], recorded),
		consumers: [
			{
				kind: "handoff",
				capture(message) {
					recorded.captured.push(message.id);
					return message.id !== MESSAGE_TWO;
				},
			},
		],
		scheduler: clock().scheduler,
		activity: () => "working",
	});
	attention.drain();
	await settle();
	assert.deepEqual(recorded.captured, [MESSAGE, MESSAGE_TWO]);
	assert.deepEqual(
		recorded.received,
		[MESSAGE],
		"only the durably captured message is received; a failed capture stays pending",
	);
});

test("overlapping triggers run one drain", async () => {
	const recorded: Recorded = { captured: [], received: [], failed: new Set() };
	let drains = 0;
	const port = mailbox([MESSAGE], recorded);
	const attention = createRadarAttention({
		mailbox: {
			...port,
			async page(after) {
				drains += 1;
				return port.page(after);
			},
		},
		consumers: [{ kind: "handoff", capture: () => true }],
		scheduler: clock().scheduler,
		activity: () => "working",
	});
	attention.drain();
	attention.hint({ revision: "a", pending: 1 });
	attention.hint({ revision: "b", pending: 1 });
	await settle();
	await settle();
	assert.equal(drains, 1, "a second page pass would consume the same message twice");
	assert.deepEqual(recorded.received, [MESSAGE]);
});

test("an unchanged revision does not suppress a retry", async () => {
	const recorded: Recorded = { captured: [], received: [], failed: new Set() };
	let attempts = 0;
	const port = mailbox([MESSAGE], recorded);
	const attention = createRadarAttention({
		mailbox: {
			...port,
			async page(after) {
				attempts += 1;
				return port.page(after);
			},
		},
		consumers: [{ kind: "handoff", capture: () => false }],
		scheduler: clock().scheduler,
		activity: () => "working",
	});
	attention.hint({ revision: "same", pending: 1 });
	await settle();
	await settle();
	attention.hint({ revision: "same", pending: 1 });
	await settle();
	await settle();
	assert.equal(attempts, 2, "an unconsumed pending message must stay eligible");
});

test("the subscription is dropped only after continuous idle", async () => {
	const recorded: Recorded = { captured: [], received: [], failed: new Set() };
	const fake = clock();
	let activity: RadarActivity = "waiting";
	let opened = 0;
	let closed = 0;
	const attention = createRadarAttention({
		mailbox: mailbox([], recorded),
		consumers: [{ kind: "handoff", capture: () => true }],
		scheduler: fake.scheduler,
		activity: () => activity,
		watch: async () => {
			opened += 1;
			return { ok: true as const, value: { close: () => void closed++ } };
		},
	});
	attention.drain();
	await settle();
	assert.equal(opened, 1, "a waiting agent subscribes: it is waiting for work");
	fake.advance(60_000);
	assert.equal(closed, 0, "waiting is not idle, so the hold never starts");
	activity = "idle";
	attention.activity();
	fake.advance(29_000);
	assert.equal(closed, 0);
	fake.advance(1_000);
	assert.equal(closed, 1, "continuous idle releases the subscription");
	activity = "working";
	attention.activity();
	await settle();
	assert.equal(opened, 2, "resuming activity re-establishes the subscription");
});

test("a refused subscription falls back to heartbeats and retries", async () => {
	const recorded: Recorded = { captured: [], received: [], failed: new Set() };
	const fake = clock();
	let attempts = 0;
	let closed = 0;
	const attention = createRadarAttention({
		mailbox: mailbox([], recorded),
		consumers: [{ kind: "handoff", capture: () => true }],
		scheduler: fake.scheduler,
		activity: () => "working",
		watch: async () => {
			attempts += 1;
			return attempts === 1
				? { ok: false as const, code: "busy", message: "no slots" }
				: { ok: true as const, value: { close: () => void closed++ } };
		},
	});
	attention.drain();
	await settle();
	assert.equal(attempts, 1);
	fake.advance(2_000);
	await settle();
	assert.equal(attempts, 2, "a refusal is retried with backoff");
	attention.stop();
	assert.equal(closed, 1, "stopping releases the subscription");
});
