import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
	mkdtempSync,
	mkdirSync,
	readdirSync,
	rmSync,
	statSync,
	chmodSync,
	symlinkSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import type {
	RadarErrorCode,
	RadarResult,
	RadarScheduler,
	RadarTimer,
} from "./radar-client.ts";
import {
	RADAR_HEARTBEAT_MS,
	RADAR_LEASE_MS,
	createPublicationStore,
	createRadarProcessSlot,
	createRadarPublication,
	parseProcStat,
	radarBindingKey,
	radarProcessIncarnation,
	type PublicationStore,
	type RadarPublication,
	type RadarPublicationClient,
} from "./radar-publication.ts";

const ok = <T>(value: T): RadarResult<T> => ({ ok: true, value });
const refused = (
	code: RadarErrorCode,
	message: string,
): RadarResult<never> => ({ ok: false, code, message });

type Call = { method: string; params: any };
/** Returning `undefined` falls back to the fake daemon's own answer. */
type Script = Record<string, (params: any, nth: number) => RadarResult<any> | undefined>;

function fakeDaemon(script: Script = {}) {
	const calls: Call[] = [];
	const agentIds: string[] = [];
	const handles: string[] = [];
	let minted = 0;
	const mint = (): string =>
		`00000000-0000-4000-8000-${String(++minted).padStart(12, "0")}`;
	const daemon = {
		calls,
		agentIds,
		handles,
		params(method: string, nth = 0): any {
			return daemon.all(method)[nth];
		},
		all(method: string): any[] {
			return calls.filter((call) => call.method === method).map((call) => call.params);
		},
		count(method: string): number {
			return daemon.all(method).length;
		},
		client: undefined as unknown as RadarPublicationClient,
	};
	daemon.client = {
		ping: async () => ok({
			protocol: 2,
			revision: 1,
			backend: "tmux",
			instance: null,
			contracts: ["registry.v1", "registry.facts.v1"],
			tasks: { available: false, error: null },
		}),
		register: async (params) => answer("agent.register", params, () => {
			const agentId = mint();
			agentIds.push(agentId);
			return ok({ agent_id: agentId });
		}),
		acquire: async (params) => answer("agent.acquire", params, () => {
			const handle = mint();
			handles.push(handle);
			return ok({
				handle,
				source: params.publisher.source,
				incarnation: params.publisher.incarnation,
				generation: 1,
				sequence: 0,
			});
		}),
		publish: async (params) =>
			answer("agent.publish", params, () => ok({ sequence: params.sequence })),
		context: async (params) =>
			answer("agent.context", params, () => {
				const handle = params.writer_handle ?? mint();
				if (params.writer_handle === undefined) handles.push(handle);
				return ok({
					writer: {
						handle,
						source: params.publisher.source,
						incarnation: params.publisher.incarnation,
						generation: 1,
						sequence: params.sequence,
					},
				});
			}),
		retire: async (params) => answer("agent.retire", params, () => ok(undefined)),
	};
	function answer(
		method: string,
		params: unknown,
		fallback: () => RadarResult<any>,
	): RadarResult<any> {
		const previous = calls.filter((call) => call.method === method).length;
		calls.push({ method, params });
		return script[method]?.(params, previous) ?? fallback();
	}
	return daemon;
}

function fakeScheduler(start = 1_700_000_000_000) {
	let time = start;
	let nextId = 0;
	const timers = new Map<
		number,
		{ at: number; every?: number; handler: () => void }
	>();
	const idOf = (timer: RadarTimer): number => (timer as { id?: number }).id ?? 0;
	const arm = (handler: () => void, ms: number, every?: number): RadarTimer => {
		const id = ++nextId;
		timers.set(id, { at: time + ms, handler, ...(every ? { every } : {}) });
		return { id, unref: () => {} } as RadarTimer;
	};
	const scheduler: RadarScheduler = {
		setTimeout: (handler, ms) => arm(handler, ms),
		clearTimeout: (timer) => {
			timers.delete(idOf(timer));
		},
		setInterval: (handler, ms) => arm(handler, ms, ms),
		clearInterval: (timer) => {
			timers.delete(idOf(timer));
		},
		now: () => time,
	};
	return {
		scheduler,
		async advance(ms: number): Promise<void> {
			time += ms;
			for (const [id, timer] of [...timers]) {
				if (timer.at > time) continue;
				if (timer.every === undefined) timers.delete(id);
				else timer.at += timer.every;
				timer.handler();
			}
			await settle();
		},
	};
}

