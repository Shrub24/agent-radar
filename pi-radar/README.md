# pi-radar

`pi-radar` is the TypeScript client and publication library for the Radar daemon. It is bundled as ESM JavaScript for runtime imports and includes TypeScript source for editor/type tooling. The package has no runtime dependencies.

```ts
import { client } from "pi-radar";

const radar = client.createRadarClient();
const handshake = await radar.ping();
```

Focused subpaths are also exported: `pi-radar/client`, `pi-radar/execution`, `pi-radar/publication`, `pi-radar/launch`, `pi-radar/location`, and `pi-radar/inspection`. Existing extension deep imports under `pi-radar/extension/` remain exported.

## Daemon contract

The client speaks protocol 2 and requires wire revision 1 or newer. It reads `revision` and the public `contracts` list from `daemon.ping`, preserves unfamiliar contract names, and preflights only contracts needed for a multi-step or versioned operation. Backend `mux.*` primitives are private to the daemon; direct mux methods are dispatched and return the daemon's backend refusal when unavailable.

The client uses `registry.v1` for registry operations and additionally requires `registry.facts.v1` for the facts channel and assignment extensions. Managed children require `spawn.v1`, child-pane removal requires `spawn.close.v1`, and event reads require `events.v1`. The expected wire examples are maintained in this repository's `docs/control-plane.fixture.jsonl` and `docs/agent-registration.fixture.jsonl` and are exercised by the package tests.

`RadarFactsSnapshot` matches the daemon facts channel: optional model/provider/thinking/session name, `context_window: { tokens, window }`, typed `background_tasks: { id, phase }[]`, and `background_started_unix_ms`. Token counts must be safe non-negative integers within a positive window; the old text `context_usage` field is not accepted.

Run `npm test` for the source suite. `npm pack` builds the bundled runtime entrypoints before creating the tarball.
