import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { chmodSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import type {
	RadarResult,
	RadarScheduler,
} from "./radar-client.ts";
import {
	createRadarExecutionAdapter,
	type RadarExecutionAdapter,
} from "./radar-execution.ts";
import {
	createRadarProcessSlot,
	radarBindingKey,
	type RadarPublicationClient,
	type RadarProcessSlot,
} from "./radar-publication.ts";

const INCARNATION = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const ok = <T>(value: T): RadarResult<T> => ({ ok: true, value });

/** A clock that never fires: this adapter's tests drive it from Pi's events. */
const frozenScheduler: RadarScheduler = {
	setTimeout: () => ({ unref: () => {} }),
	clearTimeout: () => {},
	setInterval: () => ({ unref: () => {} }),
	clearInterval: () => {},
	now: () => 1_700_000_000_000,
};

function harness(
	options: {
		managed?: boolean;
		slot?: RadarProcessSlot;
		root?: string;
		scheduler?: RadarScheduler;
	} = {},
) {
	const handlers = new Map<
		string,
		Array<(event: any, context: any) => unknown>
	>();
	const events = new EventEmitter();
	const root =
		options.root ?? mkdtempSync(join(tmpdir(), "radar-execution-test-"));
	const calls: Array<{ method: string; params: any }> = [];
	let minted = 0;
	const client: RadarPublicationClient = {
		ping: async () => ok({
			protocol: 2,
			revision: 1,
			backend: "tmux",
			instance: null,
			contracts: ["registry.v1", "registry.facts.v1"],
			tasks: { available: false, error: null },
		}),
		register: async (params) => {
			calls.push({ method: "agent.register", params });
			minted += 1;
			return ok({
				agent_id: `00000000-0000-4000-8000-${String(minted).padStart(12, "0")}`,
			});
		},
		acquire: async (params) => {
			calls.push({ method: "agent.acquire", params });
			minted += 1;
			return ok({
				handle: `00000000-0000-4000-8000-${String(minted).padStart(12, "0")}`,
				source: params.publisher.source,
				incarnation: params.publisher.incarnation,
				generation: 1,
				sequence: 0,
			});
		},
		publish: async (params) => {
			calls.push({ method: "agent.publish", params });
			return ok({ sequence: params.sequence });
		},
		context: async (params) => {
			calls.push({ method: "agent.context", params });
			minted += 1;
			return ok({
				writer: {
					handle:
						params.writer_handle ??
						`00000000-0000-4000-8000-${String(minted).padStart(12, "0")}`,
					source: params.publisher.source,
					incarnation: params.publisher.incarnation,
					generation: 1,
					sequence: params.sequence,
				},
			});
		},
		retire: async (params) => {
			calls.push({ method: "agent.retire", params });
			return ok(undefined);
		},
	};
	const key = radarBindingKey("run-1", "owner-1", "worker");
	const adapter: RadarExecutionAdapter = createRadarExecutionAdapter(
		{
			on(name: string, handler: (event: any, context: any) => unknown) {
				const list = handlers.get(name) ?? [];
				handlers.set(name, [...list, handler]);
				return () => {
					handlers.set(
						name,
						(handlers.get(name) ?? []).filter((entry) => entry !== handler),
					);
				};
			},
			events,
		},
		{
			client,
			dataRoot: root,
			slot: options.slot ?? createRadarProcessSlot(),
			incarnation: INCARNATION,
			questionnaireEvent: "rpiv:ask-user:blocked",
			// Present and undefined: this platform reports no process claim, and
			// absence is never inferred from a later successful read.
			birth: undefined,
			scheduler: options.scheduler ?? frozenScheduler,
			...(options.managed
				? {
						managedBinding: {
							key,
							run: "run-1",
							owner: "owner-1",
							label: "worker",
						},
					}
				: {}),
		},
	);
	const snapshots = () =>
		calls
			.filter((call) => call.method === "agent.publish")
			.map((call) => call.params.snapshot);
	const contexts = () =>
		calls
			.filter((call) => call.method === "agent.context")
			.map((call) => call.params);
	const emit = (
		name: string,
		event: unknown = {},
		context: unknown = {},
	): void => {
		for (const handler of [...(handlers.get(name) ?? [])]) handler(event, context);
	};
	return {
		adapter,
		calls,
		emit,
		events,
		snapshots,
		contexts,
		async settled(): Promise<void> {
			for (let tick = 0; tick < 8; tick += 1)
				await new Promise((resolve) => setImmediate(resolve));
		},
		stop() {
			adapter.stop();
			rmSync(root, { recursive: true, force: true });
		},
	};
}

test("a subject is registered before anything is published about it", async () => {
	const h = harness();
	try {
		h.emit("session_start", {}, { isIdle: () => true });
		await h.settled();
		assert.deepEqual(h.snapshots(), [{ activity: "idle" }]);
		// The immutable registration carries the subject and nothing else: no Pi
		// session, no pane, no task text, and no claim this platform cannot make.
		assert.deepEqual(h.calls[0], {
			method: "agent.register",
			params: { source: "herdsman-pi", incarnation: INCARNATION },
		});
		assert.equal(h.calls.findIndex((call) => call.method === "agent.acquire"), 1);
	} finally {
		h.stop();
	}
});

test("unavailable Radar storage cannot disable execution lifecycle setup", async () => {
	const root = mkdtempSync(join(tmpdir(), "radar-execution-broken-store-"));
	try {
		const h = harness({ root });
		chmodSync(root, 0o755);
		h.emit("session_start", {}, { isIdle: () => true });
		await h.settled();
		assert.deepEqual(h.snapshots(), [{ activity: "idle" }]);
		h.stop();
	} finally {
		rmSync(root, { recursive: true, force: true });
	}
});

test("the session's own events become the published execution facts", async () => {
	const h = harness();
	try {
		h.emit("agent_start");
		h.emit("message_end", { message: { role: "assistant", stopReason: "error" } });
		assert.deepEqual(h.snapshots(), []);

		h.emit("session_start", {}, { isIdle: () => true });
		await h.settled();
		h.emit("agent_start");
		await h.settled();
		h.emit("message_end", { message: { role: "assistant", stopReason: "toolUse" } });
		h.emit("message_end", {
			message: { role: "user", stopReason: "stop" },
		});
		h.emit("message_end", {
			message: {
				role: "assistant",
				stopReason: "error",
				errorMessage: "provider failed at /private/SENTINEL with token=SECRET",
			},
		});
		await h.settled();
		h.emit("message_end", { message: { role: "assistant", stopReason: "stop" } });
		await h.settled();
		h.emit("agent_settled", { aborted: true }, { isIdle: () => true });
		await h.settled();

		assert.deepEqual(h.snapshots(), [
			{ activity: "idle" },
			{ activity: "working" },
			{ activity: "working", last_outcome: { result: "error" } },
			{ activity: "working", last_outcome: { result: "finished" } },
			{ activity: "idle", last_outcome: { result: "aborted" } },
		]);
	} finally {
		h.stop();
	}
});

test("a burst publishes one newest complete snapshot, not a queue of stale ones", async () => {
	const h = harness();
	try {
		h.emit("session_start", {}, { isIdle: () => true });
		await h.settled();
		h.emit("agent_start");
		h.emit("message_end", {
			message: { role: "assistant", stopReason: "error", errorMessage: "late" },
		});
		await h.settled();
		assert.deepEqual(h.snapshots(), [
			{ activity: "idle" },
			{ activity: "working", last_outcome: { result: "error" } },
		]);
	} finally {
		h.stop();
	}
});

test("a blocking label is bounded and cannot blank the record", async () => {
	const h = harness();
	try {
		h.emit("session_start", {}, { isIdle: () => true });
		await h.settled();
		h.events.emit("rpiv:ask-user:blocked", {
			active: true,
			label: "x".repeat(4000),
		});
		await h.settled();
		h.events.emit("rpiv:ask-user:blocked", { active: true, label: "   " });
		await h.settled();
		const reasons = h.snapshots().map((snapshot) => snapshot.waiting_reason);
		assert.deepEqual(reasons.map((reason) => reason?.length), [
			undefined,
			256,
			"questionnaire".length,
		]);
		assert.equal(reasons[2], "questionnaire");
	} finally {
		h.stop();
	}
});

test("an open permission prompt blocks until its own decision, and no other decision clears it", async () => {
	const h = harness();
	try {
		h.emit("session_start", {}, { isIdle: () => true });
		await h.settled();
		h.events.emit("permissions:ui_prompt", { requestId: "req-a" });
		h.events.emit("permissions:ui_prompt", { requestId: "req-b" });
		await h.settled();
		assert.equal(h.adapter.blocked(), true);
		h.events.emit("permissions:decision", { requestId: "req-unrelated" });
		h.events.emit("permissions:decision", { requestId: "req-a" });
		await h.settled();
		assert.equal(h.adapter.blocked(), true);
		h.events.emit("permissions:decision", { requestId: "req-b" });
		await h.settled();
		assert.equal(h.adapter.blocked(), false);
		assert.deepEqual(h.snapshots(), [
			{ activity: "idle" },
			{ activity: "blocked", waiting_reason: "permission" },
			{ activity: "idle" },
		]);
	} finally {
		h.stop();
	}
});

test("an open permission prompt does not survive its session", async () => {
	const h = harness();
	try {
		h.emit("session_start", {}, { isIdle: () => true });
		await h.settled();
		h.events.emit("permissions:ui_prompt", { requestId: "req-a" });
		h.emit("session_shutdown", {}, {});
		assert.equal(h.adapter.blocked(), false);
		h.emit("session_start", {}, { isIdle: () => true });
		await h.settled();
		h.events.emit("permissions:decision", { requestId: "req-a" });
		await h.settled();
		assert.equal(h.adapter.blocked(), false);
	} finally {
		h.stop();
	}
});

test("a questionnaire and a permission prompt are independent waits", async () => {
	const h = harness();
	try {
		h.emit("session_start", {}, { isIdle: () => true });
		await h.settled();
		h.events.emit("rpiv:ask-user:blocked", { active: true });
		h.events.emit("permissions:ui_prompt", { requestId: "req-a" });
		await h.settled();
		h.events.emit("rpiv:ask-user:blocked", { active: false });
		await h.settled();
		assert.equal(h.adapter.blocked(), true);
		h.events.emit("permissions:decision", { requestId: "req-a" });
		await h.settled();
		assert.equal(h.adapter.blocked(), false);
		assert.deepEqual(h.snapshots(), [
			{ activity: "idle" },
			{ activity: "blocked", waiting_reason: "questionnaire" },
			{ activity: "blocked", waiting_reason: "permission" },
			{ activity: "idle" },
		]);
	} finally {
		h.stop();
	}
});

test("a managed child registers its binding and hands the subject to its owner", async () => {
	const h = harness({ managed: true });
	try {
		assert.equal(h.adapter.binding(), undefined);
		h.emit("session_start", {}, { isIdle: () => true });
		await h.settled();
		assert.deepEqual(h.calls[0], {
			method: "agent.register",
			params: {
				source: "herdsman-pi",
				incarnation: INCARNATION,
				owner: "owner-1",
				run: "run-1",
				label: "worker",
			},
		});
		assert.deepEqual(h.adapter.binding(), {
			version: 1,
			agent_id: "00000000-0000-4000-8000-000000000001",
			incarnation: INCARNATION,
			run: "run-1",
			owner: "owner-1",
			label: "worker",
			observed_at: new Date(frozenScheduler.now()).toISOString(),
		});
		assert.equal(h.adapter.binding()?.incarnation, INCARNATION);
	} finally {
		h.stop();
	}
});

test("session facts are one complete snapshot, republished only when they change", async () => {
	const h = harness();
	const session = {
		isIdle: () => true,
		model: { id: "claude-opus-example", provider: "anthropic" },
		thinkingLevel: "high",
		sessionManager: {
			getSessionId: () => "11111111-2222-4333-8444-555555555555",
			getSessionName: () => " fixture review ",
		},
	};
	const factsPublishes = () =>
		h.calls.filter(
			(call) =>
				call.method === "agent.publish" && call.params.channel === "facts",
		);
	try {
		h.emit("session_start", {}, session);
		await h.settled();
		assert.deepEqual(factsPublishes().at(-1)!.params.snapshot, {
			model: "claude-opus-example",
			provider: "anthropic",
			thinking: "high",
			session_name: "fixture review",
		});

		const published = factsPublishes().length;
		h.emit("turn_end", {}, session);
		await h.settled();
		assert.equal(factsPublishes().length, published);

		h.emit("thinking_level_select", {}, { ...session, thinkingLevel: "low" });
		await h.settled();
		assert.equal(factsPublishes().at(-1)!.params.snapshot.thinking, "low");
	} finally {
		h.stop();
	}
});

test("the current session is published as a mutable fact of the same subject", async () => {
	const h = harness();
	const session = (id: string, idle: boolean) => ({
		isIdle: () => idle,
		sessionManager: { getSessionId: () => id },
	});
	const SESSION_A = "11111111-2222-4333-8444-555555555555";
	const SESSION_B = "66666666-7777-4888-8999-aaaaaaaaaaaa";
	try {
		h.emit("session_start", {}, session(SESSION_A, true));
		await h.settled();
		const [first] = h.contexts();
		// A first publish binds the writer and carries the session alone.
		assert.equal("writer_handle" in first, false);
		assert.deepEqual(first.context, { session: SESSION_A });
		assert.equal(first.sequence, 1);
		assert.deepEqual(first.publisher, {
			source: "herdsman-pi",
			incarnation: INCARNATION,
		});
		assert.equal(
			first.agent_id,
			h.calls.find((call) => call.method === "agent.publish")!.params.agent_id,
		);

		// A replaced session is a newer sequence under the same subject, never a
		// second registration.
		h.emit("session_shutdown");
		h.emit("session_start", {}, session(SESSION_B, false));
		await h.settled();
		assert.equal(
			h.calls.filter((call) => call.method === "agent.register").length,
			1,
		);
		const [, switched] = h.contexts();
		assert.match(switched.writer_handle, /^[0-9a-f]{8}-[0-9a-f]{4}-/);
		assert.equal(switched.sequence, 2);
		assert.deepEqual(switched.context, { session: SESSION_B });
		assert.equal(switched.agent_id, first.agent_id);
	} finally {
		h.stop();
	}
});

test("a session Pi cannot name is left unreported", async () => {
	const h = harness();
	try {
		// A session file path is not a session UUID, and an explicit null would
		// claim this process has no session at all: neither is published.
		h.emit(
			"session_start",
			{},
			{ isIdle: () => true, sessionManager: { getSessionId: () => "/home/dev/.pi/sessions/one.jsonl" } },
		);
		await h.settled();
		assert.deepEqual(h.contexts(), []);
		assert.equal(h.snapshots().length, 1);
	} finally {
		h.stop();
	}
});

test("a replaced session is never renewed as the one it replaced", async () => {
	// A clock that moves without ever firing: the adapter is still driven only by
	// Pi's events, but the heartbeat that renewal is measured against has lapsed.
	let now = 1_700_000_000_000;
	const h = harness({
		scheduler: {
			setTimeout: () => ({ unref: () => {} }),
			clearTimeout: () => {},
			setInterval: () => ({ unref: () => {} }),
			clearInterval: () => {},
			now: () => now,
		},
	});
	const session = (id: string) => ({
		isIdle: () => true,
		sessionManager: { getSessionId: () => id },
	});
	const FIRST = "11111111-2222-4333-8444-555555555555";
	const SECOND = "66666666-7777-4888-8999-aaaaaaaaaaaa";
	try {
		h.emit("session_start", {}, session(FIRST));
		await h.settled();
		h.emit("session_shutdown");
		now += 60_000;
		h.emit("session_start", {}, session(SECOND));
		await h.settled();
		const contexts = h.contexts();
		assert.equal(contexts.length, 2);
		assert.deepEqual(contexts[1].context, { session: SECOND });
	} finally {
		h.stop();
	}
});

test("a session can be replaced or ended without ending this process", async () => {
	const h = harness();
	try {
		h.emit("session_start", {}, { isIdle: () => true });
		await h.settled();
		assert.deepEqual(h.snapshots(), [{ activity: "idle" }]);

		// The session ends: this session's facts stop advancing with its timers.
		h.emit("session_shutdown");
		h.emit("agent_start");
		await h.settled();
		assert.equal(h.snapshots().length, 1);

		// A new session in the same process re-arms the same process publication
		// rather than becoming a second subject of it.
		h.emit("session_start", {}, { isIdle: () => false });
		await h.settled();
		assert.deepEqual(h.snapshots().slice(1), [{ activity: "working" }]);
		assert.equal(
			h.calls.filter((call) => call.method === "agent.register").length,
			1,
		);

		// Explicit disposal silences the adapter for good.
		h.adapter.stop();
		h.emit("session_start", {}, { isIdle: () => true });
		await h.settled();
		assert.equal(h.snapshots().length, 2);
	} finally {
		h.stop();
	}
});

test("a reloaded adapter reuses the process's one publisher", async () => {
	const slot = createRadarProcessSlot();
	const root = mkdtempSync(join(tmpdir(), "radar-execution-test-"));
	const first = harness({ slot, root });
	try {
		first.emit("session_start", {}, { isIdle: () => true });
		await first.settled();
		assert.equal(first.snapshots().length, 1);

		// Pi reloads the extension in the same process: same slot, same data root.
		first.adapter.stop();
		const second = harness({ slot, root });
		try {
			second.emit("session_start", {}, { isIdle: () => false });
			await second.settled();
			assert.equal(second.adapter.publication, first.adapter.publication);
			// The reused publisher keeps its transport, its subject and its writer
			// binding: the reload neither re-registers nor re-acquires the channel,
			// and the sequence continues where the process left it.
			assert.deepEqual(second.calls, []);
			assert.deepEqual(
				first.calls.map((call) => [call.method, call.params.sequence]),
				[
					["agent.register", undefined],
					["agent.acquire", undefined],
					["agent.publish", 1],
					["agent.publish", 2],
				],
			);
			assert.deepEqual(first.snapshots(), [
				{ activity: "idle" },
				{ activity: "working" },
			]);
		} finally {
			second.stop();
		}
	} finally {
		first.stop();
	}
});