/** Let every queued drain finish; publishes are asynchronous by contract. */
async function settle(): Promise<void> {
	for (let tick = 0; tick < 8; tick += 1)
		await new Promise((resolve) => setImmediate(resolve));
}

function makeRoot() {
	const root = mkdtempSync(join(tmpdir(), "radar-publication-test-"));
	return {
		root,
		store: createPublicationStore(root),
		cleanup: () => rmSync(root, { recursive: true, force: true }),
	};
}

const HEARTBEAT_MS = 100;

/** A publication with test-owned identity, clock and store. */
function publicationFor(
	daemon: ReturnType<typeof fakeDaemon>,
	overrides: Record<string, unknown> = {},
): RadarPublication {
	return createRadarPublication({
		client: daemon.client,
		registration: { source: "herdsman-pi" },
		heartbeatMs: HEARTBEAT_MS,
		...overrides,
	} as Parameters<typeof createRadarPublication>[0]);
}

test("radar records are private, atomic and bounded", () => {
	const { root, store, cleanup } = makeRoot();
	try {
		assert.throws(() => store.path("../escape.json"), /invalid radar record name/);
		store.write("subjects/one.json", { version: 1 });
		assert.equal(statSync(store.path("subjects/one.json")).mode & 0o777, 0o600);
		assert.equal(statSync(join(root, "subjects")).mode & 0o777, 0o700);
		assert.deepEqual(store.read("subjects/one.json"), { version: 1 });

		const path = store.path("subjects/one.json");
		rmSync(path);
		symlinkSync(join(root, "elsewhere.json"), path);
		assert.throws(() => store.read("subjects/one.json"), /unsafe Radar record|ELOOP/);

		rmSync(path);
	} finally {
		cleanup();
	}
});

test("private records reject unsafe roots, files, and oversized writes", () => {
	const parent = mkdtempSync(join(tmpdir(), "radar-store-parent-"));
	const root = join(parent, "store");
	try {
		const store = createPublicationStore(root);
		assert.throws(
			() => store.write("subjects/large.json", { data: "x".repeat(70 * 1024) }),
			/ exceeds /,
		);
		store.write("subjects/record.json", { safe: true });
		const file = store.path("subjects/record.json");
		chmodSync(file, 0o644);
		assert.throws(() => store.read("subjects/record.json"), /unsafe Radar record/);
		chmodSync(file, 0o600);

		const linkParent = join(parent, "link-parent");
		mkdirSync(linkParent, { mode: 0o700 });
		const linkRoot = join(parent, "linked");
		symlinkSync(linkParent, linkRoot);
		assert.throws(() => createPublicationStore(linkRoot), /unsafe Radar storage directory/);

		const loose = join(parent, "loose");
		mkdirSync(loose, { mode: 0o755 });
		assert.throws(() => createPublicationStore(loose), /mode 0700/);
	} finally {
		rmSync(parent, { recursive: true, force: true });
	}
});

test("private record reads reject a FIFO quickly and reject symlinked namespace parents", () => {
	const { root, store, cleanup } = makeRoot();
	try {
		mkdirSync(join(root, "bindings"), { mode: 0o700 });
		const fifo = store.path("bindings/non-regular.json");
		execFileSync("mkfifo", [fifo]);
		chmodSync(fifo, 0o600);
		const started = Date.now();
		assert.throws(() => store.read("bindings/non-regular.json"), /unsafe Radar record/);
		assert.ok(Date.now() - started < 1000, "FIFO read must not block the event loop");

		rmSync(join(root, "bindings"), { recursive: true, force: true });
		const outside = join(root, "outside");
		mkdirSync(outside, { mode: 0o700 });
		writeFileSync(join(outside, "binding.json"), JSON.stringify({ safe: true }), { mode: 0o600 });
		symlinkSync(outside, join(root, "bindings"));
		assert.throws(() => store.read("bindings/binding.json"), /unsafe Radar storage directory/);
	} finally {
		cleanup();
	}
});

