# pi-radar

`pi-radar` is the standalone Pi baseline registrar, daemon client and publication library for Radar's Unix-socket control plane. It is bundled as ESM JavaScript for runtime imports and includes TypeScript source for editor/type tooling. The package has no runtime dependencies.

```ts
import { client } from "pi-radar";

const radar = client.createRadarClient();
const handshake = await radar.ping();
```

Focused subpaths are also exported: `pi-radar/client`, `pi-radar/communication`, `pi-radar/execution`, `pi-radar/publication`, `pi-radar/runtime`, `pi-radar/launch`, `pi-radar/location`, and `pi-radar/inspection`. The shared `runtime` handle exposes `services.communication`, a handoff port bound to the baseline's existing client, this process's current registration, and the managed parent injected by Radar. Callers do not choose sender or recipient identity; `communication.createRadarHandoffPort` is the underlying SDK factory. The runtime subpath provides a versioned `pi.events` request/reply handoff so multiple extensions can consume one baseline publication without sharing bundled module singletons. `extension/radar-session-marker.ts` provides the versioned Pi custom-session-entry codec; it is correlation evidence only, not lifecycle authority. Existing extension deep imports under `pi-radar/extension/` remain exported.

## Pi baseline extension

Installing `pi-radar` directly as a Pi package loads `extension/baseline.ts`, which creates one baseline publisher and exposes its lazy runtime handle through Pi's event bus. Herdsman must request that handle rather than starting another `RadarExecutionAdapter`. Do not load the baseline extension with an older Herdsman integration that still creates its own publisher; first migrate Herdsman to the runtime handle.

## Daemon contract

The client speaks protocol 2 and requires wire revision 3 or newer for its existing lifecycle operations. Revision 3 made lifecycle `subject_id` a stable logical identity and exposed the exact current `execution_id` and `u32` generation separately. Handoff methods additionally require the independently negotiated `communication.handoff.v1` contract introduced at wire revision 4; its timestamps remain exactly representable as JSON numbers. Clients that control managed executions must not target the legacy per-execution ID semantics. It reads `revision` and the public `contracts` list from `daemon.ping`, preserves unfamiliar contract names, and preflights only contracts needed for a multi-step or versioned operation. Backend `mux.*` primitives are private to the daemon; direct mux methods are dispatched and return the daemon's backend refusal when unavailable.

The client uses `registry.v1` for registry operations and additionally requires `registry.facts.v1` for the facts channel and assignment extensions. Managed children require `spawn.v1`, child-pane removal requires `spawn.close.v1`, event reads require `events.v1`, and handoff operations require `communication.handoff.v1`. The expected wire examples are maintained in this repository's `docs/control-plane.fixture.jsonl` and `docs/agent-registration.fixture.jsonl` and are exercised by the package tests.

`RadarFactsSnapshot` matches the daemon facts channel: optional model/provider/thinking/session name, `context_window: { tokens, window }`, typed `background_tasks: { id, phase }[]`, and `background_started_unix_ms`. Context counts and the timestamp are JSON numbers bounded by JavaScript's safe-integer limit. The old text `context_usage` field is not accepted; Rust retains cross-field validation such as a positive window and tokens not exceeding it.

## Durable handoffs

The handoff API stores immutable bounded content separately from the recipient receipt. `inbox()` returns pending IDs only; retrieve each payload with `get()` and persist it before `ack()`, so a batch cannot exceed the RPC response bound. A worker's first Herdsman handoff is addressed only to the stable `parent_subject_id` recorded by Radar for its accepted managed launch. The parent gets its stable subject ID from `RadarSubject` and supplies it as `RadarWorkerLaunch.parent.subject_id`; `launchRadarWorker` forwards it as `managed.parent_subject_id`, and Radar injects the accepted value into the worker as `PI_RADAR_MANAGED_PARENT_SUBJECT_ID`. The handoff port reads that process context; callers cannot choose a destination. The daemon checks it against the durable parent record. Do not substitute a registration ID, Pi session, owner-session field, pane, or inferred lineage; unmanaged/session routing is deferred. The owner must durably capture the payload locally before calling `ack`; acknowledgement does not settle Herdsman assignment state or authorize worker closure. Pending inbox scans recover work after daemon restart or missed pointer events, and acknowledged content remains retrievable during the retention window. See `docs/control-plane.md` for wire shapes, bounds and recovery semantics.

## Generated protocol contracts

The selected Rust wire DTOs are the source for the draft 2020-12 schemas in `schema/public/v1/`, shipped and exported under `pi-radar/schema/`. Serialize and deserialize schemas are separate because Serde defaults, null handling and omission differ by direction. The SDK's ping, handoff inbox, publication-parameter, immutable handoff, receipt, managed-launch-metadata input, facts-input, spawn-exit, and observation request/snapshot/response types are generated into `extension/generated-contracts.ts`; do not edit that file by hand.

From the repository checkout, run `npm run generate:contracts` in this directory to regenerate schemas and TypeScript declarations. `npm test` checks the generated artifacts are current, typechecks the public SDK aliases, validates canonical wire fixtures and integer bounds against the schemas, and runs the package tests. Generation uses the locked Schemars crate plus pinned `json-schema-to-typescript` and TypeScript development dependencies; it adds no runtime package dependency.

Lifecycle execution generations are bounded to `u32` and remain exactly representable as JavaScript numbers. The initial schema set leaves operation sequences and event cursors unchanged; their long-horizon representation remains a separate wire-compatibility decision.

Run `npm pack` to build the bundled runtime entrypoints before creating the tarball.
