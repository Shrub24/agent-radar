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

test("launch identity marker schema is versioned and JSON-safe", () => {
	const validate = ajv.compile(schema("launch-identity-marker.deserialize.schema.json"));
	assert.equal(
		validate({
			version: 1,
			subject_id: "11111111-1111-4111-8111-111111111111",
			launch_id: "22222222-2222-4222-8222-222222222222",
			execution_generation: 0xffff_ffff,
		}),
		true,
		ajv.errorsText(validate.errors),
	);
	assert.equal(
		validate({
			version: 1,
			subject_id: "11111111-1111-4111-8111-111111111111",
			launch_id: "22222222-2222-4222-8222-222222222222",
			execution_generation: 0x1_0000_0000,
		}),
		false,
	);
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
