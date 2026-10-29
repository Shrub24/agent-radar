import type { AttentionSerialize } from "./generated-contracts.ts";
import type { RadarMailboxPort, RadarMessageConsumer } from "./radar-communication.ts";
import type { RadarScheduler, RadarTimer } from "./radar-client.ts";

/** How long a continuously idle agent keeps its subscription. */
export const RADAR_IDLE_HOLD_MS = 30_000;
/** Retry spacing after a refused or dropped subscription. */
export const RADAR_ATTENTION_BACKOFF_MS = 2_000;

/** Only `idle` starts the hold: waiting for work is not being idle. */
export type RadarActivity = "idle" | "working" | "waiting";

export type RadarAttentionOptions = {
	mailbox: RadarMailboxPort;
	consumers: RadarMessageConsumer[];
	scheduler: RadarScheduler;
	activity: () => RadarActivity;
	/** Present only for a process that opted into the push tier. */
	watch?: (
		onAttention: (attention: AttentionSerialize) => void,
		onEnd: () => void,
	) => Promise<
		{ ok: true; value: { close(): void } } | { ok: false; code: string; message: string }
	>;
	idleHoldMs?: number;
	backoffMs?: number;
};

export type RadarAttention = {
	/** Reconcile now, and establish the subscription if activity allows. */
	drain(): void;
	/** The heartbeat reply's hint, if the daemon supplied one. */
	hint(attention: AttentionSerialize | undefined): void;
	/** Report this process's activity so the push tier can follow it. */
	activity(): void;
	stop(): void;
};

/**
 * Every trigger — startup, a heartbeat hint, a pushed update — funnels into one
 * serialized drain with a rerun flag. A message is received only after its
 * consumer durably captured it, and an unchanged revision never suppresses a
 * retry of work that is still pending.
 */
export function createRadarAttention(options: RadarAttentionOptions): RadarAttention {
	const holdMs = options.idleHoldMs ?? RADAR_IDLE_HOLD_MS;
	const backoffMs = options.backoffMs ?? RADAR_ATTENTION_BACKOFF_MS;
	let stopped = false;
	let running = false;
	let again = false;
	let last = "";
	let watch: { close(): void } | undefined;
	let idleTimer: RadarTimer | undefined;
	let retryTimer: RadarTimer | undefined;

	function cancel(timer: RadarTimer | undefined): undefined {
		if (timer !== undefined) options.scheduler.clearTimeout(timer);
		return undefined;
	}

	function scheduleRetry(): void {
		if (stopped || retryTimer !== undefined) return;
		retryTimer = options.scheduler.setTimeout(() => {
			retryTimer = cancel(retryTimer);
			open();
		}, backoffMs);
		retryTimer.unref?.();
	}

	function closeWatch(): void {
		watch?.close();
		watch = undefined;
	}

	function holdWatch(): void {
		if (idleTimer !== undefined) return;
		idleTimer = options.scheduler.setTimeout(() => {
			idleTimer = cancel(idleTimer);
			if (options.activity() === "idle") closeWatch();
		}, holdMs);
		idleTimer.unref?.();
	}

	function open(): void {
		if (stopped || watch !== undefined || options.watch === undefined) return;
		if (options.activity() === "idle") {
			holdWatch();
			return;
		}
		idleTimer = cancel(idleTimer);
		const start = options.watch;
		const ended = (): void => {
			if (stopped) return;
			watch = undefined;
			scheduleRetry();
		};
		void start(hint, ended).then((opened) => {
			// Refused or lost: heartbeats still deliver, so this is a retry, not an
			// incident.
			if (opened.ok === false) {
				if (!stopped) scheduleRetry();
				return;
			}
			if (stopped || options.activity() === "idle") {
				opened.value.close();
				return;
			}
			watch = opened.value;
		});
	}

	async function pass(): Promise<void> {
		let after: string | undefined;
		for (;;) {
			const page = await options.mailbox.page(after);
			if (page.ok === false) return;
			for (const id of page.value.message_ids) {
				const fetched = await options.mailbox.get(id);
				if (fetched.ok === false) return;
				const consumer = options.consumers.find((one) => one.kind === fetched.value.message.body.kind);
				// An unsupported body stays pending and visible; it is never
				// dropped to keep the loop moving.
				if (consumer === undefined) continue;
				if (!(await consumer.capture(fetched.value.message))) return;
				const received = await options.mailbox.receive(id);
				// A failed receipt means redelivery later, which the consumer's own
				// deduplication makes safe.
				if (received.ok === false) return;
			}
			if (page.value.next === undefined || page.value.next === null) return;
			after = page.value.next;
		}
	}

	function drain(): void {
		if (stopped) return;
		if (running) {
			again = true;
			return;
		}
		running = true;
		void pass()
			.catch(() => undefined)
			.finally(() => {
				running = false;
				if (again) {
					again = false;
					drain();
				}
			});
	}

	function hint(attention: AttentionSerialize | undefined): void {
		if (stopped) return;
		if (attention !== undefined) {
			if (attention.pending === 0 && attention.revision === last) return;
			last = attention.revision;
		}
		drain();
	}

	return {
		drain() {
			open();
			drain();
		},
		hint,
		activity() {
			if (options.activity() === "idle") holdWatch();
			else {
				idleTimer = cancel(idleTimer);
				open();
			}
		},
		stop() {
			stopped = true;
			idleTimer = cancel(idleTimer);
			retryTimer = cancel(retryTimer);
			closeWatch();
		},
	};
}
