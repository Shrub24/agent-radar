import assert from "node:assert/strict";
import { chmodSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { createServer, createConnection, type Socket } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { createRadarAttention } from "./radar-attention.ts";
import { createRadarClient } from "./radar-client.ts";

const REGISTRATION = "11111111-1111-4111-8111-111111111111";
const OPEN_TIMEOUT_MS = 300;

/** An attention frame: revision, then pending. */
function frame(revision: string, pending: number): Buffer {
	const text = (value: string): number[] => [0xa0 + value.length, ...Buffer.from(value)];
	const body = Buffer.from([
		0x82, ...text("revision"), ...text(revision), ...text("pending"), pending,
	]);
	const head = Buffer.alloc(4);
	head.writeUInt32BE(body.length);
	return Buffer.concat([head, body]);
}

type Behaviour = "baseline-then-drop" | "baseline-holds" | "drop-before-baseline" | "silent";

function daemon(behaviour: Behaviour) {
	const root = mkdtempSync(join(tmpdir(), "radar-watch-test-"));
	const directory = join(root, "socket");
	mkdirSync(directory, { mode: 0o700 });
	chmodSync(directory, 0o700);
	const path = join(directory, "control.sock");
	const opened: Socket[] = [];
	const server = createServer((socket) => {
		let buffered = "";
		socket.on("data", (chunk) => {
			buffered += chunk.toString("utf8");
			const end = buffered.indexOf("\n");
			if (end < 0) return;
			const request = JSON.parse(buffered.slice(0, end));
			if (request.method === "daemon.ping") {
				socket.end(`${JSON.stringify({ id: request.id, result: {
					protocol: 2, backend: "tmux", instance: null, revision: 12,
					contracts: ["communication.messages.v1"], tasks: { available: false, error: null },
				} })}\n`);
				return;
			}
			opened.push(socket);
			if (behaviour === "silent") return;
			if (behaviour === "drop-before-baseline") return socket.destroy();
			socket.write(`${JSON.stringify({ id: request.id, result: {} })}\n`);
			socket.write(frame("r1", 0));
			if (behaviour === "baseline-then-drop") setTimeout(() => socket.destroy(), 20);
		});
	});
	return {
		path,
		opened,
		async listen() {
			await new Promise<void>((ready) => server.listen(path, ready));
		},
		client() {
			return createRadarClient({
				env: { RADAR_CONTROL_SOCKET: path },
				uid: process.getuid?.() ?? 0,
				dial: (socketPath: string) => createConnection(socketPath),
				timeoutMs: OPEN_TIMEOUT_MS,
			} as Parameters<typeof createRadarClient>[0]);
		},
		close() {
			for (const socket of opened) socket.destroy();
			server.close();
			rmSync(root, { recursive: true, force: true });
		},
	};
}

const settle = async (ms = 200): Promise<void> => {
	await new Promise((resolve) => setTimeout(resolve, ms));
};

test("a subscription the daemon ends is reported to its consumer", async () => {
	const fixture = daemon("baseline-then-drop");
	await fixture.listen();
	try {
		const client = fixture.client();
		let ended = 0;
		const opened = await client.watchAttention({ registration_id: REGISTRATION, onAttention: () => {}, onEnd: () => { ended += 1; } });
		assert.equal(opened.ok, true, JSON.stringify(opened));
		await settle();
		assert.equal(ended, 1, "a dead subscription is news, not a live handle");
	} finally {
		fixture.close();
	}
});

test("a subscription the consumer closes is not reported as ended", async () => {
	const fixture = daemon("baseline-holds");
	await fixture.listen();
	try {
		const client = fixture.client();
		let ended = 0;
		const opened = await client.watchAttention({ registration_id: REGISTRATION, onAttention: () => {}, onEnd: () => { ended += 1; } });
		assert.equal(opened.ok, true);
		if (opened.ok) opened.value.close();
		await settle();
		assert.equal(ended, 0, "closing is not a failure to reconnect");
	} finally {
		fixture.close();
	}
});

test("a subscription that ends before its baseline fails instead of succeeding", async () => {
	const fixture = daemon("drop-before-baseline");
	await fixture.listen();
	try {
		const client = fixture.client();
		const opened = await client.watchAttention({ registration_id: REGISTRATION, onAttention: () => {} });
		assert.equal(opened.ok, false, "a handle with no baseline is not usable");
	} finally {
		fixture.close();
	}
});

test("a subscription that never reaches a baseline times out", async () => {
	const fixture = daemon("silent");
	await fixture.listen();
	try {
		const client = fixture.client();
		const opened = await client.watchAttention({ registration_id: REGISTRATION, onAttention: () => {} });
		assert.equal(opened.ok, false);
		assert.equal(opened.ok ? "" : opened.code, "timeout");
	} finally {
		fixture.close();
	}
});

test("repeated activity never opens a second subscription", async () => {
	const fixture = daemon("baseline-holds");
	await fixture.listen();
	try {
		const client = fixture.client();
		const loop = createRadarAttention({
			mailbox: { page: async () => ({ ok: true, value: { message_ids: [], next: null } }), get: async () => ({ ok: true, value: undefined }), receive: async () => ({ ok: true, value: undefined }), publish: async () => ({ ok: false, code: "refused", message: "unused" }) } as never,
			consumers: [],
			scheduler: { setTimeout, clearTimeout, now: Date.now } as never,
			activity: () => "working",
			watch: (onAttention, onEnd) => client.watchAttention({ registration_id: REGISTRATION, onAttention, onEnd }),
			backoffMs: 50,
		});
		loop.activity();
		loop.activity();
		loop.activity();
		await settle(60);
		assert.equal(fixture.opened.length, 1, "one subscription per process");
		loop.stop();
	} finally {
		fixture.close();
	}
});

test("a subscription that ends is reopened from a fresh baseline", async () => {
	const fixture = daemon("baseline-then-drop");
	await fixture.listen();
	try {
		const client = fixture.client();
		const loop = createRadarAttention({
			mailbox: { page: async () => ({ ok: true, value: { message_ids: [], next: null } }), get: async () => ({ ok: true, value: undefined }), receive: async () => ({ ok: true, value: undefined }), publish: async () => ({ ok: false, code: "refused", message: "unused" }) } as never,
			consumers: [],
			scheduler: { setTimeout, clearTimeout, now: Date.now } as never,
			activity: () => "working",
			watch: (onAttention, onEnd) => client.watchAttention({ registration_id: REGISTRATION, onAttention, onEnd }),
			backoffMs: 20,
		});
		loop.activity();
		await settle(400);
		assert.ok(fixture.opened.length >= 2, `reopened after the socket died (saw ${fixture.opened.length})`);
		loop.stop();
	} finally {
		fixture.close();
	}
});
