import { test } from "node:test";
import assert from "node:assert/strict";
import { provedRead } from "./radar-inspection.ts";
import type { RadarAgentProcess, RadarResult } from "./radar-client.ts";

const REGISTERED = {
	claimed: true,
	verification: "verified",
	location: { backend: "tmux", pane: "tmux:server:%1" },
	contextSession: "lead-session",
} as RadarAgentProcess;

function clientReturning(read: RadarResult<RadarAgentProcess>) {
	return { agentGet: async () => read };
}

const target = {
	agentId: "agent-1",
	paneId: "tmux:server:%1",
	session: "lead-session",
};

test("a registration is proved only when verified on its pane and session", async () => {
	assert.deepEqual(
		await provedRead(clientReturning({ ok: true, value: REGISTERED }), target),
		REGISTERED,
	);
	for (const read of [
		{ ok: true, value: { ...REGISTERED, verification: "absent" } },
		{ ok: true, value: { ...REGISTERED, location: { backend: "tmux", pane: "tmux:server:%9" } } },
		{ ok: true, value: { ...REGISTERED, contextSession: "other-session" } },
		{ ok: true, value: { ...REGISTERED, contextSession: undefined } },
		{ ok: false, code: "unknown", message: "daemon unavailable" },
	] as RadarResult<RadarAgentProcess>[]) {
		assert.equal(await provedRead(clientReturning(read), target), undefined);
	}
});
