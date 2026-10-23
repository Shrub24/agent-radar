import type { RadarExecutionAdapter } from "./radar-execution.ts";
import type { RadarChildren } from "./radar-children.ts";
import type { RadarHandoffPort } from "./radar-communication.ts";
import type { RadarPublication } from "./radar-publication.ts";

/** The event channel through which Pi extensions obtain the baseline runtime. */
export const RADAR_RUNTIME_REQUEST = "pi-radar/runtime-request";
export const RADAR_RUNTIME_VERSION = 4 as const;

/** The sole baseline publisher's session-scoped runtime, shared with consumers. */
export type RadarRuntimeServices = {
	/** The one baseline publication; Herdsman contributes through its channels. */
	publication: RadarPublication;
	/** Durable handoffs bound to this process's registration and managed parent. */
	communication: RadarHandoffPort;
	/** This process's own children, addressed by subject. */
	children: RadarChildren;
	execution: RadarExecutionAdapter;
};

/** Lazy so discovering the provider in an extension factory starts no resources. */
export type RadarRuntimeHandle = {
	version: typeof RADAR_RUNTIME_VERSION;
	/** Provider memoizes this promise, so every extension shares one publisher. */
	start(): Promise<RadarRuntimeServices>;
};

/** Builds one lazy, memoized session runtime without starting factory resources. */
export function lazyRadarRuntime(
	start: () => RadarRuntimeServices | Promise<RadarRuntimeServices>,
): RadarRuntimeHandle {
	let pending: Promise<RadarRuntimeServices> | undefined;
	return {
		version: RADAR_RUNTIME_VERSION,
		start: () => (pending ??= Promise.resolve().then(start)),
	};
}

/** Minimal structural view of Pi's shared extension EventBus. */
export type RadarExtensionEventBus = {
	emit(channel: string, data: unknown): void;
	on(channel: string, handler: (data: unknown) => void): () => void;
};

type RuntimeRequest = {
	version: typeof RADAR_RUNTIME_VERSION;
	provide(handle: unknown): void;
};

/**
 * Makes this extension's handle discoverable to other extensions in this Pi
 * runtime. Registering the listener is safe in a factory: it starts no resources.
 */
export function provideRadarRuntime(
	events: RadarExtensionEventBus,
	handle: RadarRuntimeHandle,
): () => void {
	return events.on(RADAR_RUNTIME_REQUEST, (value) => {
		if (!isRuntimeRequest(value)) return;
		value.provide(handle);
	});
}

/**
 * Finds the one baseline runtime in the current Pi process. Pi's event bus emits
 * synchronously; the reply callback therefore completes before this returns.
 * Zero or multiple providers are explicit errors, not invitations to register a
 * second baseline publisher.
 */
export function requestRadarRuntime(
	events: RadarExtensionEventBus,
): { ok: true; handle: RadarRuntimeHandle } | { ok: false; reason: "missing" | "duplicate" } {
	const handles: RadarRuntimeHandle[] = [];
	events.emit(RADAR_RUNTIME_REQUEST, {
		version: RADAR_RUNTIME_VERSION,
		provide: (candidate: unknown) => {
			if (isRadarRuntimeHandle(candidate)) handles.push(candidate);
		},
	} satisfies RuntimeRequest);
	if (handles.length === 1) return { ok: true, handle: handles[0] };
	return { ok: false, reason: handles.length === 0 ? "missing" : "duplicate" };
}

function isRuntimeRequest(value: unknown): value is RuntimeRequest {
	return (
		typeof value === "object" &&
		value !== null &&
		(value as RuntimeRequest).version === RADAR_RUNTIME_VERSION &&
		typeof (value as RuntimeRequest).provide === "function"
	);
}

function isRadarRuntimeHandle(value: unknown): value is RadarRuntimeHandle {
	if (typeof value !== "object" || value === null) return false;
	const handle = value as Partial<RadarRuntimeHandle>;
	return (
		handle.version === RADAR_RUNTIME_VERSION &&
		typeof handle.start === "function"
	);
}
