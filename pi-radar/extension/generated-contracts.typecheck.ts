import type {
	RadarFactsSnapshot,
	RadarHandoff,
	RadarHandoffInbox,
	RadarHandoffReceipt,
	RadarHandshake,
	RadarSpawnExitObservation,
} from "./radar-client.ts";

const handshake: RadarHandshake = {
	protocol: 2,
	revision: 1,
	backend: "tmux",
	instance: null,
	contracts: ["registry.v1", "future.contract.v1"],
	tasks: { available: true, error: null },
};
const facts: RadarFactsSnapshot = {
	context_window: { tokens: 86_000, window: 200_000 },
	background_started_unix_ms: null,
	background_tasks: [{ id: "bg-1", phase: "running" }],
};
const exited: RadarSpawnExitObservation = {
	state: "exited",
	code: 23,
	signal: null,
	observed_at: "2026-01-01T00:00:00.000Z",
};
const unknown: RadarSpawnExitObservation = { state: "unknown" };
const handoff: RadarHandoff = {
	version: 1,
	id: "22222222-2222-4222-8222-222222222222",
	sender: {
		agent_id: "33333333-3333-4333-8333-333333333333",
		session_id: "44444444-4444-4444-8444-444444444444",
		subject_id: "55555555-5555-4555-8555-555555555555",
		execution_id: "66666666-6666-4666-8666-666666666666",
		execution_generation: 1,
	},
	recipient_subject_id: "11111111-1111-4111-8111-111111111111",
	summary: "worker complete",
	content: "full handoff",
	published_at_unix_ms: 1_800_000_000_000,
};
const inbox: RadarHandoffInbox = { handoff_ids: [handoff.id] };
const receipt: RadarHandoffReceipt = {
	version: 1,
	handoff_id: handoff.id,
	recipient: handoff.sender,
	received_at_unix_ms: 1_800_000_000_100,
};

void [handshake, facts, exited, unknown, handoff, inbox, receipt];
