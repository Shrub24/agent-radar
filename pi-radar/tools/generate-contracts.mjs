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
		...["subject-get-params", "subject-child-get-params", "subject-child-close-params", "subject-get-result", "subject-list-params", "subject-children-params", "subject-list-result"].flatMap((name) =>
			[`${name}.serialize.schema.json`, `${name}.deserialize.schema.json`]),
	"presence-report.serialize.schema.json",
	"presence-report.deserialize.schema.json",
	"presence-answer.serialize.schema.json",
	"presence-answer.deserialize.schema.json",
	"presence-release.serialize.schema.json",
	"presence-release.deserialize.schema.json",
		"daemon-ping.serialize.schema.json",
		"daemon-ping.deserialize.schema.json",
		"message-publish-params.serialize.schema.json",
		"message-publish-params.deserialize.schema.json",
		"message.serialize.schema.json",
		"message.deserialize.schema.json",
		"message-page.serialize.schema.json",
		"message-page.deserialize.schema.json",
		"message-receipt.serialize.schema.json",
		"message-receipt.deserialize.schema.json",
		"attention.serialize.schema.json",
		"attention.deserialize.schema.json",
		"managed-launch-metadata.serialize.schema.json",
		"managed-launch-metadata.deserialize.schema.json",
		"facts-snapshot.serialize.schema.json",
		"facts-snapshot.deserialize.schema.json",
		"spawn-exit-observation.serialize.schema.json",
		"spawn-exit-observation.deserialize.schema.json",
		"observation-get-params.serialize.schema.json",
		"observation-get-params.deserialize.schema.json",
		"observation-snapshot.serialize.schema.json",
		"observation-snapshot.deserialize.schema.json",
		"observation-get-response.serialize.schema.json",
		"observation-get-response.deserialize.schema.json",
	]) {
		writeOrCheck(
			join(schemaDirectory, file),
			`${readFileSync(join(temporarySchemas, file), "utf8").trimEnd()}\n`,
		);
	}

	const types = [
		["subject-get-params.deserialize.schema.json", "SubjectGetParams"],
		["subject-child-get-params.deserialize.schema.json", "SubjectChildGetParams"],
		["subject-child-close-params.deserialize.schema.json", "SubjectChildCloseParams"],
		["subject-get-result.serialize.schema.json", "SubjectGetResult"],
		["subject-list-params.deserialize.schema.json", "SubjectListParams"],
		["subject-children-params.deserialize.schema.json", "SubjectChildrenParams"],
		["subject-list-result.serialize.schema.json", "SubjectListResult"],
	["presence-report.deserialize.schema.json", "PresenceReport"],
	["presence-answer.serialize.schema.json", "PresenceAnswer"],
	["presence-release.deserialize.schema.json", "PresenceRelease"],
		["daemon-ping.serialize.schema.json", "DaemonPingResult"],
		["message-publish-params.deserialize.schema.json", "MessagePublishParamsDeserialize"],
		["message-page.serialize.schema.json", "MessagePageSerialize"],
		["message.serialize.schema.json", "MessageSerialize"],
		["message-receipt.serialize.schema.json", "MessageReceiptSerialize"],
		["attention.serialize.schema.json", "AttentionSerialize"],
		["managed-launch-metadata.deserialize.schema.json", "ManagedLaunchMetadataDeserialize"],
		["facts-snapshot.deserialize.schema.json", "FactsSnapshotInput"],
		["spawn-exit-observation.serialize.schema.json", "SpawnExitObservation"],
		["observation-get-params.deserialize.schema.json", "ObservationGetParamsDeserialize"],
		["observation-get-response.serialize.schema.json", "ObservationGetResponseSerialize"],
		["spawn-list-params.deserialize.schema.json", "SpawnListParamsDeserialize"],
		["spawn-list-page.serialize.schema.json", "SpawnListPageSerialize"],
	];
	let declarations = "// Generated from the checked-in Rust-derived JSON Schemas. Do not edit.\n\n";
	for (const [file, typeName] of types) {
		let schema = JSON.parse(readFileSync(join(temporarySchemas, file), "utf8"));
		// Some DTOs share internal definition names with others (a page names the
		// freshness and state vocabularies observation also names); namespace their
		// generated declarations rather than emitting a duplicate identifier.
		if (file.startsWith("subject-")) schema.title = typeName;
		if (file.startsWith("presence-") || file.startsWith("spawn-") || file.startsWith("subject-")) {
			for (const name of Object.keys(schema.$defs ?? {})) {
				const qualified = `${typeName}${name}`;
				schema.$defs[qualified] = schema.$defs[name];
				delete schema.$defs[name];
				if (schema.$defs[qualified].title) schema.$defs[qualified].title = qualified;
				schema = JSON.parse(JSON.stringify(schema).replaceAll(`\"#/$defs/${name}\"`, `\"#/$defs/${qualified}\"`));
			}
		}
		declarations += await compile(schema, typeName, {
			bannerComment: "",
			style: { singleQuote: false, bracketSpacing: false },
		});
	}
	writeOrCheck(declarationsPath, declarations);
} finally {
	rmSync(temporaryRoot, { recursive: true, force: true });
}
