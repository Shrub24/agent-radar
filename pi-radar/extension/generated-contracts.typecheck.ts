import type {
	RadarFactsSnapshot,
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

void [handshake, facts, exited, unknown];