test("owner-facing binding and lazy assignment failures are contained after setup", async () => {
	const { root, store, cleanup } = makeRoot();
	try {
		const daemon = fakeDaemon();
		let failReads = false;
		let failWrites = false;
		const unstableStore: PublicationStore = {
			path: (name) => store.path(name),
			read: (name) => {
				if (failReads) throw new Error("corrupt owner binding");
				return store.read(name);
			},
			write: (name, value) => {
				if (failWrites && name.startsWith("channels/"))
					throw new Error("assignment storage unavailable");
				store.write(name, value);
			},
			remove: (name) => store.remove(name),
			};
		const publication = publicationFor(daemon, { root, store: unstableStore });
		publication.execution.update({ activity: "working" });
		await settle();
		const agentId = daemon.agentIds[0];

		failReads = true;
		assert.doesNotThrow(() => publication.binding("run-key"));
		assert.equal(publication.binding("run-key"), undefined);
		assert.match(publication.diagnostic() ?? "", /corrupt owner binding/);

		failReads = false;
		failWrites = true;
		let writer: ReturnType<typeof publication.assignment> | undefined;
		assert.doesNotThrow(() => { writer = publication.assignment(agentId, "owner"); });
		assert.ok(writer);
		assert.doesNotThrow(() => writer!.update({ activity: "waiting" }));
		await settle();
		assert.match(writer!.diagnostic() ?? "", /storage failure/);
		assert.doesNotThrow(() => publication.retireAssignment(agentId));
	} finally {
		cleanup();
	}
});

test("publisher storage failures are contained at initialization, drain, and retirement", async () => {
	const { root, store, cleanup } = makeRoot();
	try {
		const daemon = fakeDaemon();
		const unsupportedRoot = join(root, "unsupported-store");
		mkdirSync(unsupportedRoot, { mode: 0o700 });
		const brokenStorePublication = publicationFor(daemon, { root: unsupportedRoot, store: {
			path: (name: string) => store.path(name),
			read: (name: string) => store.read(name),
			remove: (name: string) => store.remove(name),
			write: () => { throw new Error("injected initialization failure"); },
		} });
		// The registration record is built at first use - it carries a location that
		// may still be arriving - so an unwritable store is reported when that
		// record is first written, and never thrown out of a lifecycle event.
		assert.equal(await brokenStorePublication.subject(), undefined);
		assert.equal(brokenStorePublication.diagnostic()?.includes("unavailable"), true);

		let failWrites = false;
		const selectiveStore: PublicationStore = {
			path: (name) => store.path(name),
			read: (name) => store.read(name),
			remove: (name) => store.remove(name),
			write: (name, value) => {
				if (failWrites) throw new Error("injected disk failure");
				store.write(name, value);
			},
		};
		const { scheduler } = fakeScheduler();
		const publication = publicationFor(daemon, { root, store: selectiveStore, scheduler });
		publication.execution.update({ activity: "working" });
		await settle();
		assert.equal(daemon.count("agent.publish"), 1);
		failWrites = true;
		publication.retireAssignment(daemon.agentIds[0]);
		await settle();
		assert.doesNotThrow(() => publication.stop());
	} finally {
		cleanup();
	}
});

