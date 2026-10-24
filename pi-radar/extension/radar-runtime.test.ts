import assert from "node:assert/strict";
import test from "node:test";
import {
	RADAR_RUNTIME_VERSION,
	lazyRadarRuntime,
	provideRadarRuntime,
	requestRadarRuntime,
	type RadarExtensionEventBus,
	type RadarRuntimeHandle,
	type RadarRuntimeServices,
} from "./radar-runtime.ts";

class Events implements RadarExtensionEventBus {
	private readonly listeners = new Map<string, Array<(data: unknown) => void>>();

	emit(channel: string, data: unknown): void {
		for (const handler of this.listeners.get(channel) ?? []) handler(data);
	}

	on(channel: string, handler: (data: unknown) => void): () => void {
		const handlers = this.listeners.get(channel) ?? [];
		handlers.push(handler);
		this.listeners.set(channel, handlers);
		return () => this.listeners.set(channel, handlers.filter((entry) => entry !== handler));
	}
}

function runtime(_label: string): RadarRuntimeHandle {
	const publication = {
		execution: {},
		context: {},
		facts: {},
		assignment: () => ({}),
		binding: () => undefined,
		subject: async () => undefined,
		retireAssignment: () => undefined,
		diagnostic: () => undefined,
		stop: () => undefined,
		reopen: () => undefined,
	} as unknown as RadarRuntimeServices["publication"];
	const services: RadarRuntimeServices = {
		publication,
		communication: {} as RadarRuntimeServices["communication"],
		children: {} as RadarRuntimeServices["children"],
		execution: {
			publication,
			binding: () => undefined,
			blocked: () => false,
			stop: () => undefined,
		} as RadarRuntimeServices["execution"],
	};
	return lazyRadarRuntime(async () => services);
}

test("independent consumers receive the same baseline runtime, not new publishers", async () => {
	const events = new Events();
	const baseline = runtime("baseline");
	const remove = provideRadarRuntime(events, baseline);

	const first = requestRadarRuntime(events);
	const second = requestRadarRuntime(events);
	assert.deepEqual(first, { ok: true, handle: baseline });
	assert.deepEqual(second, { ok: true, handle: baseline });
	assert.equal(first.ok && second.ok && first.handle === second.handle, true);
	if (!first.ok || !second.ok) throw new Error("expected runtime handles");
	const [firstServices, secondServices] = await Promise.all([
		first.handle.start(),
		second.handle.start(),
	]);
	assert.equal(firstServices, secondServices, "both extensions share one memoized start");
	remove();
	assert.deepEqual(requestRadarRuntime(events), { ok: false, reason: "missing" });
});

test("multiple baseline providers are reported instead of silently selecting one", () => {
	const events = new Events();
	const removeFirst = provideRadarRuntime(events, runtime("first"));
	const removeSecond = provideRadarRuntime(events, runtime("second"));
	assert.deepEqual(requestRadarRuntime(events), { ok: false, reason: "duplicate" });
	removeFirst();
	removeSecond();
});

test("the handoff ignores incompatible request versions and malformed handles", () => {
	const events = new Events();
	provideRadarRuntime(events, runtime("baseline"));
	let offered = 0;
	// The version a consumer offers must match this runtime's own.
	events.emit("pi-radar/runtime-request", {
		version: RADAR_RUNTIME_VERSION,
		provide: () => offered++,
	});
	events.emit("pi-radar/runtime-request", {
		version: 1,
		provide: () => offered++,
	});
	assert.equal(offered, 1);

	const invalidEvents = new Events();
	invalidEvents.on("pi-radar/runtime-request", (request) => {
		(request as { provide(handle: unknown): void }).provide({ version: 1 });
	});
	assert.deepEqual(requestRadarRuntime(invalidEvents), { ok: false, reason: "missing" });
});
