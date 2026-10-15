import {
	type RadarAgentProcess,
	type RadarClient,
	type RadarForeground,
} from "./radar-client.ts";

// The caps the removed Herdr inspection applied; evidence is returned, never
// the pane's full scrollback.
const INSPECTION_LINES = 80;
const INSPECTION_OUTPUT_BYTES = 16 * 1024;

export type RadarInspection = {
	capturedAt: number;
	recentOutputTruncated: boolean;
	recentOutput?: string;
	foreground?: RadarForeground;
	/** The child's published activity from the proof read; unknown when absent. */
	activity?: string;
};

/**
 * The registration's own read when the registered child `agentId` still runs on
 * `paneId` (and, when `session` is given, still publishes that session): a
 * `verified` verdict. Absent, mismatched, unavailable and unreadable are not
 * proof.
 */
export async function provedRead(
	client: Pick<RadarClient, "agentGet">,
	target: { agentId: string; paneId: string; session?: string },
): Promise<RadarAgentProcess | undefined> {
	const read = await client.agentGet(target.agentId);
	return read.ok === true &&
		read.value.verification === "verified" &&
		read.value.location?.pane === target.paneId &&
		(target.session === undefined ||
			read.value.contextSession === target.session)
		? read.value
		: undefined;
}

/**
 * Reads a managed child's recent output and foreground process, once the
 * registered child is proved on its pane before and after the read.
 */
export async function inspectRadarChild(
	client: RadarClient,
	target: { agentId: string; paneId: string; session?: string },
): Promise<RadarInspection> {
	const before = await provedRead(client, target);
	if (before === undefined)
		throw new Error("Inspection target identity did not match");
	const output = await client.output({
		pane_id: target.paneId,
		source: "recent_unwrapped",
		lines: INSPECTION_LINES,
	});
	if (output.ok === false)
		throw new Error(
			`Inspection output unavailable (${output.code}: ${output.message})`,
		);
	const foreground = await client.processInfo({ pane_id: target.paneId });
	const after = await provedRead(client, target);
	if (after === undefined)
		throw new Error("Inspection target changed during capture");
	const lines = output.value.text.trim().split(/\r?\n/).slice(-INSPECTION_LINES);
	const bounded = utf8Tail(lines.join("\n"), INSPECTION_OUTPUT_BYTES);
	return Object.freeze({
		capturedAt: Date.now(),
		recentOutputTruncated: output.value.truncated || bounded.truncated,
		...(bounded.text ? { recentOutput: bounded.text } : {}),
		...(foreground.ok === true ? { foreground: foreground.value } : {}),
		...(after.activity !== undefined ? { activity: after.activity } : {}),
	});
}

function utf8Tail(
	value: string,
	maxBytes: number,
): { text: string; truncated: boolean } {
	const bytes = Buffer.from(value);
	if (bytes.length <= maxBytes) return { text: value, truncated: false };
	let start = bytes.length - maxBytes;
	while (start < bytes.length && (bytes[start]! & 0xc0) === 0x80) start++;
	return { text: bytes.subarray(start).toString(), truncated: true };
}
