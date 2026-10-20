import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import Ajv2020 from "ajv/dist/2020.js";
import { test } from "node:test";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repositoryRoot = resolve(packageRoot, "..");
const schemaDirectory = resolve(packageRoot, "schema/public/v1");
const MAX_SAFE_INTEGER = Number.MAX_SAFE_INTEGER;
const ajv = new Ajv2020({ strict: false, validateFormats: false });

function schema(name: string) {
	return JSON.parse(readFileSync(resolve(schemaDirectory, name), "utf8"));
}

function jsonLines(path: string): any[] {
	return readFileSync(path, "utf8")
		.split("\n")
		.filter((line) => line.trim() !== "")
		.map((line) => JSON.parse(line));
}

const controlPlane = jsonLines(resolve(repositoryRoot, "docs/control-plane.fixture.jsonl"));
const registrations = jsonLines(resolve(repositoryRoot, "docs/agent-registration.fixture.jsonl"));

function fixtureSnapshots() {
	return registrations.flatMap((line) =>
		line.kind === "request" &&
		line.method === "agent.publish" &&
		line.params?.channel === "facts"
			? [line.params.snapshot]
			: [],
	);
}

function spawnExitObservations() {
	return controlPlane.flatMap((line) => {
		const value = line.result?.spawn?.exit_observation;
		return value === undefined ? [] : [value];
	});
}

test("presence answers expose only the public registration and claim receipt", () => {
	const answer = schema("presence-answer.serialize.schema.json");
	assert.deepEqual(answer.required, ["registration_id", "claim_id", "standing"]);
	assert.deepEqual(Object.keys(answer.properties).sort(), [
		"claim_id", "registration_id", "standing", "subject_id",
	]);
});

test("the npm export exposes the portable daemon schema", () => {
	const exported = import.meta.resolve("pi-radar/schema/daemon-ping.serialize.schema.json");
	assert.deepEqual(JSON.parse(readFileSync(new URL(exported), "utf8")), schema("daemon-ping.serialize.schema.json"));
});

test("daemon ping schemas distinguish tolerated input defaults from emitted output fields", () => {
	const validates = ajv.compile(schema("daemon-ping.serialize.schema.json"));
	const accepts = ajv.compile(schema("daemon-ping.deserialize.schema.json"));
	const pingResults = controlPlane.flatMap((request, index) =>
		request.method === "daemon.ping" && controlPlane[index + 1]?.id === request.id
			? [controlPlane[index + 1].result]
			: [],
	);
	assert.ok(pingResults.length >= 2);
	for (const result of pingResults) {
		assert.equal(validates(result), true, ajv.errorsText(validates.errors));
	}
	const defaultedInput = {
		protocol: 2,
		revision: 1,
		backend: "tmux",
		contracts: ["registry.v1"],
		tasks: { available: false },
	};
	assert.equal(accepts(defaultedInput), true, ajv.errorsText(accepts.errors));
	assert.equal(validates(defaultedInput), false);
});

test("managed launch metadata schemas cover the typed spawn intent shape", () => {
	const input = ajv.compile(schema("managed-launch-metadata.deserialize.schema.json"));
	const output = ajv.compile(schema("managed-launch-metadata.serialize.schema.json"));
	const metadata = {
		owner: "herdr",
		run: "run-17",
		label: "reviewer",
		role: "worker",
		parent_subject_id: "11111111-1111-4111-8111-111111111111",
	};
	assert.equal(input(metadata), true, ajv.errorsText(input.errors));
	assert.equal(output(metadata), true, ajv.errorsText(output.errors));
	assert.equal(input({}), true);
	assert.equal(input({ owner: null }), true);
	assert.equal(input({ unknown: "field" }), false);
	assert.equal(input({ label: 42 }), false);
});

test("facts input/output schemas match omitted optionals and exact integer limits", () => {
	const input = ajv.compile(schema("facts-snapshot.deserialize.schema.json"));
	const output = ajv.compile(schema("facts-snapshot.serialize.schema.json"));
	const snapshots = fixtureSnapshots();
	assert.ok(snapshots.length > 0);
	for (const snapshot of snapshots) {
		assert.equal(input(snapshot), true, ajv.errorsText(input.errors));
		assert.equal(output(snapshot), true, ajv.errorsText(output.errors));
	}
	assert.equal(output({}), true, ajv.errorsText(output.errors));
	assert.equal(input({ model: null }), true);
	assert.equal(output({ model: null }), false);
	assert.equal(
		input({ context_window: { tokens: MAX_SAFE_INTEGER, window: MAX_SAFE_INTEGER } }),
		true,
	);
	assert.equal(
		input({ context_window: { tokens: MAX_SAFE_INTEGER + 1, window: MAX_SAFE_INTEGER + 1 } }),
		false,
	);
	// Cross-field relationships remain Rust domain validation, not JSON Schema.
	assert.equal(input({ context_window: { tokens: 2, window: 1 } }), true);
});

