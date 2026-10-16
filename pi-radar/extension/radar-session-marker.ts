/**
 * Radar-issued identity carried as a non-context custom entry in the active Pi
 * session branch. It is a recovery breadcrumb, not an authority to create or
 * mutate a Radar subject.
 */
import type { LaunchIdentityMarkerSerialize } from "./generated-contracts.ts";

export const RADAR_LAUNCH_MARKER_TYPE = "pi-radar.launch-identity";
export const RADAR_LAUNCH_MARKER_VERSION = 1 as const;
export const RADAR_MAX_EXECUTION_GENERATION = 0xffff_ffff;

export type RadarLaunchMarker = Omit<LaunchIdentityMarkerSerialize, "version"> & {
	version: typeof RADAR_LAUNCH_MARKER_VERSION;
};

/** Structural subset of Pi's custom SessionEntry used by this codec. */
export type CustomSessionEntry = {
	type: string;
	customType?: string;
	data?: unknown;
};

/** pi.appendEntry-compatible callback; no Pi runtime import is needed here. */
export type AppendSessionEntry = (customType: string, data?: unknown) => void;
export type PiSessionMarkerHost = {
	appendEntry?: AppendSessionEntry;
	sessionManager?: { getBranch(): readonly CustomSessionEntry[] };
};

export const RADAR_MANAGED_SUBJECT_ENV = "PI_RADAR_MANAGED_SUBJECT_ID";
export const RADAR_MANAGED_LAUNCH_ENV = "PI_RADAR_MANAGED_LAUNCH_ID";
export const RADAR_MANAGED_GENERATION_ENV = "PI_RADAR_MANAGED_EXECUTION_GENERATION";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/iu;

/** Validates the exact versioned marker shape before writing or accepting it. */
export function decodeRadarLaunchMarker(value: unknown): RadarLaunchMarker | undefined {
	if (typeof value !== "object" || value === null || Array.isArray(value)) return undefined;
	const record = value as Record<string, unknown>;
	if (
		Object.keys(record).sort().join(",") !==
		"execution_generation,launch_id,subject_id,version"
	) {
		return undefined;
	}
	if (
		record.version !== RADAR_LAUNCH_MARKER_VERSION ||
		typeof record.subject_id !== "string" ||
		!UUID.test(record.subject_id) ||
		typeof record.launch_id !== "string" ||
		!UUID.test(record.launch_id) ||
		!Number.isSafeInteger(record.execution_generation) ||
		(record.execution_generation as number) < 1 ||
		(record.execution_generation as number) > RADAR_MAX_EXECUTION_GENERATION
	) {
		return undefined;
	}
	return {
		version: RADAR_LAUNCH_MARKER_VERSION,
		subject_id: record.subject_id,
		launch_id: record.launch_id,
		execution_generation: record.execution_generation as number,
	};
}

/** Appends a valid Radar-issued marker as a Pi custom session entry. */
export function appendRadarLaunchMarker(
	appendEntry: AppendSessionEntry,
	marker: RadarLaunchMarker,
): void {
	const validated = decodeRadarLaunchMarker(marker);
	if (!validated) throw new Error("invalid Radar launch identity marker");
	appendEntry(RADAR_LAUNCH_MARKER_TYPE, validated);
}

/**
 * Reads the newest marker on Pi's current branch. A malformed newest marker is
 * returned as unresolved rather than silently falling back to an older launch.
 */
export function latestRadarLaunchMarker(
	branch: readonly CustomSessionEntry[],
): { status: "absent" } | { status: "valid"; marker: RadarLaunchMarker } | { status: "invalid" } {
	let latest: CustomSessionEntry | undefined;
	for (const entry of branch) {
		if (entry.type === "custom" && entry.customType === RADAR_LAUNCH_MARKER_TYPE) {
			latest = entry;
		}
	}
	if (!latest) return { status: "absent" };
	const marker = decodeRadarLaunchMarker(latest.data);
	return marker ? { status: "valid", marker } : { status: "invalid" };
}

/**
 * Persist this daemon-injected process identity into the active Pi session branch.
 * No marker is written when any variable is absent or malformed. Repeated session
 * starts in the same generation do not append duplicate entries.
 */
export function radarLaunchMarkerFromEnvironment(
	env: Record<string, string | undefined> = process.env,
): RadarLaunchMarker | undefined {
	const rawGeneration = env[RADAR_MANAGED_GENERATION_ENV];
	if (
		env[RADAR_MANAGED_SUBJECT_ENV] === undefined ||
		env[RADAR_MANAGED_LAUNCH_ENV] === undefined ||
		rawGeneration === undefined ||
		!/^[1-9][0-9]*$/.test(rawGeneration)
	) {
		return undefined;
	}
	return decodeRadarLaunchMarker({
		version: RADAR_LAUNCH_MARKER_VERSION,
		subject_id: env[RADAR_MANAGED_SUBJECT_ENV],
		launch_id: env[RADAR_MANAGED_LAUNCH_ENV],
		execution_generation: Number(rawGeneration),
	});
}

export function activeRadarLaunchMarker(
	host: PiSessionMarkerHost,
): ReturnType<typeof latestRadarLaunchMarker> {
	if (!host.sessionManager) return { status: "absent" };
	return latestRadarLaunchMarker(host.sessionManager.getBranch());
}

export function persistRadarLaunchMarker(
	host: PiSessionMarkerHost,
	env: Record<string, string | undefined> = process.env,
): "absent" | "unsupported" | "unchanged" | "written" {
	const marker = radarLaunchMarkerFromEnvironment(env);
	if (!marker) return "absent";
	if (!host.appendEntry || !host.sessionManager) return "unsupported";
	const current = latestRadarLaunchMarker(host.sessionManager.getBranch());
	if (current.status === "valid" && sameMarker(current.marker, marker)) return "unchanged";
	appendRadarLaunchMarker(host.appendEntry, marker);
	return "written";
}

function sameMarker(a: RadarLaunchMarker, b: RadarLaunchMarker): boolean {
	return (
		a.version === b.version &&
		a.subject_id === b.subject_id &&
		a.launch_id === b.launch_id &&
		a.execution_generation === b.execution_generation
	);
}
