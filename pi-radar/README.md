# pi-radar

`pi-radar` is the standalone Pi baseline registrar, daemon client and publication library for Radar's Unix-socket control plane. It is bundled as ESM JavaScript for runtime imports and includes TypeScript source for editor/type tooling. The package has no runtime dependencies.

```ts
import { client } from "pi-radar";

const radar = client.createRadarClient();
const handshake = await radar.ping();
```

Focused subpaths are also exported: `pi-radar/client`, `pi-radar/communication`, `pi-radar/execution`, `pi-radar/publication`, `pi-radar/runtime`, `pi-radar/launch`, `pi-radar/location`, and `pi-radar/inspection`. The shared `runtime` handle exposes `services.communication`, a handoff port bound to the baseline's existing client, this process's current registration, and the managed parent injected by Radar. Callers do not choose sender or recipient identity; `communication.createRadarHandoffPort` is the underlying SDK factory. The runtime subpath provides a versioned `pi.events` request/reply handoff so multiple extensions can consume one baseline publication without sharing bundled module singletons. Existing extension deep imports under `pi-radar/extension/` remain exported.

## Pi baseline extension

Installing `pi-radar` directly as a Pi package loads `extension/baseline.ts`, which creates one baseline publisher and exposes its lazy runtime handle through Pi's event bus. Herdsman must request that handle rather than starting another `RadarExecutionAdapter`. Do not load the baseline extension with an older Herdsman integration that still creates its own publisher; first migrate Herdsman to the runtime handle.

The baseline reports session presence through one flat request. The first managed `session_start` uses its pre-minted launch claim; subsequent session loads create fresh claims. Heartbeat/reconnect retries reuse the same claim. `publication.presence.management()` reports `managed`, `duplicate`, `pending` or `unmanaged`; `session_shutdown` awaits release of the exact claim. A duplicate remains rejected until a fresh Pi session load. `subject()` returns Radar's current subject only after a grant; no session marker is written to the Pi transcript.

## Daemon contract

**Managed execution:** a launch Radar admits arrives with `PI_RADAR_REGISTRATION_ID` in its environment. The baseline captures it, removes it from this process's environment before any child starts, and reports it as `registration_id`, so a managed launch binds the registration Radar already allocated instead of registering itself. A session claim is still fresh per load, and a reload or `/resume` takes a new one under the same registration; nothing about the process identity is presented or reused as a claim. `launch.admit` is the method `radar run` uses and is not part of the client surface. `agent.list` entries and observation agents carry `scoped`: `true` for a process Radar launched into a scope, `false` for an observed process it did not.

**0.7.0 upgrade:** update Radar (0.3.0) and pi-radar together; the floor is wire revision 6. A managed launch now carries `PI_RADAR_REGISTRATION_ID` instead of a claim id, and pi-radar always presents its own fresh claim. Nothing in your code changes unless you read the launch environment directly.

**0.6.0 upgrade:** update Radar and pi-radar together. Lifecycle controls require both `subject_id` and `claim_id`; handoff identities and spawn edges use `claim_id`, with no execution generation. Do not pass an agent's subject id where an attempt is required. Existing private launch records remain readable.

pi-radar 0.7.0 speaks protocol 2 and requires wire revision 6. Revision 6 gives a managed launch its process identity up front: Radar mints the `registration_id`, passes it as `PI_RADAR_REGISTRATION_ID`, and holds a `reserved` claim that grants nothing until the first verified report replaces it with the session's own claim, so a 0.6.0 client, which spends the spawn token as its first claim, cannot bind. Revision 4 added Pi presence (`agent.presence`/`agent.release`), which is how a session acquires its subject. Revision 5 names the agent `subject_id` and one attempt `claim_id`, drops the execution-generation ordinal, and requires lifecycle controls to name the claim they were issued against, so a revision-4 client would read those fields with the old meanings. Handoff methods additionally require the independently negotiated `communication.handoff.v1` contract; its timestamps remain exactly representable as JSON numbers. Clients that control managed executions must not target the legacy per-execution ID semantics. It reads `revision` and the public `contracts` list from `daemon.ping`, preserves unfamiliar contract names, and preflights only contracts needed for a multi-step or versioned operation. Backend `mux.*` primitives are private to the daemon; direct mux methods are dispatched and return the daemon's backend refusal when unavailable.