test("handoff schemas expose stable subject addressing and safe-range timestamps", () => {
	const handoff = ajv.compile(schema("handoff.serialize.schema.json"));
	const publishParams = ajv.compile(schema("handoff-publish-params.deserialize.schema.json"));
	const receipt = ajv.compile(schema("handoff-receipt.serialize.schema.json"));
	const value = {
		version: 1,
		id: "33333333-3333-4333-8333-333333333333",
		sender: {
			registration_id: "11111111-1111-4111-8111-111111111111",
			session_id: "22222222-2222-4222-8222-222222222222",
			subject_id: "44444444-4444-4444-8444-444444444444",
			execution_id: "55555555-5555-4555-8555-555555555555",
			execution_generation: 2,
		},
		recipient_subject_id: "66666666-6666-4666-8666-666666666666",
		correlation_id: "77777777-7777-4777-8777-777777777777",
		summary: "completed",
		content: "full handoff payload",
		published_at_unix_ms: MAX_SAFE_INTEGER,
	};
	assert.equal(handoff(value), true, ajv.errorsText(handoff.errors));
	const params = {
		sender_registration_id: value.sender.registration_id,
		recipient_subject_id: value.recipient_subject_id,
		correlation_id: value.correlation_id,
		summary: value.summary,
		content: value.content,
	};
	assert.equal(publishParams(params), true, ajv.errorsText(publishParams.errors));
	assert.equal(publishParams({ ...params, unexpected: true }), false);
	assert.equal(handoff({ ...value, published_at_unix_ms: MAX_SAFE_INTEGER + 1 }), false);
	assert.equal(handoff({ ...value, unknown: true }), false);
	assert.equal(receipt({
		version: 1,
		handoff_id: value.id,
		recipient: value.sender,
		received_at_unix_ms: MAX_SAFE_INTEGER,
	}), true, ajv.errorsText(receipt.errors));
});

test("spawn-exit schemas describe tagged output and optional input fields", () => {
	const validates = ajv.compile(schema("spawn-exit-observation.serialize.schema.json"));
	const accepts = ajv.compile(schema("spawn-exit-observation.deserialize.schema.json"));
	const observations = [...spawnExitObservations(), { state: "unknown" }];
	assert.ok(observations.some((observation) => observation.state === "exited"));
	assert.ok(observations.some((observation) => observation.state === "unknown"));
	for (const observation of observations)
		assert.equal(validates(observation), true, ajv.errorsText(validates.errors));
	const partialExitedInput = { state: "exited", observed_at: "2026-01-01T00:00:00.000Z" };
	assert.equal(accepts(partialExitedInput), true, ajv.errorsText(accepts.errors));
	assert.equal(validates(partialExitedInput), false);
});

test("observation.get schemas cover unchanged, expired, oversized, and snapshot responses", () => {
	const params = ajv.compile(schema("observation-get-params.deserialize.schema.json"));
	const snapshotInput = ajv.compile(schema("observation-snapshot.deserialize.schema.json"));
	const snapshotOutput = ajv.compile(schema("observation-snapshot.serialize.schema.json"));
	const response = ajv.compile(schema("observation-get-response.serialize.schema.json"));
	assert.equal(params({ epoch: "epoch-1", since_revision: 1, wait_ms: 30_000, classes: ["fleet"] }), true);
	assert.equal(params({ wait_ms: 30_001 }), false);
	const outcomes = [
		{ not_modified: { epoch: "epoch-1", revision: 1 } },
		{ snapshot_expired: { epoch: "epoch-1", revision: 1 } },
		{ snapshot_too_large: { epoch: "epoch-1", revision: 1, limit_bytes: 524_288 } },
		{
			epoch: "epoch-1",
			revision: 1,
			snapshot: {
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
			},
		},
	];
	for (const outcome of outcomes)
		assert.equal(response(outcome), true, ajv.errorsText(response.errors));
	const snapshot = outcomes[3].snapshot;
	assert.equal(snapshotInput(snapshot), true, ajv.errorsText(snapshotInput.errors));
	assert.equal(snapshotOutput(snapshot), true, ajv.errorsText(snapshotOutput.errors));
	assert.equal(
		response({ not_modified: { epoch: "epoch-1", revision: MAX_SAFE_INTEGER + 1 } }),
		false,
	);
	const exported = import.meta.resolve("pi-radar/schema/observation-get-response.serialize.schema.json");
	assert.deepEqual(
		JSON.parse(readFileSync(new URL(exported), "utf8")),
		schema("observation-get-response.serialize.schema.json"),
	);
});
