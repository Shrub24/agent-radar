import type {
	ObservationGetParamsDeserialize,
	ObservationGetResponseSerialize,
	ObservationSnapshot,
	SpawnListParamsDeserialize,
	SpawnListPageSerialize,
} from "./generated-contracts.js";
import type {
	RadarClient,
	RadarResult,
	RadarFactsSnapshot,
	RadarMessage,
	RadarMessagePage,
	RadarMessageReceipt,
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
const observationParams: ObservationGetParamsDeserialize = {
	epoch: "epoch-1",
	since_revision: 1,
	wait_ms: 30_000,
	classes: ["fleet"],
};
const observationSnapshot: ObservationSnapshot = {
	epoch: "epoch-1",
	revision: 1,
	collected_at: null,
	sources: [],
	tasks: null,
	observation: {
		inventory: null,
		source: "pending",
		registry: "pending",
		retained: {},
		containment: {},
		awaiting_evidence: [],
		foreground: {},
	},
};
const observationResponse: ObservationGetResponseSerialize = {
	epoch: observationSnapshot.epoch,
	revision: observationSnapshot.revision,
	snapshot: observationSnapshot,
};
const message: RadarMessage = {
	version: 1,
	id: "22222222-2222-4222-8222-222222222222",
	producer: {
		registration_id: "33333333-3333-4333-8333-333333333333",
		session_id: "44444444-4444-4444-8444-444444444444",
		subject_id: "55555555-5555-4555-8555-555555555555",
		claim_id: "66666666-6666-4666-8666-666666666666",
	},
	recipients: ["11111111-1111-4111-8111-111111111111"],
	body: { kind: "handoff", summary: "worker complete", content: "full result" },
	created_at_unix_ms: 1_800_000_000_000,
};
const page: RadarMessagePage = { message_ids: [message.id], next: null };
const receipt: RadarMessageReceipt = {
	version: 1,
	message_id: message.id,
	recipient: message.producer,
	received_at_unix_ms: 1_800_000_000_100,
};

void [
	handshake,
	facts,
	exited,
	unknown,
	observationParams,
	observationResponse,
	message,
	page,
	receipt,
];

function spawnListApi(client: RadarClient, params: SpawnListParamsDeserialize) {
	const page: Promise<RadarResult<SpawnListPageSerialize>> = client.spawnList({ after: params.after, limit: params.limit });
	// @ts-expect-error Caller identity comes from process registration.
	client.spawnList({ registration_id: "caller" });
	// @ts-expect-error Parent identity cannot be chosen by callers.
	client.spawnList({ parent_session_id: "parent" });
	return page;
}
void spawnListApi;
