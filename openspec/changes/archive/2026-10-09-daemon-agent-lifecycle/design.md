# Design

## Context

See proposal.md for motivation. `src/control_plane/server.rs::execute_report` currently dispatches to `RuntimeProvider::report`; `HerdrRuntime::report` maps directly to Herdr's pane/session/metadata methods. Its durable operation record is a receipt, not a current agent registry. The new layer belongs alongside that store, not inside the Herdr adapter.

Radar already has boot-id/PID/start-tick identity for sampled processes. Herdsman's assignment and mailbox state is independent of its Herdr adapter; its public projection is computed before metadata publication. Those are reusable producer facts, not something the coordinator needs to recreate. pi-subagents has been removed and is excluded.

## Goals / Non-Goals

**Goals:** a durable, backend-independent registration and direct-report contract ready for Herdsman adoption; honest freshness and writer identity; explicit private launch specifications for later execution.

**Non-Goals:** moving assignment algorithms or mailbox ownership, implementing launch/stop/resume, enabling new lifecycle keys, TUI integration, replacing the background-task bus, tmux, OSC parsing, or silently importing Herdr agent tokens into the registry.

## Decisions

### 1. A registry beside the physical runtime adapter

Add a small `control_plane::registry` module for records, validation and persistence. Proposed additive methods are `agent.register`, `agent.publish`, `agent.retire`, `agent.get` and `agent.list`. Advertise a distinct `agent_registry` capability; no backend is required. Keep the current `report` method unchanged so old clients do not accidentally switch semantics.

Reject storing registrations inside `HerdrRuntime`: that would keep the state backend-dependent. Reject extending the one-way task bus to execute lifecycle controls: it has a different ownership and consumption contract.

### 2. Incarnation is not session identity

The durable key is a daemon-issued agent-record UUID. A registration identifies publisher source plus publisher-incarnation UUID, session UUID, and optional verified `ProcessIdentity`. Owner/run/label links and a `{backend, instance, workspace, tab, pane}` location are optional, separately labelled facts. The same session may have multiple live records. Unknown process identity is allowed for observation but cannot become execution eligibility.

Registration retry for the same publisher incarnation and identical content returns the existing handle. Conflicting identity content refuses. The handle is a fencing credential, not authentication against hostile code running as the same Unix user. A fresh writer cannot be taken over implicitly; retirement/expiry and replacement are explicit transitions, persisted atomically.

Reject session UUID as the key because double attaches share it. Reject pane ID as identity because it is backend-local and reusable.

### 3. Complete channel snapshots with freshness

`agent.publish` replaces one source channel's complete snapshot, not arbitrary patches. Execution and assignment channels remain separate; a producer can publish its own facts, while owner projections identify the reporting owner. Store daemon receipt time, publisher observation time, sequence and a bounded lease (default 30 seconds, range 1–300 seconds). Only daemon time determines lease expiry. Equal-sequence identical retry does not refresh the lease; a heartbeat is a newer sequence.

Unknown state/reason/action names survive as strings; field shapes are strict and bounded. Proposed maximums: 64 KiB per registry record, 1 KiB per display field, 16 advertised actions, 256 argv entries within the record bound, and 100 entries per list page. Page by stable record UUID with an explicit continuation cursor. Do not reuse mux target-lane suppression: telemetry must remain writable after an uncertain physical mutation.

Reject the four-word Herdr state enum here: the new model must carry waiting reasons and outcomes without encoding them as tokens.

### 4. Persistent history, explicit reconnect

Use the existing trusted state root, a separate private registry subdirectory, atomic 0600 records and bounded non-symlink reads. Registry writes complete before acknowledgment. On daemon startup, restored snapshots are stale even when their old timestamps appear recent. Reconnecting publishers refresh their own handle/sequence; expired handles cannot replace a newer generation. Corrupt records cause explicit read/admission failure, not a truncated successful fleet.

A stale lease is not exit evidence. In this change process verification uses local birth identity through an injectable verifier, outside the registry lock. Report it as verified, absent, mismatched or unavailable. It does not automatically close, delete or restart records.

### 5. Explicit launch information, no execution yet

An optional private launch specification holds an absolute executable path, literal argv, absolute cwd, explicit session UUID/path, producer identity and revision. No shell string, implicit environment capture, prompt extraction or launch-command inference. Environment customization and executor policy are deferred to the execution change. Public get/list responses expose availability and revision only, never argv or session paths; owner-side access for an executor is not exposed in this foundation.

Reject `/proc` argv as a resume recipe: wrappers, prompts and incomplete launch configuration make it unreliable. Missing launch data is acceptable and honestly disables future capability.

### 6. Publish the seam before porting Herdsman

This repository delivers endpoints, registry snapshots and an extension-facing contract/example. pi-extensions then publishes its existing assignment projection and child/lead registration directly. Optional Herdr mirroring is a producer compatibility concern. Once that is verified, a separate change adds physical launch/stop/resume using the registered representation and Herdsman-provided eligibility/reconciliation.

No new authority framework: Pi provides execution evidence, Herdsman provides assignment authority, coordinator provides physical observations/controls. Registration alone promises neither safe restart nor recovery of children.

## Risks / Trade-offs

- Stored launch arguments may be sensitive → private records and redacted public reads; document same-user trust limits.
- Daemon/publisher restart can revive stale reports → startup freshness reset, sequence fencing and explicit writer replacement.
- Two reporting paths can disagree → retain source labels; direct registry is not silently merged into current TUI tokens in this slice.
- Registry can grow indefinitely → bound each record/read and expose explicit retirement; retention/compaction policy follows measured use rather than deleting history silently.
- Upstream publisher migration is separate → test the documented publisher example against the real socket without requiring a live Herdsman process.

## Migration Plan

1. Implement registry and additive endpoints with no changes to current mux methods.
2. Publish fixtures and a reconnecting publisher example, independently verify persistence and failures.
3. Port Herdsman advertising in pi-extensions; confirm no Herdr dependency for registry publication.
4. Add daemon lifecycle execution and owner-coordinated recovery in subsequent changes.

Rollback: producers return to existing Herdr metadata reporting; existing mux methods and managed owner controls remain available. Registry records remain private and inert; rollback never triggers a launch.
