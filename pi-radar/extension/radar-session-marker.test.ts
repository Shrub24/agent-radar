import assert from "node:assert/strict";
import test from "node:test";
import {
	appendRadarLaunchMarker,
	latestRadarLaunchMarker,
	persistRadarLaunchMarker,
	RADAR_LAUNCH_MARKER_TYPE,
	RADAR_MANAGED_GENERATION_ENV,
	RADAR_MANAGED_LAUNCH_ENV,
	RADAR_MANAGED_SUBJECT_ENV,
	type CustomSessionEntry,
	type RadarLaunchMarker,
} from "./radar-session-marker.ts";

const marker: RadarLaunchMarker = {
	version: 1,
	subject_id: "3d6f021b-4718-4a2e-a8c9-9920ff71af12",
	launch_id: "97403b5a-f470-4c80-bcb0-2bc84ad0e1c0",
	execution_generation: 2,
};

test("a Radar marker round-trips through Pi's non-context custom-entry shape", () => {
	const appended: Array<{ customType: string; data: unknown }> = [];
	appendRadarLaunchMarker((customType, data) => appended.push({ customType, data }), marker);

	// Pi persists appendEntry(customType, data) as a `custom` session entry.
	const branch: CustomSessionEntry[] = [
		{
			type: "custom",
			customType: appended[0].customType,
			data: appended[0].data,
		},
	];
	assert.equal(appended[0].customType, RADAR_LAUNCH_MARKER_TYPE);
	assert.deepEqual(latestRadarLaunchMarker(branch), { status: "valid", marker });
});

test("the latest marker wins on the active Pi session branch", () => {
	const prior = { ...marker, execution_generation: 1 };
	const branch: CustomSessionEntry[] = [
		{ type: "custom", customType: RADAR_LAUNCH_MARKER_TYPE, data: prior },
		{ type: "custom", customType: RADAR_LAUNCH_MARKER_TYPE, data: marker },
	];
	assert.deepEqual(latestRadarLaunchMarker(branch), { status: "valid", marker });
});

test("a malformed latest marker does not fall back to an older generation", () => {
	const branch: CustomSessionEntry[] = [
		{ type: "custom", customType: RADAR_LAUNCH_MARKER_TYPE, data: marker },
		{
			type: "custom",
			customType: RADAR_LAUNCH_MARKER_TYPE,
			data: { ...marker, execution_generation: 0 },
		},
	];
	assert.deepEqual(latestRadarLaunchMarker(branch), { status: "invalid" });
});

test("the process marker is persisted to the active session branch exactly once", () => {
	const branch: CustomSessionEntry[] = [];
	const env = {
		[RADAR_MANAGED_SUBJECT_ENV]: marker.subject_id,
		[RADAR_MANAGED_LAUNCH_ENV]: marker.launch_id,
		[RADAR_MANAGED_GENERATION_ENV]: String(marker.execution_generation),
	};
	const appended: Array<{ customType: string; data: unknown }> = [];
	const host = {
		appendEntry: (customType: string, data?: unknown) => {
			appended.push({ customType, data });
			branch.push({ type: "custom", customType, data });
		},
		sessionManager: { getBranch: () => branch },
	};
	assert.equal(persistRadarLaunchMarker(host, env), "written");
	assert.equal(persistRadarLaunchMarker(host, env), "unchanged");
	assert.equal(appended.length, 1);
	assert.deepEqual(latestRadarLaunchMarker(branch), { status: "valid", marker });
	assert.equal(persistRadarLaunchMarker(host, {}), "absent");
});

test("a breadcrumb rejects extra fields and generations outside the public u32 range", () => {
	for (const invalid of [
		{ ...marker, spawn_token: "must-not-persist" },
		{ ...marker, execution_generation: Number.MAX_SAFE_INTEGER + 1 },
		{ ...marker, execution_generation: 0x1_0000_0000 },
	]) {
		assert.throws(
			() => appendRadarLaunchMarker(() => {}, invalid as RadarLaunchMarker),
			/invalid Radar launch identity marker/u,
		);
	}
});
