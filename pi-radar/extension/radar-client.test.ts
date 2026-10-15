import assert from "node:assert/strict";
import {
	chmodSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { createServer, type Server } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import {
	RADAR_MAX_LINE_BYTES,
	createRadarClient,
	watchRadarEvents,
	untrustedControlSocket,
	type RadarClient,
	type RadarClientOptions,
	type RadarSocket,
} from "./radar-client.ts";

const UUID_A = "11111111-1111-4111-8111-111111111111";
const UUID_B = "22222222-2222-4222-8222-222222222222";
const UID = process.getuid?.() ?? 0;
const FIXTURE_WRITER = "5a5a5a5a-5a5a-4a5a-8a5a-5a5a5a5a5a5a";
/** The fixture's successor writer handle, for its replacement exchange. */
const FIXTURE_SUCCESSOR = "6b6b6b6b-6b6b-4b6b-8b6b-6b6b6b6b6b6b";

/** The fixture's generated placeholders, resolved to real values. */
const FIXTURE_HANDLES: Record<string, string> = {
	"<opaque-uuid>": FIXTURE_WRITER,
	"<observed-incumbent-handle>": FIXTURE_WRITER,
	"<replacement-opaque-uuid>": FIXTURE_SUCCESSOR,
};

function materialise(value: unknown): any {
	return JSON.parse(
		JSON.stringify(value).replace(
			/<(?:observed-incumbent-handle|replacement-opaque-uuid|opaque-uuid)>/g,
			(name) => FIXTURE_HANDLES[name]!,
		),
	);
}

/**
 * Read the repository's canonical registry exchange fixture so this client test
 * exercises the same fact and channel shapes as the Rust daemon tests.
 */
function fixtureExchanges(): Array<{ request: any; response: any }> {
	const lines = readFileSync(
		new URL("../../docs/agent-registration.fixture.jsonl", import.meta.url),
		"utf8",
	)
		.split("\n")
		.filter((line) => line.trim() !== "")
		.map((line) => JSON.parse(line) as any);
	return lines
		.map((line: any, index: number) => ({ line, next: lines[index + 1] }))
		.filter(
			(entry) => entry.line.kind === "request" && entry.next?.kind === "response",
		)
		.map((entry) => ({ request: entry.line, response: entry.next }));
}

function currentPingAnswers(): any[] {
	const lines = readFileSync(
		new URL("../../docs/control-plane.fixture.jsonl", import.meta.url),
		"utf8",
	)
		.split("\n")
		.filter((line) => line.trim() !== "")
		.map((line) => JSON.parse(line) as any);
	return lines.flatMap((request: any, index: number) => {
		const response = lines[index + 1];
		return request.method === "daemon.ping" && response?.id === request.id
			? [response.result]
			: [];
	});
}

/** The raw reply line a fake daemon writes, or `undefined` for silence. */
type Answer = (request: any) => string | undefined;

const rawReply = (request: any, result: unknown): string =>
	JSON.stringify({ id: request.id, result });
const reply = (request: any, result: unknown): string => {
	if (request.method !== "daemon.ping" || typeof result !== "object" || result === null)
		return rawReply(request, result);
	return rawReply(request, {
		backend: "tmux",
		instance: null,
		tasks: { available: false, error: null },
		...(result as Record<string, unknown>),
	});
};

const refuse = (request: any, code: string, message = ""): string =>
	JSON.stringify({ id: request.id, error: { code, message } });

class FakeSocket implements RadarSocket {
	private listeners = new Map<string, (value?: unknown) => void>();
	private answer: Answer;
	private requests: any[];
	constructor(answer: Answer, requests: any[]) {
		this.answer = answer;
		this.requests = requests;
	}
	on(event: string, listener: (value?: unknown) => void): unknown {
		this.listeners.set(event, listener);
		return this;
	}
	write(text: string): unknown {
		for (const line of text.split("\n").filter(Boolean)) {
			const request = JSON.parse(line);
			this.requests.push(request);
			queueMicrotask(() => {
				const body = this.answer(request);
				if (body !== undefined)
					this.listeners.get("data")?.(Buffer.from(`${body}\n`, "utf8"));
			});
		}
		return true;
	}
	connect(): void {
		this.listeners.get("connect")?.();
	}
	destroy(): void {}
}

type Fixture = {
	root: string;
	directory: string;
	path: string;
	requests: any[];
	dialled: number;
	server: Server;
	client(options?: RadarClientOptions): RadarClient;
};

/**
 * A fixture socket root that satisfies the trust check: the directory is 0700
 * and a real unix socket is bound at the path, while every dial is answered by
 * an in-memory fake. The daemon's own behaviour never runs here.
 */
function makeFixture(answer: Answer): Fixture {
	const root = mkdtempSync(join(tmpdir(), "radar-client-test-"));
	const directory = join(root, "socket");
	mkdirSync(directory, { mode: 0o700 });
	chmodSync(directory, 0o700);
	const path = join(directory, "control.sock");
	const requests: any[] = [];
	const server = createServer();
	const fixture: Fixture = {
		root,
		directory,
		path,
		requests,
		dialled: 0,
		server,
		client: (options = {}) =>
			createRadarClient({
				env: { RADAR_CONTROL_SOCKET: path },
				uid: UID,
				dial: () => {
					fixture.dialled += 1;
					const socket = new FakeSocket(answer, requests);
					queueMicrotask(() => socket.connect());
					return socket;
				},
				...options,
			}),
	};
	return fixture;
}

async function listen(fixture: Fixture): Promise<void> {
	await new Promise<void>((resolve, reject) => {
		fixture.server.once("error", reject);
		fixture.server.listen(fixture.path, () => {
			fixture.server.removeListener("error", reject);
			resolve();
		});
	});
}

function cleanup(fixture: Fixture): void {
	rmSync(fixture.root, { recursive: true, force: true });
	if (fixture.server.listening) fixture.server.close();
}

const OBSERVED_AGENTS = [
	{
		location: { workspace_id: "wA", tab_id: "wA:t1", pane_id: "wA:p1" },
		name: "pi",
		session: { kind: "uuid", value: UUID_A },
	},
	{
		location: { workspace_id: "wA", tab_id: "wA:t1", pane_id: "wA:p2" },
		session: {
			kind: "reported",
			value: { source: "herdr:pi", value: "/tmp/session.jsonl" },
		},
	},
	{
		location: { workspace_id: "wA", tab_id: "wA:t1", pane_id: "wA:p3" },
		session: null,
	},
	{ location: { workspace_id: "wA", tab_id: "wA:t1", pane_id: "wA:p4" } },
];

const OBSERVED_INVENTORY = {
	workspaces: [{ workspace_id: "wA", label: "main", number: 1 }],
	tabs: [{ tab_id: "wA:t1", workspace_id: "wA" }],
	panes: [
		{ location: { workspace_id: "wA", tab_id: "wA:t1", pane_id: "wA:p1" }, title: "pi" },
		{ location: { workspace_id: "wA", tab_id: "wA:t1", pane_id: "wA:p2" } },
	],
	agents: OBSERVED_AGENTS,
};

test("observe decodes the whole canonical fleet inventory", async () => {
	const fixture = makeFixture((request) => {
		if (request.method === "daemon.ping")
			return reply(request, {
				protocol: 2,
				revision: 1, contracts: ["registry.v1"],
			});
		assert.equal(request.method, "mux.observe");
		assert.deepEqual(request.params, {});
		return reply(request, { inventory: OBSERVED_INVENTORY });
	});
	try {
		await listen(fixture);
		assert.deepEqual(await fixture.client().observe(), {
			ok: true,
			value: {
				workspaces: [{ workspace_id: "wA" }],
				tabs: [{ tab_id: "wA:t1", workspace_id: "wA" }],
				panes: [
					{ workspace_id: "wA", tab_id: "wA:t1", pane_id: "wA:p1" },
					{ workspace_id: "wA", tab_id: "wA:t1", pane_id: "wA:p2" },
				],
				agents: [
					{
						workspace_id: "wA",
						tab_id: "wA:t1",
						pane_id: "wA:p1",
						name: "pi",
						session: { kind: "uuid", value: UUID_A },
					},
					{
						workspace_id: "wA",
						tab_id: "wA:t1",
						pane_id: "wA:p2",
						session: {
							kind: "reported",
							value: "/tmp/session.jsonl",
							source: "herdr:pi",
						},
					},
					{ workspace_id: "wA", tab_id: "wA:t1", pane_id: "wA:p3" },
					{ workspace_id: "wA", tab_id: "wA:t1", pane_id: "wA:p4" },
				],
			},
		});
	} finally {
		cleanup(fixture);
	}
});

test("observe preserves a managed label, run and an unrecognized status", async () => {
	const fixture = makeFixture((request) => {
		if (request.method === "daemon.ping")
			return reply(request, {
				protocol: 2,
				revision: 1, contracts: ["registry.v1"],
			});
		return reply(request, {
			inventory: {
				workspaces: [],
				tabs: [],
				panes: [],
				agents: [
					{
						location: { workspace_id: "wA", tab_id: "wA:t1", pane_id: "wA:p1" },
						status: { other: "compacting" },
						facts: { label: "worker", run: "run-1" },
					},
				],
			},
		});
	});
	try {
		await listen(fixture);
		const answer = await fixture.client().observe();
		assert.equal(answer.ok, true, JSON.stringify(answer));
		assert.deepEqual(answer.ok ? answer.value.agents[0] : undefined, {
			workspace_id: "wA",
			tab_id: "wA:t1",
			pane_id: "wA:p1",
			status: "compacting",
			managed_label: "worker",
			managed_run: "run-1",
		});
	} finally {
		cleanup(fixture);
	}
});

test("an inventory missing any canonical array is a malformed observation", async () => {
	// A tmux-backed daemon reports panes and no agents at all, so an absent array
	// is a broken observation rather than an empty fleet.
	const fixture = makeFixture((request) => {
		if (request.method === "daemon.ping")
			return reply(request, {
				protocol: 2,
				revision: 1, contracts: ["registry.v1"],
			});
		return reply(request, {
			inventory: { workspaces: [], tabs: [], agents: [] },
		});
	});
	try {
		await listen(fixture);
		const answer = await fixture.client().observe();
		assert.equal(answer.ok, false);
		assert.equal(answer.ok ? "" : answer.code, "protocol");
	} finally {
		cleanup(fixture);
	}
});

test("observe decodes observed locations and sessions after daemon negotiation", async () => {
	const fixture = makeFixture((request) => {
		if (request.method === "daemon.ping")
			return reply(request, {
				protocol: 2,
				revision: 1, contracts: ["registry.v1"],
			});
		assert.equal(request.method, "mux.observe");
		assert.deepEqual(request.params, {});
		return reply(request, {
			inventory: {
				workspaces: [],
				tabs: [],
				panes: [],
				agents: OBSERVED_AGENTS,
			},
		});
	});
	try {
		await listen(fixture);
		const answer = await fixture.client().observe();
		assert.equal(answer.ok, true, JSON.stringify(answer));
		assert.deepEqual(answer.ok ? answer.value.agents : undefined, [
			{
				workspace_id: "wA",
				tab_id: "wA:t1",
				pane_id: "wA:p1",
				name: "pi",
				session: { kind: "uuid", value: UUID_A },
			},
			{
				workspace_id: "wA",
				tab_id: "wA:t1",
				pane_id: "wA:p2",
				session: {
					kind: "reported",
					value: "/tmp/session.jsonl",
					source: "herdr:pi",
				},
			},
			{ workspace_id: "wA", tab_id: "wA:t1", pane_id: "wA:p3" },
			{ workspace_id: "wA", tab_id: "wA:t1", pane_id: "wA:p4" },
		]);
	} finally {
		cleanup(fixture);
	}
});

test("observe refuses an inventory it cannot decode exactly", async () => {
	for (const agents of [
		[{ location: { workspace_id: "wA", tab_id: "wA:t1" } }],
		[
			{
				location: { workspace_id: "wA", tab_id: "wA:t1", pane_id: "wA:p1" },
				session: { kind: "reported", value: "/tmp/session.jsonl" },
			},
		],
		[
			{
				location: { workspace_id: "wA", tab_id: "wA:t1", pane_id: "wA:p1" },
				session: { kind: "session", value: "not-a-uuid" },
			},
		],
	]) {
		const fixture = makeFixture((request) =>
			request.method === "daemon.ping"
				? reply(request, {
						protocol: 2,
						revision: 1, contracts: ["registry.v1"],
					})
				: reply(request, { inventory: { agents } }),
		);
		try {
			await listen(fixture);
			const result = await fixture.client().observe();
			assert.equal(result.ok, false);
			assert.equal(result.ok ? "" : result.code, "protocol");
		} finally {
			cleanup(fixture);
		}
	}
});

test("observe dispatches a protocol method without exposing backend primitives", async () => {
	const fixture = makeFixture((request) =>
		request.method === "daemon.ping"
			? reply(request, { protocol: 2, revision: 1, contracts: ["registry.v1"] })
			: refuse(request, "backend_unavailable", "the configured backend cannot observe"),
	);
	try {
		await listen(fixture);
		const result = await fixture.client().observe();
		assert.equal(result.ok === true ? "" : result.code, "backend_unavailable");
		assert.deepEqual(
			fixture.requests.map((request) => request.method),
			["daemon.ping", "mux.observe"],
		);
	} finally {
		cleanup(fixture);
	}
});

test("focus dispatches one pane target and keeps the caller's operation UUID", async () => {
	const fixture = makeFixture((request) => {
		if (request.method === "daemon.ping")
			return reply(request, {
				protocol: 2,
				revision: 1, contracts: ["registry.v1"],
			});
		assert.equal(request.method, "mux.focus");
		assert.equal(request.id, UUID_A);
		assert.deepEqual(request.params, {
			target: "wA:p1",
			target_kind: "pane",
			requester: "pi-radar",
		});
		return reply(request, {
			request: {
				id: UUID_A,
				method: "mux.focus",
				state: "completed",
				outcome: "completed",
				effects: ["mux.focus"],
			},
		});
	});
	try {
		await listen(fixture);
		assert.deepEqual(
			await fixture.client().focus({ request_id: UUID_A, pane_id: "wA:p1" }),
			{ ok: true, value: { request_id: UUID_A } },
		);
	} finally {
		cleanup(fixture);
	}
});

test("output and process_info decode the pinned read replies and send the pane target", async () => {
	const fixture = makeFixture((request) => {
		if (request.method === "daemon.ping")
			return reply(request, {
				protocol: 2,
				revision: 1, contracts: ["registry.v1"],
			});
		if (request.method === "mux.output") {
			assert.deepEqual(request.params, {
				pane_id: "tmux:srv:%1",
				source: "recent_unwrapped",
				lines: 80,
			});
			return reply(request, {
				output: { text: "line one\n", truncated: true, revision: 7 },
			});
		}
		assert.equal(request.method, "mux.process_info");
		return reply(request, {
			evidence: { kind: "non_shell", pid: 42, name: "pi", command: "pi -p" },
		});
	});
	try {
		await listen(fixture);
		const client = fixture.client();
		assert.deepEqual(
			await client.output({
				pane_id: "tmux:srv:%1",
				source: "recent_unwrapped",
				lines: 80,
			}),
			{ ok: true, value: { text: "line one\n", truncated: true, revision: 7 } },
		);
		assert.deepEqual(await client.processInfo({ pane_id: "tmux:srv:%1" }), {
			ok: true,
			value: { kind: "non_shell", pid: 42, name: "pi", command: "pi -p" },
		});
	} finally {
		cleanup(fixture);
	}
});

test("output and process_info pass backend refusals through without primitive negotiation", async () => {
	const fixture = makeFixture((request) =>
		request.method === "daemon.ping"
			? reply(request, { protocol: 2, revision: 1, contracts: ["registry.v1"] })
			: refuse(request, "backend_unavailable", "the configured backend cannot read this pane"),
	);
	try {
		await listen(fixture);
		const client = fixture.client();
		const output = await client.output({
			pane_id: "tmux:srv:%1",
			source: "recent_unwrapped",
		});
		const process = await client.processInfo({ pane_id: "tmux:srv:%1" });
		assert.equal(output.ok === true ? "" : output.code, "backend_unavailable");
		assert.equal(process.ok === true ? "" : process.code, "backend_unavailable");
		assert.deepEqual(
			fixture.requests.map((request) => request.method),
			["daemon.ping", "mux.output", "mux.process_info"],
		);
	} finally {
		cleanup(fixture);
	}
});

test("focus requires a canonical operation UUID and dials nothing without one", async () => {
	const fixture = makeFixture(() => undefined);
	try {
		const result = await fixture.client().focus({
			request_id: "not-a-uuid",
			pane_id: "wA:p1",
		});
		assert.equal(result.ok === true ? "" : result.code, "bad_params");
		assert.equal(fixture.dialled, 0);
		assert.deepEqual(fixture.requests, []);
	} finally {
		cleanup(fixture);
	}
});

test("focus passes a backend refusal through after protocol negotiation", async () => {
	const fixture = makeFixture((request) =>
		request.method === "daemon.ping"
			? reply(request, { protocol: 2, revision: 1, contracts: ["registry.v1"] })
			: refuse(request, "backend_unavailable", "the configured backend cannot focus"),
	);
	try {
		await listen(fixture);
		const result = await fixture.client().focus({
			request_id: UUID_A,
			pane_id: "wA:p1",
		});
		assert.equal(result.ok === true ? "" : result.code, "backend_unavailable");
		assert.deepEqual(
			fixture.requests.map((request) => request.method),
			["daemon.ping", "mux.focus"],
		);
	} finally {
		cleanup(fixture);
	}
});

test("a refused focus record is a refusal, never an unresolved operation", async () => {
	const fixture = makeFixture((request) =>
		request.method === "daemon.ping"
			? reply(request, {
					protocol: 2,
					revision: 1, contracts: ["registry.v1"],
				})
			: reply(request, {
					request: {
						id: UUID_A,
						method: "mux.focus",
						state: "completed",
						outcome: "refused",
						message: "the backend refused to focus the pane",
					},
				}),
	);
	try {
		await listen(fixture);
		const result = await fixture.client().focus({
			request_id: UUID_A,
			pane_id: "wA:p1",
		});
		assert.equal(result.ok === true ? "" : result.code, "refused");
		assert.equal(
			result.ok === true ? "" : result.message,
			"the backend refused to focus the pane",
		);
	} finally {
		cleanup(fixture);
	}
});

test("a lost focus reply is read back by its exact UUID and never resent", async () => {
	for (const record of [
		{
			id: UUID_A,
			method: "mux.focus",
			state: "completed",
			outcome: "unknown",
		},
		{ id: UUID_A, method: "mux.focus", state: "started" },
		undefined,
	]) {
		let lost = false;
		const fixture = makeFixture((request) => {
			if (request.method === "daemon.ping")
				return reply(request, {
					protocol: 2,
					revision: 1, contracts: ["registry.v1"],
				});
			if (request.method === "mux.focus") {
				lost = true;
				return undefined;
			}
			assert.equal(request.method, "mux.request.get");
			assert.deepEqual(request.params, { id: UUID_A });
			if (record === undefined) return refuse(request, "not_found", "no request");
			return reply(request, { request: record });
		});
		try {
			await listen(fixture);
			const result = await fixture
				.client({ timeoutMs: 20 })
				.focus({ request_id: UUID_A, pane_id: "wA:p1" });
			assert.ok(lost);
			assert.equal(result.ok === true ? "" : result.code, "unknown");
			assert.match(result.ok === true ? "" : result.message, new RegExp(UUID_A));
			assert.deepEqual(
				fixture.requests.map((request) => [request.method, request.id]),
				[
					["daemon.ping", fixture.requests[0]!.id],
					["mux.focus", UUID_A],
					["mux.request.get", fixture.requests[2]!.id],
				],
			);
		} finally {
			cleanup(fixture);
		}
	}
});

test("a lost focus reply whose record completed settles as completed", async () => {
	const fixture = makeFixture((request) => {
		if (request.method === "daemon.ping")
			return reply(request, {
				protocol: 2,
				revision: 1, contracts: ["registry.v1"],
			});
		if (request.method === "mux.focus") return undefined;
		return reply(request, {
			request: {
				id: UUID_A,
				method: "mux.focus",
				state: "completed",
				outcome: "completed",
				effects: ["mux.focus"],
			},
		});
	});
	try {
		await listen(fixture);
		assert.deepEqual(
			await fixture.client({ timeoutMs: 20 }).focus({
				request_id: UUID_A,
				pane_id: "wA:p1",
			}),
			{ ok: true, value: { request_id: UUID_A } },
		);
		assert.deepEqual(
			fixture.requests
				.filter((request) => request.method === "mux.focus")
				.map((request) => request.id),
			[UUID_A],
		);
	} finally {
		cleanup(fixture);
	}
});

test("an internal focus failure is unresolved rather than a refusal", async () => {
	const fixture = makeFixture((request) => {
		if (request.method === "daemon.ping")
			return reply(request, {
				protocol: 2,
				revision: 1, contracts: ["registry.v1"],
			});
		if (request.method === "mux.focus") return refuse(request, "internal", "boom");
		assert.equal(request.method, "mux.request.get");
		assert.deepEqual(request.params, { id: UUID_A });
		return reply(request, {
			request: { id: UUID_A, method: "mux.focus", state: "started" },
		});
	});
	try {
		await listen(fixture);
		const result = await fixture.client().focus({
			request_id: UUID_A,
			pane_id: "wA:p1",
		});
		assert.equal(result.ok === true ? "" : result.code, "unknown");
	} finally {
		cleanup(fixture);
	}
});

test("a focus reply carrying no operation record is unresolved, never success", async () => {
	const fixture = makeFixture((request) => {
		if (request.method === "daemon.ping")
			return reply(request, {
				protocol: 2,
				revision: 1, contracts: ["registry.v1"],
			});
		if (request.method === "mux.focus") return reply(request, {});
		return refuse(request, "unknown_method", "unexpected method");
	});
	try {
		await listen(fixture);
		const result = await fixture.client().focus({
			request_id: UUID_A,
			pane_id: "wA:p1",
		});
		assert.equal(result.ok === true ? "" : result.code, "unknown");
		assert.match(
			result.ok === true ? "" : result.message,
			/returned no operation record/u,
		);
	} finally {
		cleanup(fixture);
	}
});

test("child.close sends the exact spawn edge and accepts only confirmed pane closure", async () => {
	const fixture = makeFixture((request) => {
		if (request.method === "daemon.ping")
			return reply(request, {
				protocol: 2,
				revision: 1, contracts: ["registry.v1", "spawn.close.v1"],
			});
		assert.equal(request.method, "spawn.close");
		assert.deepEqual(request.params, {
			spawn_request_id: UUID_A,
			source: "herdsman-child",
			incarnation: UUID_B,
			intent: "complete",
		});
		return reply(request, { close: { outcome: "completed", intent: "complete", pane: "wA:p2" } });
	});
	try {
		await listen(fixture);
		assert.deepEqual(await fixture.client().childClose({
			request_id: UUID_B,
			spawn_request_id: UUID_A,
			source: "herdsman-child",
			incarnation: UUID_B,
			intent: "complete",
		}), { ok: true, value: { outcome: "completed", pane: "wA:p2" } });
	} finally { cleanup(fixture); }
});

test("agent.get decodes each process verdict exactly as the daemon names it", async () => {
	for (const verification of ["verified", "absent", "mismatched", "unavailable"]) {
		const fixture = makeFixture((request) =>
			request.method === "daemon.ping"
				? reply(request, { protocol: 2, revision: 1, contracts: ["registry.v1"] })
				: reply(request, { agent: { registration: {}, process: { claimed: true, verification } } }),
		);
		try {
			await listen(fixture);
			assert.deepEqual(await fixture.client().agentGet(UUID_A), {
				ok: true,
				value: { claimed: true, verification },
			});
			const read = fixture.requests.find((request) => request.method === "agent.get");
			assert.deepEqual(read.params, { agent_id: UUID_A });
		} finally { cleanup(fixture); }
	}
});

test("agent.get reports the child's activity only while its execution report is fresh", async () => {
	const cases = [
		{ snapshot: { freshness: "fresh", snapshot: { activity: "working" } }, activity: "working" },
		{ snapshot: { freshness: "stale", snapshot: { activity: "working" } }, activity: undefined },
		{ snapshot: null, activity: undefined },
	];
	for (const { snapshot, activity } of cases) {
		const fixture = makeFixture((request) =>
			request.method === "daemon.ping"
				? reply(request, { protocol: 2, revision: 1, contracts: ["registry.v1"] })
				: reply(request, {
						agent: {
							registration: {},
							process: { claimed: true, verification: "verified" },
							execution: { snapshot },
						},
					}),
		);
		try {
			await listen(fixture);
			const read = await fixture.client().agentGet(UUID_A);
			assert.equal(read.ok === true ? read.value.activity : "refused", activity);
		} finally { cleanup(fixture); }
	}
});

test("agent.get reports the current session only while its context is fresh", async () => {
	const session = "11111111-2222-4333-8444-555555555555";
	const cases = [
		{ context: { freshness: "fresh", session }, contextSession: session },
		{ context: { freshness: "stale", session }, contextSession: undefined },
		{ context: { freshness: "fresh", session: null }, contextSession: undefined },
	];
	for (const { context, contextSession } of cases) {
		const fixture = makeFixture((request) =>
			request.method === "daemon.ping"
				? reply(request, { protocol: 2, revision: 1, contracts: ["registry.v1"] })
				: reply(request, {
						agent: {
							registration: {},
							process: { claimed: true, verification: "verified" },
							context: { version: 1, agent_id: UUID_A, context },
						},
					}),
		);
		try {
			await listen(fixture);
			const read = await fixture.client().agentGet(UUID_A);
			assert.equal(read.ok === true ? read.value.contextSession : "refused", contextSession);
		} finally { cleanup(fixture); }
	}
});

test("agent.get refuses a reply with no known verdict and passes not_found through", async () => {
	const malformed = makeFixture((request) =>
		request.method === "daemon.ping"
			? reply(request, { protocol: 2, revision: 1, contracts: ["registry.v1"] })
			: reply(request, { agent: { process: { claimed: true, verification: "probably_gone" } } }),
	);
	const missing = makeFixture((request) =>
		request.method === "daemon.ping"
			? reply(request, { protocol: 2, revision: 1, contracts: ["registry.v1"] })
			: refuse(request, "not_found", "unknown agent"),
	);
	try {
		await listen(malformed);
		const verdict = await malformed.client().agentGet(UUID_A);
		assert.equal(verdict.ok, false);
		assert.equal(verdict.ok ? "" : verdict.code, "protocol");
		await listen(missing);
		const absent = await missing.client().agentGet(UUID_A);
		assert.equal(absent.ok, false);
		assert.equal(absent.ok ? "" : absent.code, "not_found");
	} finally { cleanup(malformed); cleanup(missing); }
});

test("child.close unknown outcome is preserved as failure", async () => {
	const fixture = makeFixture((request) => request.method === "daemon.ping"
		? reply(request, { protocol: 2, revision: 1, contracts: ["registry.v1", "spawn.close.v1"] })
		: refuse(request, "backend_unavailable", "managed child close outcome is unknown"));
	try {
		await listen(fixture);
		const result = await fixture.client().childClose({
			request_id: UUID_B, spawn_request_id: UUID_A, source: "herdsman-child",
			incarnation: UUID_B, intent: "cancel",
		});
		assert.equal(result.ok, false);
		assert.equal(result.ok ? "" : result.code, "backend_unavailable");
	} finally { cleanup(fixture); }
});

test("an untrusted control socket is refused without dialling", async () => {
	const fixture = makeFixture(() => undefined);
	try {
		const absent = fixture.client();
		assert.deepEqual(await absent.ping(), {
			ok: false,
			code: "absent",
			message: `${fixture.path} is unavailable (ENOENT)`,
		});

		writeFileSync(fixture.path, "");
		assert.match(untrustedControlSocket(fixture.path, UID) ?? "", /not a socket/);

		chmodSync(fixture.directory, 0o755);
		assert.match(
			untrustedControlSocket(fixture.path, UID) ?? "",
			/not mode 0700/,
		);

		assert.equal(fixture.dialled, 0);
	} finally {
		cleanup(fixture);
	}
});

test("daemon.ping requires protocol 2, revision 1, and registry.v1", async () => {
	const answers = [
		(request: any) =>
			reply(request, { protocol: 1, revision: 1, contracts: ["registry.v1"] }),
		(request: any) =>
			reply(request, { protocol: 2, contracts: ["registry.v1"] }),
		(request: any) =>
			reply(request, {
				protocol: 2,
				backend: "sqlite",
				revision: 1,
				contracts: ["registry.v1", "future.contract.v9"],
			}),
	];
	const fixture = makeFixture((request) => answers.shift()?.(request));
	try {
		await listen(fixture);
		const client = fixture.client();
		const wrongVersion = await client.ping();
		assert.equal(wrongVersion.ok, false);
		assert.equal(wrongVersion.ok ? "" : wrongVersion.code, "unsupported");
		const oldRevision = await client.ping();
		assert.equal(oldRevision.ok, false);
		assert.match(oldRevision.ok ? "" : oldRevision.message, /restart radar-daemon/);
		const capable = await client.ping();
		assert.deepEqual(capable, {
			ok: true,
			value: {
				protocol: 2,
				backend: "sqlite",
				revision: 1,
				instance: null,
				contracts: ["registry.v1", "future.contract.v9"],
				tasks: { available: false, error: null },
			},
		});
	} finally {
		cleanup(fixture);
	}
});

test("daemon.ping rejects malformed contract lists", async () => {
	const fixture = makeFixture((request) =>
		reply(request, { protocol: 2, revision: 1, contracts: ["registry.v1", 7] }),
	);
	try {
		await listen(fixture);
		const result = await fixture.client().ping();
		assert.equal(result.ok, false);
		assert.equal(result.ok ? "" : result.code, "protocol");
	} finally {
		cleanup(fixture);
	}
});

test("every repository daemon.ping fixture is understood unchanged", async () => {
	const answers = currentPingAnswers();
	assert.ok(answers.length >= 2, "the repository fixture covers multiple daemon configurations");
	const seen = new Set(answers.flatMap((answer) => answer.contracts));
	for (const contract of [
		"registry.v1",
		"registry.facts.v1",
		"events.v1",
		"spawn.v1",
		"spawn.close.v1",
		"spawn.launch_context.v1",
	])
		assert.ok(seen.has(contract), `the repository fixtures cover ${contract}`);
	for (const expected of answers) {
		const fixture = makeFixture((request) => reply(request, expected));
		try {
			await listen(fixture);
			const result = await fixture.client().ping();
			assert.equal(result.ok, true, JSON.stringify(result));
			if (result.ok) {
				assert.equal(result.value.protocol, expected.protocol);
				assert.equal(result.value.revision, expected.revision);
				assert.equal(result.value.backend, expected.backend);
				assert.equal(result.value.instance ?? null, expected.instance ?? null);
				assert.deepEqual(result.value.contracts, expected.contracts);
			}
		} finally {
			cleanup(fixture);
		}
	}
});

test("a handshake carries the instance the backend addresses", async () => {
	// The server a tmux daemon drives is its own fact, and a client that has to
	// name a pane of that server reads it here rather than from its environment.
	const fixture = makeFixture((request) =>
		reply(request, {
			protocol: 2,
			backend: "tmux",
			instance: "work",
			revision: 1, contracts: ["registry.v1"],
		}),
	);
	try {
		await listen(fixture);
		const ready = await fixture.client().ping();
		assert.equal(ready.ok, true, JSON.stringify(ready));
		assert.equal(ready.ok ? ready.value.instance : undefined, "work");
	} finally {
		cleanup(fixture);
	}
});

test("failed registry negotiation is retried on the same client", async () => {
	let negotiations = 0;
	const fixture = makeFixture((request) => {
		if (request.method === "daemon.ping") {
			negotiations += 1;
			return negotiations === 1
				? undefined
				: reply(request, { protocol: 2, revision: 1, contracts: ["registry.v1"] });
		}
		return reply(request, { registration: { agent_id: UUID_A } });
	});
	try {
		await listen(fixture);
		const client = fixture.client({ timeoutMs: 100 });
		const first = await client.register({ source: "herdsman-pi", incarnation: UUID_A });
		assert.equal(first.ok, false);
		assert.equal(first.ok ? "" : first.code, "timeout");
		assert.equal((await client.ping()).ok, true);
		const recovered = await client.register({ source: "herdsman-pi", incarnation: UUID_A });
		assert.deepEqual(recovered, { ok: true, value: { agent_id: UUID_A } });
		assert.equal(negotiations, 3);
	} finally {
		cleanup(fixture);
	}
});

test("facts and assignment writes require the facts contract before dispatch", async () => {
	const fixture = makeFixture((request) =>
		reply(request, { protocol: 2, revision: 1, contracts: ["registry.v1"] }),
	);
	try {
		await listen(fixture);
		const client = fixture.client();
		const facts = await client.publish({
			agent_id: UUID_A,
			channel: "facts",
			writer_handle: FIXTURE_WRITER,
			sequence: 1,
			snapshot: { model: "model" },
		});
		const assignment = await client.acquire({
			agent_id: UUID_A,
			channel: "assignment",
			publisher: { source: "pi-radar", incarnation: UUID_A },
		});
		assert.equal(facts.ok ? "" : facts.code, "unsupported");
		assert.equal(assignment.ok ? "" : assignment.code, "unsupported");
		assert.deepEqual(fixture.requests.map((request) => request.method), ["daemon.ping"]);
	} finally {
		cleanup(fixture);
	}
});

test("facts publication requires registry.facts.v1 and preserves typed fields", async () => {
	const snapshot = {
		context_window: { tokens: 86_000, window: 200_000 },
		background_tasks: [{ id: "bg-1", phase: "running" }],
		background_started_unix_ms: 1_700_000_000_000,
	};
	const fixture = makeFixture((request) =>
		request.method === "daemon.ping"
			? reply(request, {
					protocol: 2,
					revision: 1,
					contracts: ["registry.v1", "registry.facts.v1"],
				})
			: reply(request, { channel: { writer: { sequence: 1 } } }),
	);
	try {
		await listen(fixture);
		const result = await fixture.client().publish({
			agent_id: UUID_A,
			channel: "facts",
			writer_handle: FIXTURE_WRITER,
			sequence: 1,
			snapshot,
		});
		assert.deepEqual(result, { ok: true, value: { sequence: 1 } });
		assert.deepEqual(
			fixture.requests.find((request) => request.method === "agent.publish")?.params.snapshot,
			snapshot,
		);
	} finally {
		cleanup(fixture);
	}
});

test("facts publication rejects legacy text and invalid token ranges before dialing", async () => {
	const fixture = makeFixture(() => undefined);
	const client = fixture.client();
	try {
		for (const snapshot of [
			{ context_usage: "43%" },
			{ context_window: { tokens: 201, window: 200 } },
		]) {
			const result = await client.publish({
				agent_id: UUID_A,
				channel: "facts",
				writer_handle: FIXTURE_WRITER,
				sequence: 1,
				snapshot,
			} as any);
			assert.equal(result.ok, false);
			assert.equal(result.ok ? "" : result.code, "bad_params");
		}
		assert.equal(fixture.dialled, 0);
	} finally {
		cleanup(fixture);
	}
});

test("ping refuses a daemon that cannot register agents", async () => {
	const fixture = makeFixture((request) =>
		reply(request, { protocol: 2, revision: 1, contracts: ["presence"] }),
	);
	try {
		await listen(fixture);
		const result = await fixture.client().ping();
		assert.equal(result.ok, false);
		assert.equal(result.ok ? "" : result.code, "unsupported");
		assert.match(result.ok ? "" : result.message, /registry\.v1/);
	} finally {
		cleanup(fixture);
	}
});

test("a reply is bounded, valid JSON and matched to its request id", async () => {
	const replies: string[] = [
		"not json",
		JSON.stringify({ id: UUID_B, result: {} }),
		JSON.stringify({ id: "", result: {} }),
		`${JSON.stringify({ result: {} })}`,
	];
	const fixture = makeFixture(() => replies.shift());
	try {
		await listen(fixture);
		const client = fixture.client();
		for (const expected of [
			"reply is not JSON",
			"reply does not match the request id",
			"reply does not match the request id",
			"reply does not match the request id",
		]) {
			const result = await client.ping();
			assert.equal(result.ok, false);
			assert.equal(result.ok ? "" : result.code, "protocol");
			assert.equal(result.ok ? "" : result.message, expected);
		}
	} finally {
		cleanup(fixture);
	}
});

test("an oversized reply is abandoned rather than buffered", async () => {
	const fixture = makeFixture((request) =>
		JSON.stringify({
			id: request.id,
			result: { padding: "x".repeat(RADAR_MAX_LINE_BYTES) },
		}),
	);
	try {
		await listen(fixture);
		const result = await fixture.client().ping();
		assert.equal(result.ok, false);
		assert.equal(result.ok ? "" : result.code, "protocol");
		assert.match(result.ok ? "" : result.message, /line bound/);
	} finally {
		cleanup(fixture);
	}
});

test("nothing gets a reply when the daemon never answers", async () => {
	const fixture = makeFixture(() => undefined);
	try {
		await listen(fixture);
		const result = await fixture
			.client({ timeoutMs: 20 })
			.register({ source: "herdsman-pi", incarnation: UUID_A });
		assert.equal(result.ok, false);
		assert.equal(result.ok ? "" : result.code, "timeout");
	} finally {
		cleanup(fixture);
	}
});

test("daemon refusals pass through and unknown codes do not", async () => {
	const codes = ["refused", "not_found", "made_up"];
	const fixture = makeFixture((request) =>
		refuse(request, codes.shift() ?? "refused", "fenced"),
	);
	try {
		await listen(fixture);
		const client = fixture.client();
		for (const [code, message] of [
			["refused", "refused: fenced"],
			["not_found", "not_found: fenced"],
			["internal", "made_up: fenced"],
		]) {
			const result = await client.ping();
			assert.deepEqual(result, { ok: false, code, message });
		}
	} finally {
		cleanup(fixture);
	}
});

test("unsupported daemons are never sent registry methods", async () => {
	const fixture = makeFixture((request) =>
		reply(request, { protocol: 2, revision: 1, contracts: ["presence"] }),
	);
	try {
		await listen(fixture);
		const unsupported = fixture.client();
		assert.equal(
			(await unsupported.register({ source: "herdsman-pi", incarnation: UUID_A })).ok,
			false,
		);
		assert.deepEqual(fixture.requests.map(({ method }) => method), ["daemon.ping"]);
	} finally {
		cleanup(fixture);
	}
});

test("registry calls put only the contract's own fields on the wire", async () => {
	const fixture = makeFixture((request) => {
		switch (request.method) {
			case "daemon.ping":
				return reply(request, { protocol: 2, revision: 1, contracts: ["registry.v1"] });
			case "agent.register":
				return reply(request, { registration: { agent_id: UUID_A } });
			case "agent.acquire":
				return reply(request, {
					writer: {
						handle: UUID_B,
						source: "herdsman-pi",
						incarnation: UUID_A,
						generation: 1,
						sequence: 0,
					},
				});
			case "agent.publish":
				return reply(request, { channel: { writer: { sequence: 7 } } });
			default:
				return reply(request, {});
		}
	});
	try {
		await listen(fixture);
		const client = fixture.client();
		const registered = await client.register({
			source: "herdsman-pi",
			incarnation: UUID_A,
			session: UUID_B,
			owner: "owner",
			run: "run",
			label: "worker",
			location: { backend: "tmux", pane: "%1" },
			process: { boot_id: "boot", pid: 42, start_ticks: 99 },
		});
		assert.deepEqual(registered, { ok: true, value: { agent_id: UUID_A } });

		const acquired = await client.acquire({
			agent_id: UUID_A,
			channel: "execution",
			publisher: {
				source: "herdsman-pi",
				incarnation: UUID_A,
				reporting_owner: "owner",
			},
		});
		assert.equal(acquired.ok, true);
		assert.equal(acquired.ok ? acquired.value.handle : "", UUID_B);

		const published = await client.publish({
			agent_id: UUID_A,
			channel: "execution",
			writer_handle: UUID_B,
			sequence: 0,
			snapshot: { activity: "working" },
		});
		assert.deepEqual(published, { ok: true, value: { sequence: 7 } });
		await client.retire({
			agent_id: UUID_A,
			channel: "execution",
			writer_handle: UUID_B,
		});

		assert.deepEqual(
			fixture.requests.map(({ version, method }) => [version, method]),
			[
				[2, "daemon.ping"],
				[2, "agent.register"],
				[2, "agent.acquire"],
				[2, "agent.publish"],
				[2, "agent.retire"],
			],
		);
		assert.deepEqual(
			fixture.requests.filter(({ method }) => method !== "daemon.ping").map(({ method, params }) => [
				method,
				params,
			]),
			[
				[
					"agent.register",
					{
						source: "herdsman-pi",
						incarnation: UUID_A,
						session: UUID_B,
						owner: "owner",
						run: "run",
						label: "worker",
						location: { backend: "tmux", pane: "%1" },
						process: { boot_id: "boot", pid: 42, start_ticks: 99 },
					},
				],
				[
					"agent.acquire",
					{
						agent_id: UUID_A,
						channel: "execution",
						publisher: {
							source: "herdsman-pi",
							incarnation: UUID_A,
							reporting_owner: "owner",
						},
					},
				],
				[
					"agent.publish",
					{
						agent_id: UUID_A,
						channel: "execution",
						writer_handle: UUID_B,
						sequence: 0,
						snapshot: { activity: "working" },
					},
				],
				[
					"agent.retire",
					{
						agent_id: UUID_A,
						channel: "execution",
						writer_handle: UUID_B,
					},
				],
			],
		);
	} finally {
		cleanup(fixture);
	}
});

test("an ambivalent registration reply is not a registration", async () => {
	const fixture = makeFixture((request) =>
		request.method === "daemon.ping"
			? reply(request, { protocol: 2, revision: 1, contracts: ["registry.v1"] })
			: reply(request, { registration: { agent_id: "not-a-uuid" } }),
	);
	try {
		await listen(fixture);
		const result = await fixture
			.client()
			.register({ source: "herdsman-pi", incarnation: UUID_A });
		assert.equal(result.ok, false);
		assert.equal(result.ok ? "" : result.code, "protocol");
	} finally {
		cleanup(fixture);
	}
});

test("the context channel carries only the session and keeps the returned binding", async () => {
	const fixture = makeFixture((request) =>
		request.method === "daemon.ping"
			? reply(request, { protocol: 2, revision: 1, contracts: ["registry.v1"] })
			: reply(request, {
					writer: {
						handle: FIXTURE_WRITER,
						source: "herdsman-pi",
						incarnation: UUID_A,
						generation: 1,
						sequence: request.params.sequence,
					},
					context: { version: 1, agent_id: UUID_A },
					warning: null,
				}),
	);
	try {
		await listen(fixture);
		const client = fixture.client();
		const first = await client.context({
			agent_id: UUID_A,
			publisher: { source: "herdsman-pi", incarnation: UUID_A },
			sequence: 1,
			lease_ms: 30_000,
			context: { session: UUID_B },
		});
		assert.deepEqual(first, {
			ok: true,
			value: {
				writer: {
					handle: FIXTURE_WRITER,
					source: "herdsman-pi",
					incarnation: UUID_A,
					generation: 1,
					sequence: 1,
				},
			},
		});
		// A first publish omits both credentials the daemon would issue, and the
		// payload is the session alone.
		const sent = () =>
			fixture.requests.filter((request) => request.method === "agent.context");
		assert.equal("writer_handle" in sent()[0].params, false);
		assert.equal("replace" in sent()[0].params, false);
		assert.deepEqual(sent()[0].params.context, { session: UUID_B });

		// Once issued, the writer is presented again: a session switch is a newer
		// sequence under the same binding, never another subject.
		const second = await client.context({
			agent_id: UUID_A,
			publisher: { source: "herdsman-pi", incarnation: UUID_A },
			writer_handle: FIXTURE_WRITER,
			sequence: 2,
			context: { session: null },
		});
		assert.equal(sent()[1].params.writer_handle, FIXTURE_WRITER);
		assert.deepEqual(sent()[1].params.context, { session: null });
		assert.equal(second.ok, true);
	} finally {
		cleanup(fixture);
	}
});

test("the canonical daemon contract fixture round-trips through this client", async () => {
	// The fixture answers one request shape twice — a session switch and its
	// identical replay — so each shape consumes its answers in the fixture's own
	// order rather than by method alone.
	const exchanges = fixtureExchanges().map((entry) => {
		const params = entry.request.params as {
			launch?: unknown;
		};
		// This client never sends the fixture's optional private launch
		// specification: it would put paths on the wire and advertises nothing this
		// client needs. Everything else is sent exactly as the fixture wrote it.
		const { launch, ...request } = materialise(params);
		return {
			method: entry.request.method,
			launch,
			key: `${entry.request.method}\u0000${JSON.stringify(request)}`,
			request,
			response: materialise(entry.response),
		};
	});
	const pending = new Map<string, typeof exchanges>();
	for (const exchange of exchanges)
		pending.set(exchange.key, [
			...(pending.get(exchange.key) ?? []),
			exchange,
		]);
	const byMethod = (method: string) =>
		exchanges.find((entry) => entry.method === method)!.request;
	const fixture = makeFixture((request) => {
		if (request.method === "daemon.ping")
			return reply(request, {
				protocol: 2,
				revision: 1,
				contracts: ["registry.v1", "registry.facts.v1"],
			});
		const exchange = pending
			.get(`${request.method}\u0000${JSON.stringify(request.params)}`)
			?.shift();
		if (exchange === undefined)
			return refuse(
				request,
				"bad_params",
				`the fixture has no ${request.method} exchange for this request`,
			);
		return JSON.stringify({ ...exchange.response, id: request.id });
	});
	try {
		await listen(fixture);
		const client = fixture.client();

		// The fixture's registration exercises its optional private launch
		// specification, which this client omits.
		const registration = byMethod("agent.register");
		assert.ok(
			exchanges.find((entry) => entry.method === "agent.register")!.launch,
			"the fixture covers the optional launch specification",
		);
		assert.deepEqual(await client.register(registration), {
			ok: true,
			value: { agent_id: "8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22" },
		});

		const acquire = byMethod("agent.acquire");
		assert.deepEqual(await client.acquire(acquire), {
			ok: true,
			value: {
				handle: FIXTURE_WRITER,
				source: "herdsman-owner",
				incarnation: "9f8e7d6c-4321-4def-8abc-0123456789ab",
				reporting_owner: "owner-1",
				generation: 1,
				sequence: 0,
			},
		});

		const publish = byMethod("agent.publish");
		assert.deepEqual(await client.publish(publish), {
			ok: true,
			value: { sequence: 1 },
		});

		// Every context exchange the fixture documents, in its own order: a first
		// publish, a switch, that switch's identical replay, an explicit null, and a
		// replacement of the retired or expired incumbent.
		const contextRequests = exchanges
			.filter((entry) => entry.method === "agent.context")
			.map((entry) => entry.request);
		const firstContext = contextRequests.find(
			(entry) => entry.writer_handle === undefined && entry.replace === undefined,
		)!;
		const switchedContext = contextRequests.find(
			(entry) => entry.writer_handle !== undefined && entry.context.session !== null,
		)!;
		const nullContext = contextRequests.find(
			(entry) => entry.context.session === null,
		)!;
		const replacementContext = contextRequests.find(
			(entry) => entry.replace !== undefined,
		)!;
		const incumbent = "1c2d3e4f-5678-4abc-9def-0123456789ab";
		assert.deepEqual(await client.context(firstContext), {
			ok: true,
			value: {
				writer: {
					handle: FIXTURE_WRITER,
					source: "herdsman",
					incarnation: incumbent,
					generation: 1,
					sequence: 1,
				},
			},
		});
		assert.deepEqual(await client.context(switchedContext), {
			ok: true,
			value: {
				writer: {
					handle: FIXTURE_WRITER,
					source: "herdsman",
					incarnation: incumbent,
					generation: 1,
					sequence: 2,
				},
			},
		});
		// The identical replay is answered, never refused, and renews no lease.
		assert.deepEqual(await client.context(switchedContext), {
			ok: true,
			value: {
				writer: {
					handle: FIXTURE_WRITER,
					source: "herdsman",
					incarnation: incumbent,
					generation: 1,
					sequence: 2,
				},
				warning: "<replay-warning>",
			},
		});
		// An explicit null is an accepted fact, not a missing one.
		assert.equal(
			(await client.context(nullContext)).ok,
			true,
			"an explicit null session is served",
		);
		// The replacement names the exact incumbent it observed and comes back as
		// the successor binding, one generation on.
		assert.deepEqual(await client.context(replacementContext), {
			ok: true,
			value: {
				writer: {
					handle: FIXTURE_SUCCESSOR,
					source: "herdsman",
					incarnation: "2c3d4e5f-6789-4abc-9def-0123456789ab",
					generation: 2,
					sequence: 1,
				},
			},
		});

		const retire = byMethod("agent.retire");
		assert.deepEqual(await client.retire(retire), {
			ok: true,
			value: undefined,
		});

		assert.deepEqual(
			fixture.requests.map((request) => [
				request.version,
				request.method,
				request.params,
			]),
			[
				[2, "daemon.ping", {}],
				[2, "agent.register", registration],
				[2, "agent.acquire", acquire],
				[2, "agent.publish", publish],
				[2, "agent.context", firstContext],
				[2, "agent.context", switchedContext],
				[2, "agent.context", switchedContext],
				[2, "agent.context", nullContext],
				[2, "agent.context", replacementContext],
				[2, "agent.retire", retire],
			],
		);
		// The vendor fixture's private-path sentinel never reaches the wire.
		assert.equal(JSON.stringify(fixture.requests).includes("SENTINEL"), false);
	} finally {
		cleanup(fixture);
	}
});

test("the control socket path follows the environment", async () => {
	const fixture = makeFixture(() => undefined);
	try {
		assert.equal(fixture.client().socketPath(), join(fixture.directory, "control.sock"));
		assert.equal(
			createRadarClient({ env: {}, uid: 7 }).socketPath(),
			"/tmp/agent-radar-7/control.sock",
		);
		assert.equal(
			createRadarClient({ env: { XDG_RUNTIME_DIR: "/run/user/7" }, uid: 7 })
				.socketPath(),
			"/run/user/7/agent-radar/control.sock",
		);
	} finally {
		cleanup(fixture);
	}
});

const EVENTS_CONTRACTS = { protocol: 2, revision: 1, contracts: ["registry.v1", "events.v1"] };

/** Wait until `done` holds or the deadline passes, then return what it saw. */
async function until(done: () => boolean, ms = 2_000): Promise<void> {
	const deadline = Date.now() + ms;
	while (!done() && Date.now() < deadline)
		await new Promise((resolve) => setTimeout(resolve, 5));
}

test("events without events.v1 is an explicit refusal that dispatches nothing", async () => {
	const fixture = makeFixture((request) =>
		reply(request, { protocol: 2, revision: 1, contracts: ["registry.v1"] }),
	);
	try {
		await listen(fixture);
		const result = await fixture.client().events({ after: 0, wait_ms: 0 });
		assert.equal(result.ok === true ? "" : result.code, "unsupported");
		assert.match(
			result.ok === true ? "" : result.message,
			/does not provide `events\.v1`/,
		);
		assert.deepEqual(
			fixture.requests.map((request) => request.method),
			["daemon.ping"],
		);
	} finally {
		cleanup(fixture);
	}
});

test("events decodes a page to the last delivered sequence, not the journal tail", async () => {
	const fixture = makeFixture((request) =>
		request.method === "daemon.ping"
			? reply(request, EVENTS_CONTRACTS)
			: reply(request, {
					epoch: "e1",
					events: [{ seq: 4, kind: "publication" }, { seq: 5, kind: "spawn" }],
					cursor: { epoch: "e1", sequence: 5 },
					tail: 9,
					more: true,
					earliest: 1,
				}),
	);
	try {
		await listen(fixture);
		const result = await fixture.client().events({ after: 3, wait_ms: 0 });
		assert.deepEqual(result, {
			ok: true,
			value: { kind: "page", epoch: "e1", sequence: 5, more: true },
		});
	} finally {
		cleanup(fixture);
	}
});

test("events decodes cursor_expired to the new epoch and the journal's cursor", async () => {
	const fixture = makeFixture((request) =>
		request.method === "daemon.ping"
			? reply(request, EVENTS_CONTRACTS)
			: reply(request, {
					cursor_expired: {
						epoch: "e2",
						cursor: { epoch: "e2", sequence: 12 },
						earliest: 10,
					},
				}),
	);
	try {
		await listen(fixture);
		const result = await fixture.client().events({ after: 3, epoch: "e1", wait_ms: 0 });
		assert.deepEqual(result, {
			ok: true,
			value: { kind: "expired", epoch: "e2", sequence: 12 },
		});
		const read = fixture.requests.find((request) => request.method === "events.read");
		assert.equal(read.params.epoch, "e1");
	} finally {
		cleanup(fixture);
	}
});

test("a malformed events reply is a protocol failure, never a silent page", async () => {
	const fixture = makeFixture((request) =>
		request.method === "daemon.ping"
			? reply(request, EVENTS_CONTRACTS)
			: reply(request, { epoch: "e1", events: "not-a-list", more: false }),
	);
	try {
		await listen(fixture);
		const result = await fixture.client().events({ after: 0, wait_ms: 0 });
		assert.equal(result.ok === true ? "" : result.code, "protocol");
	} finally {
		cleanup(fixture);
	}
});

test("watchRadarEvents wakes on a page and drains a backlog from the last delivered sequence", async () => {
	const requests: Array<{ after: number; wait_ms: number }> = [];
	const pages = [
		{ ok: true as const, value: { kind: "page" as const, epoch: "e1", sequence: 2, more: true } },
		{ ok: true as const, value: { kind: "page" as const, epoch: "e1", sequence: 3, more: false } },
	];
	const client: Pick<RadarClient, "events"> = {
		events: async (request) => {
			requests.push({ after: request.after, wait_ms: request.wait_ms });
			return pages.shift() ?? new Promise(() => {});
		},
	};
	const controller = new AbortController();
	let wakes = 0;
	const diagnoses: string[] = [];
	try {
		watchRadarEvents(client, controller.signal, {
			wake: () => (wakes += 1),
			diagnose: (message) => diagnoses.push(message),
		});
		await until(() => wakes === 2);
		assert.deepEqual(diagnoses, []);
		assert.equal(wakes, 2);
		assert.deepEqual(requests.slice(0, 3), [
			{ after: 0, wait_ms: 1000 },
			{ after: 2, wait_ms: 0 },
			{ after: 3, wait_ms: 1000 },
		]);
	} finally {
		controller.abort();
	}
});

test("a cursor_expired read wakes once and adopts the new epoch without replaying", async () => {
	const requests: Array<{ after: number; epoch?: string }> = [];
	const replies: Array<any> = [
		{ ok: true, value: { kind: "page", epoch: "e1", sequence: 4, more: false } },
		{ ok: true, value: { kind: "expired", epoch: "e2", sequence: 9 } },
	];
	const client: Pick<RadarClient, "events"> = {
		events: async (request) => {
			requests.push({ after: request.after, epoch: request.epoch });
			return replies.shift() ?? new Promise(() => {});
		},
	};
	const controller = new AbortController();
	let wakes = 0;
	const diagnoses: string[] = [];
	try {
		watchRadarEvents(client, controller.signal, {
			wake: () => (wakes += 1),
			diagnose: (message) => diagnoses.push(message),
		});
		await until(() => requests.length === 3);
		assert.deepEqual(diagnoses, []);
		assert.equal(wakes, 2);
		assert.deepEqual(requests[2], { after: 9, epoch: "e2" });
	} finally {
		controller.abort();
	}
});

test("a refused events contract warns once per outage, then once more after it is restored and lost again", async () => {
	// Three refusals in one outage, a restored contract, then a second outage.
	const replies: Array<any> = [
		{ ok: false, code: "unsupported", message: "no events.v1" },
		{ ok: false, code: "unsupported", message: "no events.v1" },
		{ ok: false, code: "unsupported", message: "no events.v1" },
		{ ok: true, value: { kind: "page", epoch: "e1", sequence: 0, more: false } },
		{ ok: false, code: "unsupported", message: "no events.v1" },
	];
	let calls = 0;
	const client: Pick<RadarClient, "events"> = {
		events: async () => {
			calls += 1;
			return replies.shift() ?? new Promise(() => {});
		},
	};
	const controller = new AbortController();
	let wakes = 0;
	const diagnoses: string[] = [];
	try {
		watchRadarEvents(client, controller.signal, {
			wake: () => (wakes += 1),
			diagnose: (message) => diagnoses.push(message),
		});
		await until(() => calls === 6, 6_000);
		assert.equal(calls, 6);
		assert.deepEqual(diagnoses, ["no events.v1", "no events.v1"]);
		assert.equal(wakes, 0);
	} finally {
		controller.abort();
	}
});

test("a page after cursor_expired delivers its wake once and reads from the new cursor", async () => {
	const requests: Array<{ after: number; epoch?: string }> = [];
	const replies: Array<any> = [
		{ ok: true, value: { kind: "page", epoch: "e1", sequence: 4, more: false } },
		{ ok: true, value: { kind: "expired", epoch: "e2", sequence: 9 } },
		{ ok: true, value: { kind: "page", epoch: "e2", sequence: 11, more: false } },
	];
	const client: Pick<RadarClient, "events"> = {
		events: async (request) => {
			requests.push({ after: request.after, epoch: request.epoch });
			return replies.shift() ?? new Promise(() => {});
		},
	};
	const controller = new AbortController();
	let wakes = 0;
	try {
		watchRadarEvents(client, controller.signal, {
			wake: () => (wakes += 1),
			diagnose: () => assert.fail("no diagnostic expected"),
		});
		await until(() => requests.length === 4);
		assert.equal(wakes, 3, "one wake per reply: page, resync, page");
		assert.deepEqual(requests[2], { after: 9, epoch: "e2" });
		assert.deepEqual(requests[3], { after: 11, epoch: "e2" });
	} finally {
		controller.abort();
	}
});

test("a throwing wake is diagnosed once per run of failures and never ends the watcher", async () => {
	const pages = [1, 2, 3, 4, 5].map((sequence) => ({
		ok: true as const,
		value: { kind: "page" as const, epoch: "e1", sequence, more: false },
	}));
	const client: Pick<RadarClient, "events"> = {
		events: async () => pages.shift() ?? new Promise(() => {}),
	};
	// Fails, fails, recovers, fails, recovers: two runs of failures.
	const outcomes = ["throw", "throw", "ok", "throw", "ok"];
	let wakeCalls = 0;
	const diagnoses: string[] = [];
	const controller = new AbortController();
	try {
		watchRadarEvents(client, controller.signal, {
			wake: () => {
				const outcome = outcomes[wakeCalls++];
				if (outcome === "throw") throw new Error("boom");
			},
			diagnose: (message) => diagnoses.push(message),
		});
		await until(() => wakeCalls === 5);
		assert.equal(wakeCalls, 5, "the loop read on past every throw");
		assert.deepEqual(diagnoses, ["event wake failed: Error: boom", "event wake failed: Error: boom"]);
	} finally {
		controller.abort();
	}
});

test("a rejected events read is diagnosed once and does not escape as an unhandled rejection", async () => {
	const client: Pick<RadarClient, "events"> = {
		events: async () => {
			throw new Error("dial failed");
		},
	};
	const controller = new AbortController();
	const diagnoses: string[] = [];
	try {
		watchRadarEvents(client, controller.signal, {
			wake: () => assert.fail("a failed read must not wake"),
			diagnose: (message) => diagnoses.push(message),
		});
		await until(() => diagnoses.length === 1);
		assert.deepEqual(diagnoses, ["event watch stopped: Error: dial failed"]);
	} finally {
		controller.abort();
	}
});

test("a throwing diagnose is contained: the refusal does not end the watcher", async () => {
	const replies: Array<any> = [
		{ ok: false, code: "unsupported", message: "no events.v1" },
		{ ok: true, value: { kind: "page", epoch: "e1", sequence: 1, more: false } },
	];
	const client: Pick<RadarClient, "events"> = {
		events: async () => replies.shift() ?? new Promise(() => {}),
	};
	const controller = new AbortController();
	let wakes = 0;
	try {
		watchRadarEvents(client, controller.signal, {
			wake: () => (wakes += 1),
			diagnose: () => {
				throw new Error("stale extension context");
			},
		});
		await until(() => wakes === 1, 3_000);
		assert.equal(wakes, 1, "the page after the refusal still wakes");
	} finally {
		controller.abort();
	}
});
