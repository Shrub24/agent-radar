import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { compile } from "json-schema-to-typescript";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repositoryRoot = resolve(packageRoot, "..");
const check = process.argv.includes("--check");
const temporaryRoot = mkdtempSync(join(tmpdir(), "radar-protocol-schemas-"));
const temporarySchemas = join(temporaryRoot, "schemas");
const schemaDirectory = join(packageRoot, "schema", "public", "v1");
const declarationsPath = join(packageRoot, "extension", "generated-contracts.ts");

function run(command, args, cwd) {
	const result = spawnSync(command, args, { cwd, encoding: "utf8", stdio: "inherit" });
	if (result.error) throw result.error;
	if (result.status !== 0) throw new Error(`${command} exited with ${result.status}`);
}

function writeOrCheck(path, contents) {
	if (check) {
		let existing;
		try {
			existing = readFileSync(path, "utf8");
		} catch {
			throw new Error(`missing generated contract: ${path}`);
		}
		if (existing !== contents) throw new Error(`generated contract is stale: ${path}`);
		return;
	}
	mkdirSync(dirname(path), { recursive: true });
	writeFileSync(path, contents);
}

try {
	run(
		"cargo",
		["run", "--locked", "--quiet", "--bin", "generate_protocol_schemas", "--", "--output", temporarySchemas],
		repositoryRoot,
	);

	for (const file of [
		"daemon-ping.serialize.schema.json",
		"daemon-ping.deserialize.schema.json",
		"managed-launch-metadata.serialize.schema.json",
		"managed-launch-metadata.deserialize.schema.json",
		"launch-identity-marker.serialize.schema.json",
		"launch-identity-marker.deserialize.schema.json",
		"facts-snapshot.serialize.schema.json",
		"facts-snapshot.deserialize.schema.json",
		"spawn-exit-observation.serialize.schema.json",
		"spawn-exit-observation.deserialize.schema.json",
	]) {
		writeOrCheck(
			join(schemaDirectory, file),
			`${readFileSync(join(temporarySchemas, file), "utf8").trimEnd()}\n`,
		);
	}

	const types = [
		["daemon-ping.serialize.schema.json", "DaemonPingResult"],
		["managed-launch-metadata.deserialize.schema.json", "ManagedLaunchMetadataDeserialize"],
		["launch-identity-marker.serialize.schema.json", "LaunchIdentityMarker"],
		["facts-snapshot.deserialize.schema.json", "FactsSnapshotInput"],
		["spawn-exit-observation.serialize.schema.json", "SpawnExitObservation"],
	];
	let declarations = "// Generated from the checked-in Rust-derived JSON Schemas. Do not edit.\n\n";
	for (const [file, typeName] of types) {
		const schema = JSON.parse(readFileSync(join(temporarySchemas, file), "utf8"));
		declarations += await compile(schema, typeName, {
			bannerComment: "",
			style: { singleQuote: false, bracketSpacing: false },
		});
	}
	writeOrCheck(declarationsPath, declarations);
} finally {
	rmSync(temporaryRoot, { recursive: true, force: true });
}
