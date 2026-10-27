# Proposal

## Why

The TUI receives canonical navigation through a push subscription but fetches observation facts independently, allowing a published row projection to outrun the facts used to explain it. Publish one coherent head over three independently reusable payloads and remove the duplicate frontend delivery workflow, while establishing portable content and transport primitives for later consumers.

## What Changes

- **BREAKING**: replace TUI-specific JSON projection delivery with a bounded MessagePack publication/content path using XXH3-128 identities for observation, tree projection and UI-session state.
- Order complete heads with one actor publication counter; lane hashes identify content without per-lane version counters. Preserve attachment, action, interaction and domain-request identities for their separate purposes.
- Build reusable codec, immutable encoded-content, bounded content-store and frame I/O primitives; keep lane producers typed and avoid per-lane copies or an extensibility framework.
- Adopt heads coherently in the TUI, retaining only current payloads and a bounded replacement in progress; remove its independent observation-fetch thread/path.
- Preserve observation freshness, source failures and demand-driven optional process collection. Keep publication metadata outside content hashes and define stable encoding explicitly.
- Bound encoding, frames, retention and slow writers. Keep receipts/effects and gate invalidation independent of latest-state rendering.
- Record pi-radar/other-lane adoption as follow-on work in `plan.md`, not implementation scope here.

## Capabilities

### New Capabilities

- `tui-publications`: coherent three-payload heads, portable content identity, current-only consumer adoption and recovery.

### Modified Capabilities

- `ui-session`: supersede the predecessor's two-payload publication/baseline API after its capability baseline is synced.
- `daemon-transport`: bounded binary framing for the TUI publication/content path while other daemon and compatibility traffic retains its existing wire contract.
- `shared-observation`: allow TUI delivery through coherent publications without changing the existing observation read API or sampler authority.

## Impact

- Builds on `shared-ui-session`; its unsynced UI-session requirements must be reconciled before this change is synced or archived. This proposal replaces its two-payload JSON/BLAKE3 publication contract, not its command/admission semantics.
- Expected seams: `src/ui_session/{session,actor,client}.rs`, public UI DTOs, UI server/transport, observation-hub adapter, `src/{app,main,snapshot_fetch}.rs`, protocol fixtures and docs. Choose shared-module placement to fit existing ownership, not to shorten files.
- Dependencies: `rmp-serde` and `xxhash-rust` with XXH3 enabled. Reuse the existing Tokio runtime and dedicated synchronous UI actor; no framework or second runtime.
- No migration of pi-radar, event/ingest/presence lanes, generic domain RPCs or durable storage; no sampler redesign, smoothing, per-consumer filtered datasets, deltas, compression or chunking. No live terminal acceptance claimed by automated tests.