test("an outcome without detail survives pending and accepted record reload", async () => {
	const { root, store, cleanup } = makeRoot();
	try {
		const daemon = fakeDaemon({ "agent.publish": () => refused("timeout", "lost reply") });
		const { scheduler } = fakeScheduler();
		const slot = createRadarProcessSlot();
		const first = publicationFor(daemon, { root, store, scheduler, slot, incarnation: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa" });
		first.execution.update({ activity: "idle", last_outcome: { result: "aborted" } });
		await settle();
		first.stop();
		const restarted = publicationFor(daemon, { root, store, scheduler, slot: createRadarProcessSlot(), incarnation: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa" });
		restarted.execution.update({ activity: "idle", last_outcome: { result: "aborted" } });
		await settle();
		assert.equal(daemon.count("agent.publish"), 2);
		assert.deepEqual(daemon.params("agent.publish", 1).snapshot.last_outcome, { result: "aborted" });
	} finally {
		cleanup();
	}
});

test("a reload reuses the process incarnation and its one publisher", async () => {
	const { root, store, cleanup } = makeRoot();
	try {
		const daemon = fakeDaemon();
		const slot = createRadarProcessSlot();
		const { scheduler } = fakeScheduler();
		const common = { dataRoot: root, store, slot, scheduler };
		const first = publicationFor(daemon, common);
		first.execution.update({ activity: "working" });
		await settle();
		assert.equal(daemon.count("agent.register"), 1);
		assert.equal(daemon.count("agent.acquire"), 1);
		assert.equal(daemon.params("agent.publish").sequence, 1);

		const incarnation = radarProcessIncarnation(slot);
		first.stop();

		const reloaded = publicationFor(daemon, common);
		assert.equal(reloaded, first);
		assert.equal(radarProcessIncarnation(slot), incarnation);
		reloaded.execution.update({ activity: "idle" });
		await settle();
		// The same writer binding and sequence continue: no second subject and no
		// competing publisher of the same channel.
		assert.equal(daemon.count("agent.register"), 1);
		assert.equal(daemon.count("agent.acquire"), 1);
		assert.equal(daemon.params("agent.publish", 1).sequence, 2);
		assert.equal(daemon.params("agent.publish", 1).snapshot.activity, "idle");
	} finally {
		cleanup();
	}
});

test("a fresh process is a distinct subject of the same data root", async () => {
	const { root, store, cleanup } = makeRoot();
	try {
		const daemon = fakeDaemon();
		const { scheduler } = fakeScheduler();
		const common = { dataRoot: root, store, scheduler };
		// Registration carries no session UUID at all, so two processes that resume
		// one Pi session still register as two distinct subjects.
		for (const slot of [createRadarProcessSlot(), createRadarProcessSlot()]) {
			const publication = publicationFor(daemon, { ...common, slot });
			publication.execution.update({ activity: "working" });
			await settle();
		}
		const registrations = daemon.all("agent.register");
		assert.equal(registrations.length, 2);
		assert.notEqual(
			registrations[0].incarnation,
			registrations[1].incarnation,
		);
		assert.notEqual(daemon.agentIds[0], daemon.agentIds[1]);
		assert.equal(readdirSync(join(root, "subjects")).length, 2);
	} finally {
		cleanup();
	}
});

test("the persisted registration outlives a later process claim", async () => {
	const { root, store, cleanup } = makeRoot();
	try {
		let loseChannel = 0;
		const daemon = fakeDaemon({
			"agent.publish": () => {
				if (loseChannel === 0) return undefined;
				loseChannel -= 1;
				return refused("not_found", "unknown channel");
			},
		});
		const { scheduler, advance } = fakeScheduler();
		const incarnation = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
		const claim = (pid: number) => ({
			boot_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
			pid,
			start_ticks: pid * 10,
		});
		const common = { dataRoot: root, store, scheduler, incarnation };
		const first = publicationFor(daemon, {
			...common,
			slot: createRadarProcessSlot(),
			birth: claim(11),
		});
		first.execution.update({ activity: "working" });
		await settle();
		assert.deepEqual(daemon.params("agent.register").process, claim(11));
		// The registration is durable before it is ever sent.
		const [record] = readdirSync(join(root, "subjects"));
		assert.deepEqual(
			(store.read(`subjects/${record}`) as { registration: { process: unknown } })
				.registration.process,
			claim(11),
		);
		first.stop();

		// The daemon lost the channel and this process reads a *different* claim
		// now: the incarnation still registers as the one it was persisted with.
		loseChannel = 1;
		const restarted = publicationFor(daemon, {
			...common,
			slot: createRadarProcessSlot(),
			birth: claim(22),
		});
		restarted.execution.update({ activity: "idle" });
		await settle();
		await advance(HEARTBEAT_MS);
		assert.equal(daemon.count("agent.register"), 2);
		assert.deepEqual(daemon.params("agent.register", 1).process, claim(11));
		// The recovered subject starts its own channel sequence.
		assert.equal(daemon.count("agent.publish"), 3);
		assert.equal(daemon.params("agent.publish", 2).sequence, 1);
		assert.equal(daemon.params("agent.publish", 2).snapshot.activity, "idle");
	} finally {
		cleanup();
	}
});

test("a refused channel stops with one diagnostic and no takeover", async () => {
	const { root, store, cleanup } = makeRoot();
	try {
		const daemon = fakeDaemon({
			"agent.acquire": (_params, nth) =>
				nth === 0 ? refused("refused", "held by another writer") : undefined,
		});
		const { scheduler, advance } = fakeScheduler();
		const publication = publicationFor(daemon, {
			dataRoot: root,
			store,
			scheduler,
			slot: createRadarProcessSlot(),
		});
		publication.execution.update({ activity: "working" });
		await settle();
		assert.equal(
			publication.execution.diagnostic(),
			"channel execution stopped: held by another writer",
		);
		const diagnostic = publication.execution.diagnostic();
		await advance(HEARTBEAT_MS * 4);
		assert.equal(daemon.count("agent.acquire"), 1);
		assert.equal(daemon.count("agent.publish"), 0);
		// Replacement is explicit: a fenced publisher never asks to take over.
		assert.ok(daemon.all("agent.acquire").every((params) => params.replace === undefined));
		assert.equal(publication.execution.diagnostic(), diagnostic);
	} finally {
		cleanup();
	}
});

test("a lost subject is registered again from the identical content", async () => {
	const { root, store, cleanup } = makeRoot();
	try {
		const daemon = fakeDaemon({
			"agent.acquire": (_params, nth) =>
				nth === 0 ? refused("not_found", "unknown agent") : undefined,
		});
		const { scheduler, advance } = fakeScheduler();
		const publication = publicationFor(daemon, {
			dataRoot: root,
			store,
			scheduler,
			slot: createRadarProcessSlot(),
		});
		publication.execution.update({ activity: "working" });
		await settle();
		// A lost subject is not retried in a loop; the heartbeat holds the cadence.
		assert.equal(daemon.count("agent.register"), 1);
		await advance(HEARTBEAT_MS);
		assert.equal(daemon.count("agent.register"), 2);
		assert.deepEqual(
			daemon.params("agent.register", 0),
			daemon.params("agent.register", 1),
		);
		// A new subject cannot carry the old subject's channel state.
		assert.equal(daemon.params("agent.publish").sequence, 1);
		assert.equal(daemon.params("agent.publish").snapshot.activity, "working");
	} finally {
		cleanup();
	}
});

test("a lost reply replays the exact request and does not advance the lease", async () => {
	const { root, store, cleanup } = makeRoot();
	try {
		const daemon = fakeDaemon({
			"agent.publish": (_params, nth) =>
				nth === 0 ? refused("timeout", "lost ack") : undefined,
		});
		const { scheduler, advance } = fakeScheduler();
		const publication = publicationFor(daemon, {
			dataRoot: root,
			store,
			scheduler,
			slot: createRadarProcessSlot(),
		});
		publication.execution.update({ activity: "working" });
		await settle();
		const unresolved = daemon.params("agent.publish", 0);

		await advance(HEARTBEAT_MS);
		assert.deepEqual(daemon.params("agent.publish", 1), unresolved);

		publication.execution.update({ activity: "idle" });
		await settle();
		// The replayed request kept sequence 1, so the next publish is 2 rather
		// than a sequence the daemon never accepted.
		assert.equal(daemon.params("agent.publish", 2).sequence, 2);
		assert.equal(daemon.params("agent.publish", 2).snapshot.activity, "idle");
	} finally {
		cleanup();
	}
});

test("an unchanged channel is renewed with a newer sequence", async () => {
	const { root, store, cleanup } = makeRoot();
	try {
		const daemon = fakeDaemon();
		const { scheduler, advance } = fakeScheduler();
		const publication = publicationFor(daemon, {
			dataRoot: root,
			store,
			scheduler,
			slot: createRadarProcessSlot(),
		});
		publication.execution.update({ activity: "working" });
		await settle();
		assert.equal(daemon.count("agent.publish"), 1);

		await advance(HEARTBEAT_MS);
		// Freshness comes from a newer sequence and a newer observation, never from
		// the publisher's own opinion that the facts are still true.
		assert.equal(daemon.params("agent.publish", 1).sequence, 2);
		assert.deepEqual(daemon.params("agent.publish", 1).snapshot, {
			activity: "working",
		});
		assert.notEqual(
			daemon.params("agent.publish", 1).observed_at,
			daemon.params("agent.publish", 0).observed_at,
		);
	} finally {
		cleanup();
	}
});

test("an absent daemon is silent rather than an incident", async () => {
	const { root, store, cleanup } = makeRoot();
	try {
		const daemon = fakeDaemon({
			"agent.register": () =>
				refused("absent", "/run/user/1000/agent-radar/control.sock is unavailable"),
		});
		const { scheduler, advance } = fakeScheduler();
		const publication = publicationFor(daemon, {
			dataRoot: root,
			store,
			scheduler,
			slot: createRadarProcessSlot(),
		});
		publication.execution.update({ activity: "working" });
		await settle();
		await advance(HEARTBEAT_MS * 2);
		// Nothing is dialled past the registration attempt, nothing is published,
		// and the process keeps its own durable registration meanwhile.
		assert.equal(daemon.count("agent.acquire"), 0);
		assert.equal(daemon.count("agent.publish"), 0);
		assert.equal(publication.diagnostic(), undefined);
		assert.equal(readdirSync(join(root, "subjects")).length, 1);
	} finally {
		cleanup();
	}
});

test("the owner publishes about a child's subject and retires only its own writer", async () => {
	const { root, store, cleanup } = makeRoot();
	try {
		const daemon = fakeDaemon();
		const { scheduler } = fakeScheduler();
		const key = radarBindingKey("run-1", "owner-1", "worker");
		const child = publicationFor(daemon, {
			dataRoot: root,
			store,
			scheduler,
			slot: createRadarProcessSlot(),
			incarnation: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
			registration: {
				source: "herdsman-pi",
				owner: "owner-1",
				run: "run-1",
				label: "worker",
			},
			binding: { key, run: "run-1", owner: "owner-1", label: "worker" },
		});
		child.execution.update({ activity: "working" });
		await settle();
		// Only the child registers, and only the child writes its own sidecar.
		assert.equal(daemon.count("agent.register"), 1);

		const owner = publicationFor(daemon, {
			dataRoot: root,
			store,
			scheduler,
			slot: createRadarProcessSlot(),
			incarnation: "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
		});
		const bound = owner.binding(key);
		assert.equal(bound?.agent_id, daemon.agentIds[0]);
		assert.equal(bound?.label, "worker");

		const writer = owner.assignment(bound!.agent_id, "owner-1");
		writer.update({ activity: "waiting", waiting_reason: "background-work" });
		await settle();
		const published = daemon.params("agent.publish", 1);
		assert.equal(published.agent_id, daemon.agentIds[0]);
		assert.equal(published.channel, "assignment");
		// The owner acquires it in its own name, reporting whose assignment it is.
		assert.deepEqual(daemon.params("agent.acquire", 1).publisher, {
			source: "herdsman-owner",
			incarnation: "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
			reporting_owner: "owner-1",
		});

		// An agent this process never published about is not retired.
		owner.retireAssignment("ffffffff-ffff-4fff-8fff-ffffffffffff");
		await settle();
		assert.equal(daemon.count("agent.retire"), 0);

		owner.retireAssignment(bound!.agent_id);
		await settle();
		assert.equal(daemon.count("agent.retire"), 1);
		assert.equal(daemon.params("agent.retire").agent_id, daemon.agentIds[0]);
		assert.equal(daemon.params("agent.retire").writer_handle, daemon.handles[1]);
	} finally {
		cleanup();
	}
});

const SESSION_A = "11111111-2222-4333-8444-555555555555";
const SESSION_B = "66666666-7777-4888-8999-aaaaaaaaaaaa";

test("a current session is published once and renewed from the same cadence", async () => {
	const { root, store, cleanup } = makeRoot();
	try {
		const daemon = fakeDaemon();
		const { scheduler, advance } = fakeScheduler();
		const publication = publicationFor(daemon, {
			dataRoot: root,
			store,
			scheduler,
			slot: createRadarProcessSlot(),
		});
		publication.context.update(SESSION_A);
		await settle();
		const first = daemon.params("agent.context");
		// A first publish presents no credential: the daemon binds the writer this
		// publisher already is, and the payload is the session alone.
		assert.equal("writer_handle" in first, false);
		assert.deepEqual(first.context, { session: SESSION_A });
		assert.equal(first.sequence, 1);
		assert.equal(first.lease_ms, RADAR_LEASE_MS);

		await advance(HEARTBEAT_MS);
		// Renewal is a newer sequence under the issued binding, on the cadence that
		// already carries this process's facts: no second heartbeat, no longer lease.
		const renewed = daemon.params("agent.context", 1);
		assert.equal(renewed.writer_handle, daemon.handles[0]);
		assert.equal(renewed.sequence, 2);
		assert.deepEqual(renewed.context, { session: SESSION_A });
		assert.equal(renewed.lease_ms, RADAR_LEASE_MS);
	} finally {
		cleanup();
	}
});

test("a session switch is a newer sequence under the same subject", async () => {
	const { root, store, cleanup } = makeRoot();
	try {
		const daemon = fakeDaemon();
		const { scheduler } = fakeScheduler();
		const publication = publicationFor(daemon, {
			dataRoot: root,
			store,
			scheduler,
			slot: createRadarProcessSlot(),
		});
		publication.context.update(SESSION_A);
		await settle();
		publication.context.update(SESSION_B);
		await settle();
		// Switching or forking a session is not becoming another subject: no second
		// registration, no second writer binding and no regressed sequence.
		assert.equal(daemon.count("agent.register"), 1);
		assert.equal(daemon.count("agent.context"), 2);
		assert.equal(
			daemon.params("agent.context", 0).agent_id,
			daemon.params("agent.context", 1).agent_id,
		);
		assert.equal(daemon.params("agent.context", 1).writer_handle, daemon.handles[0]);
		assert.equal(daemon.params("agent.context", 1).sequence, 2);
		assert.deepEqual(daemon.params("agent.context", 1).context, {
			session: SESSION_B,
		});

		// An unchanged session is not re-sent between heartbeats.
		publication.context.update(SESSION_B);
		await settle();
		assert.equal(daemon.count("agent.context"), 2);

		// An explicit null is the record's own value for "no current session": it
		// is published as that fact, not as a missing one.
		publication.context.update(null);
		await settle();
		assert.equal(daemon.params("agent.context", 2).sequence, 3);
		assert.equal("session" in daemon.params("agent.context", 2).context, true);
		assert.equal(daemon.params("agent.context", 2).context.session, null);
	} finally {
		cleanup();
	}
});

test("a warned replay renews on the original lease origin, not on the replay", async () => {
	const { root, store, cleanup } = makeRoot();
	try {
		// The daemon's answer to an identical replay: the stored record, with a
		// warning that the lease was not renewed.
		const replayWarning = (params: any) =>
			ok({
				writer: {
					handle: params.writer_handle,
					source: params.publisher.source,
					incarnation: params.publisher.incarnation,
					generation: 1,
					sequence: params.sequence,
				},
				warning: "answered replay: the lease was not renewed",
			});
		const daemon = fakeDaemon({
			"agent.context": (params, nth) => {
				if (nth === 1) return refused("timeout", "lost ack");
				return nth === 2 ? replayWarning(params) : undefined;
			},
		});
		const { scheduler, advance } = fakeScheduler();
		const publication = publicationFor(daemon, {
			dataRoot: root,
			store,
			scheduler,
			slot: createRadarProcessSlot(),
		});
		publication.context.update(SESSION_A);
		await settle();

		// One heartbeat in, the renewal goes out and its reply is lost.
		await advance(HEARTBEAT_MS + HEARTBEAT_MS / 2);
		assert.equal(daemon.count("agent.context"), 2);

		// An extension reload between heartbeats replays the unresolved request, and
		// the daemon answers that identical replay with the warning.
		publication.context.reopen();
		await settle();
		assert.equal(daemon.count("agent.context"), 3);
		assert.deepEqual(
			daemon.params("agent.context", 2),
			daemon.params("agent.context", 1),
		);

		// The lease the warning did not renew still started at the original
		// acceptance, so the next heartbeat tick renews instead of waiting another
		// full heartbeat after the replay.
		await advance(HEARTBEAT_MS / 2);
		assert.equal(daemon.count("agent.context"), 4);
		const renewed = daemon.params("agent.context", 3);
		assert.equal(renewed.sequence, 3);
		assert.equal(renewed.writer_handle, daemon.handles[0]);
		assert.deepEqual(renewed.context, { session: SESSION_A });

		// An ordinary acceptance does draw the clock forward to its own time: this
		// reload is half a heartbeat after it and stages nothing.
		await advance(HEARTBEAT_MS / 2);
		publication.context.reopen();
		await settle();
		assert.equal(daemon.count("agent.context"), 4);
	} finally {
		cleanup();
	}
});

test("a context for a lost subject is bound again from a fresh sequence", async () => {
	const { root, store, cleanup } = makeRoot();
	try {
		const daemon = fakeDaemon({
			"agent.context": (_params, nth) =>
				nth === 0 ? refused("not_found", "unknown agent") : undefined,
		});
		const { scheduler, advance } = fakeScheduler();
		const publication = publicationFor(daemon, {
			dataRoot: root,
			store,
			scheduler,
			slot: createRadarProcessSlot(),
		});
		publication.context.update(SESSION_A);
		await settle();
		// A lost subject is registered again on the heartbeat, not in a hot loop.
		assert.equal(daemon.count("agent.register"), 1);
		await advance(HEARTBEAT_MS);
		assert.equal(daemon.count("agent.register"), 2);
		const rebound = daemon.params("agent.context", 1);
		assert.equal("writer_handle" in rebound, false);
		assert.equal(rebound.sequence, 1);
		assert.deepEqual(rebound.context, { session: SESSION_A });
	} finally {
		cleanup();
	}
});

test("a refused context stops with one diagnostic and no takeover", async () => {
	// A refusal and a method this daemon does not serve are both permanent for the
	// record's lifetime: neither is retried, and neither provokes a takeover.
	for (const [code, message] of [
		["refused", "context has a writer this publisher did not present"],
		["unknown_method", "unknown method agent.context"],
	] as const) {
		const { root, store, cleanup } = makeRoot();
		try {
			const daemon = fakeDaemon({
				"agent.context": (_params, nth) =>
					nth === 0 ? refused(code, message) : undefined,
			});
			const { scheduler, advance } = fakeScheduler();
			const publication = publicationFor(daemon, {
				dataRoot: root,
				store,
				scheduler,
				slot: createRadarProcessSlot(),
			});
			publication.context.update(SESSION_A);
			await settle();
			const diagnostic = `context stopped: ${message}`;
			assert.equal(publication.context.diagnostic(), diagnostic);
			assert.equal(publication.diagnostic(), diagnostic);
			await advance(HEARTBEAT_MS * 4);
			assert.equal(daemon.count("agent.context"), 1);
			// Replacement stays explicit: a fenced writer never asks to take over.
			assert.ok(
				daemon
					.all("agent.context")
					.every((params) => params.replace === undefined),
			);
			assert.equal(publication.context.diagnostic(), diagnostic);
		} finally {
			cleanup();
		}
	}
});

test("process start ticks parse around a command name with spaces", () => {
	const tail = Array.from({ length: 22 }, (_, index) => String(index + 1));
	// `state` is the field after the command name, so `starttime` is the
	// twentieth of the tail rather than the twenty-second of the line.
	tail[18] = "4242";
	assert.deepEqual(
		parseProcStat(`99 (agent worker) S ${tail.join(" ")}`),
		{ pid: 99, start_ticks: 4242 },
	);
	assert.equal(parseProcStat("garbage"), undefined);
	assert.equal(parseProcStat("0 (x) S 1 2"), undefined);
});

test("the daemon's lease outlives the heartbeat that renews it", () => {
	assert.ok(RADAR_LEASE_MS > RADAR_HEARTBEAT_MS);
});
