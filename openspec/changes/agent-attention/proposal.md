# Proposal

## Why

pi-radar dials a fresh connection per call, so an idle Pi process holds no socket, but it also learns nothing from the daemon between its own requests. Pending handoffs are found only by reading the inbox, and `watchRadarEvents` (a long-poll wake) has no caller. Making every live process hold a subscription would cost one socket per process while most are idle.

Each process already makes one daemon round trip per heartbeat (15 s, 30 s lease). That round trip can carry a small typed hint, so an idle agent keeps zero connections and still learns within one heartbeat that something needs it. A lead that is actively working on children can additionally hold a push connection, dropped after an idle hold and resumed from the hint's cursor.

## What Changes

- The heartbeat reply gains an optional, additive `attention` hint for the replying registration: pending handoff count and the current event journal position (epoch and sequence).
- A lead that is working or blocked MAY open a wake subscription over the existing trusted socket: JSON opener, then length-prefixed MessagePack, pushing the latest journal position whenever it advances. It is latest-wins; it carries no event bodies.
- pi-radar gains an attention loop: heartbeat hint drives reads; the opt-in push tier runs only while the process is working or blocked, drops after an idle hold, and resumes from the hint cursor.
- pi-radar adopts the MessagePack codec (`radar-msgpack.ts`, already landed) for this connection only. Every other call stays JSON.
- The TUI path, `events.read` and `observation.get` are unchanged.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `agent-registration`: heartbeat reply carries the attention hint.
- `daemon-transport`: a bounded wake-subscription opener on the shared socket.
- `pi-radar-sdk`: attention loop, idle hold, and codec use.

## Impact

`src/control_plane/registry/publication.rs` and its server handler (hint), `src/control_plane/transport` and a new small server module for the wake subscription (reusing `content.rs` framing, `Tasks`, class admission), `pi-radar/extension/{radar-publication,radar-client,radar-runtime}.ts`, generated contracts for the changed reply, `docs/control-plane.md`, `docs/agent-registration.md`. Depends on the merged typed-daemon-contracts change. No new dependency.