The client uses `registry.v1` for registry operations and additionally requires `registry.facts.v1` for the facts channel and assignment extensions. Managed children require `spawn.v1`, child-pane removal requires `spawn.close.v1`, event reads require `events.v1`, and handoff operations require `communication.handoff.v1`. The expected wire examples are maintained in this repository's `docs/control-plane.fixture.jsonl` and `docs/agent-registration.fixture.jsonl` and are exercised by the package tests.

`RadarFactsSnapshot` matches the daemon facts channel: optional model/provider/thinking/session name, `context_window: { tokens, window }`, typed `background_tasks: { id, phase }[]`, and `background_started_unix_ms`. Context counts and the timestamp are JSON numbers bounded by JavaScript's safe-integer limit. The old text `context_usage` field is not accepted; Rust retains cross-field validation such as a positive window and tokens not exceeding it.

## Durable handoffs

The handoff API stores immutable bounded content separately from the recipient receipt. `inbox()` returns pending IDs only; retrieve each payload with `get()` and persist it before `ack()`, so a batch cannot exceed the RPC response bound. A worker's first Herdsman handoff is addressed only to the stable `parent_subject_id` recorded by Radar for its accepted managed launch. The parent gets its stable subject ID from `RadarSubject` and supplies it as `RadarWorkerLaunch.parent.subject_id`; `launchRadarWorker` forwards it as `managed.parent_subject_id`, and Radar injects the accepted value into the worker as `PI_RADAR_MANAGED_PARENT_SUBJECT_ID`. The handoff port reads that process context; callers cannot choose a destination. The daemon checks it against the durable parent record. Do not substitute a registration ID, Pi session, owner-session field, pane, or inferred lineage; unmanaged/session routing is deferred. The owner must durably capture the payload locally before calling `ack`; acknowledgement does not settle Herdsman assignment state or authorize worker closure. Pending inbox scans recover work after daemon restart or missed pointer events, and acknowledged content remains retrievable during the retention window. See `docs/control-plane.md` for wire shapes, bounds and recovery semantics.

## Launching a worker

`launchRadarWorker` takes the parent from the caller's own `publication`, so a consumer passes the pane it splits and no registration or subject for the parent. The result names the child by its `subject_id`, the key that stays the same when the child restarts or resumes; keep that as the child's identifier. A received handoff carries its sender's `subject_id`; match children by it.

To start a released child again, pass `continues: subject_id`: the daemon takes the session from its own record, so no session id or path is sent, and it refuses a child that is not yours or is still running. `client.restart` replaces your own live child under a new program. Both need Radar 0.4.0 or newer.

## Addressing children by subject

The runtime's `children` service (`RADAR_RUNTIME_VERSION` 3) is the surface for everything after a launch. `list()` returns the caller's children as `{ subject_id, request_id, pane_id, state, exited }`; `observe`, `inspect`, `assignment` and `retire` take the subject. pi-radar resolves a subject to its newest bound launch on every call and never remembers the process, so a child replaced under the same subject is reached by the same key. A held assignment follows a replaced child: the next `list`, `observe` or `inspect` finds the new process and moves the assignment onto it, retiring it from the old one. Consumers need no registration or claim id.

## Generated protocol contracts

The selected Rust wire DTOs are the source for the draft 2020-12 schemas in `schema/public/v1/`, shipped and exported under `pi-radar/schema/`. They are this package's contract of record: a daemon change lands as a schema regeneration, and the method-to-schema index lives in `docs/agent-registration.md` in the Radar repository. Serialize and deserialize schemas are separate because Serde defaults, null handling and omission differ by direction. The SDK's ping, Pi presence report/answer/release, handoff inbox, publication-parameter, immutable handoff, receipt, managed-launch-metadata input, facts-input, spawn-exit, and observation request/snapshot/response types are generated into `extension/generated-contracts.ts`; do not edit that file by hand.

The public process identifier is `registration_id` — a clean rename from the pre-0.4 `agent_id` with no alias, because a mixed fleet would otherwise read a missing field as an absent identity. Durable agent identity is `subject_id`, which survives process replacement; a Pi session's subject is granted by `agent.presence` and released by `agent.release`. Breaking wire changes bump this package's minor version, so a consumer must move its pin and restart the daemon in one step.

