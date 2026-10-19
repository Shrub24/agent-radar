import { homedir } from "node:os";
import { join } from "node:path";
import { createRadarClient } from "./radar-client.ts";
import { createRadarHandoffPort } from "./radar-communication.ts";
import {
	createRadarExecutionAdapter,
	type RadarExecutionEventApi,
} from "./radar-execution.ts";
import { resolveRadarProcessLocation } from "./radar-location.ts";
import {
	lazyRadarRuntime,
	provideRadarRuntime,
	type RadarExtensionEventBus,
} from "./radar-runtime.ts";

/** Minimal structural Pi API used by the baseline extension. */
type PiRadarHost = RadarExecutionEventApi & {
	events: RadarExtensionEventBus;
};

/**
 * Register pi-radar as the one baseline registrar for this Pi process.
 * Herdsman integrations request the same session-scoped runtime handle over
 * `pi.events`; they add orchestration data without creating another publisher.
 */
export default function registerPiRadar(pi: PiRadarHost): void {
	const client = createRadarClient();
	const execution = createRadarExecutionAdapter(pi, {
		client,
		dataRoot: join(homedir(), ".pi", "agent", "pi-radar"),
		location: () => resolveRadarProcessLocation(client),
	});
	const runtime = lazyRadarRuntime(async () => ({
		publication: execution.publication,
		communication: createRadarHandoffPort(client, execution.publication),
		execution,
	}));
	provideRadarRuntime(pi.events, runtime);
}