From the repository checkout, run `npm run generate:contracts` in this directory to regenerate schemas and TypeScript declarations. `npm test` checks the generated artifacts are current, typechecks the public SDK aliases, validates canonical wire fixtures and integer bounds against the schemas, and runs the package tests. Generation uses the locked Schemars crate plus pinned `json-schema-to-typescript` and TypeScript development dependencies; it adds no runtime package dependency.

Lifecycle execution generations are bounded to `u32` and remain exactly representable as JavaScript numbers. The initial schema set leaves operation sequences and event cursors unchanged; their long-horizon representation remains a separate wire-compatibility decision.

Run `npm pack` to build the bundled runtime entrypoints before creating the tarball.

## Testing against this SDK

Fake the port, not the daemon. Every consumer-facing class here depends on a
narrow `Pick` of the client, so a test double implements a handful of methods and
no socket, framing, IDs, error envelope or durable state:

| Port | Methods | Where |
| --- | --- | --- |
| `RadarPublicationClient` | `ping`, `presence`, `releasePresence`, `acquire`, `publish`, `retire` | `pi-radar/publication` |
| `RadarHandoffPort` | `publish`, `inbox`, `get`, `ack` | `pi-radar/communication` |
| anything else | `Pick<RadarClient, ...>` of the reads you call | `pi-radar/client` |

```ts
import type { RadarPublicationClient } from "pi-radar/publication";
import { createRadarPublication } from "pi-radar/publication";

const client: RadarPublicationClient = {
	ping: async () => ({
		ok: true,
		value: {
			protocol: 2, backend: "tmux", revision: 4, instance: null,
			contracts: ["registry.v1"], tasks: { available: false, error: null },
		},
	}),
	presence: async (report) => ({
		ok: true,
		value: {
			registration_id: REGISTRATION,
			claim_id: report.claim_id,
			standing: "granted",
			subject_id: SUBJECT,
		},
	}),
	releasePresence: async (release) => ({
		ok: true,
		value: {
			registration_id: release.registration_id,
			claim_id: release.claim_id,
			standing: "released",
		},
	}),
	acquire: async () => ({
		ok: true,
		value: {
			handle: "writer", source: "consumer", incarnation: INCARNATION,
			generation: 1, sequence: 0,
		},
	}),
	publish: async ({ sequence }) => ({ ok: true, value: { sequence } }),
	retire: async () => ({ ok: true, value: undefined }),
};

const publication = createRadarPublication({
	dataRoot, client, registration: { source: "consumer", label: "lead" },
});
```

A socket-level fake daemon is a second implementation of Radar's protocol
maintained by someone who does not own it. It drifts silently: it keeps passing
while the real daemon refuses the same call, which is worse than having no fake
at all. That is not hypothetical — a consumer's fake still answered the retired
registration methods after the daemon had moved to presence, and its suite stayed
green through the whole contract change.

Wire-level proof belongs to one integration test against the real daemon (a
temporary socket path and state root are enough; `tests/agent_publisher.py` in
the Radar repository shows the shape), not to a fake every consumer re-implements.

## Releasing

Publishing is one tag push and it runs in CI; no npm token is stored anywhere.
`.github/workflows/publish-pi-radar.yml` publishes on a `pi-radar-vX.Y.Z` tag
push, authenticating with npm trusted publishing (OIDC) and emitting provenance.

One-time setup, on npmjs.com under the `pi-radar` package's settings: add a
trusted publisher of type GitHub Actions naming owner `Shrub24`, repository
`agent-radar`, workflow filename `publish-pi-radar.yml`, and no environment.
The workflow sets no `environment:`, so a non-empty value here stops it from
publishing; the same is true of an `NPM_TOKEN` secret, which trusted publishing
does not use.

To release:

1. Bump `version` in `package.json` and `package-lock.json` on `main` and push.
   Semantics: a breaking wire change bumps the minor, additive changes the patch.
2. Tag that commit and push the tag: `git tag pi-radar-vX.Y.Z && git push origin pi-radar-vX.Y.Z`.

The workflow refuses to publish unless the tag's version matches
`package.json`, the tagged commit is on `main`, and that version is not already
on npm; then it installs from the lockfile, runs `npm test`, preflights the
tarball with `npm pack --dry-run` (which builds `dist/` through `prepack`), and
publishes. A failed run is re-runnable by fixing the cause and re-pushing the
tag; an already-published version fails by design rather than republishing.
