# Design

## Context

See proposal.md. `agent-registration` (standing spec, archived from
`2026-10-09-daemon-agent-lifecycle`) keeps immutable subject records and
separate fenced execution/assignment channels. Registry reads are
`agent.get`/`agent.list` JSON responses assembled in
`src/control_plane/server.rs`. Herdsman verified register/acquire/publish/
retire/get/list 24/24 against an isolated daemon and publishes no session
field because none exists that may change.

## Goals / Non-Goals

**Goals:** a fenced mutable current-session UUID per agent record; clear
absent/stale semantics in reads; contract and example a publisher can adopt.

**Non-Goals:** immutable-registration relaxation, private session-path
publication, executor/resume behavior, daemon-side retention, push/events,
TUI wiring, inferred readiness, or any launch/stop/resume.

## Decisions

### 1. A separate context record, not a third channel or a snapshot field

The shared `Snapshot` shape exists for activity/waiting/outcome/actions, and
a session UUID is none of those. Forcing context through it would either
demand fake activity words or allow a session value to ride any channel,
leaving ownership unclear. A dedicated `SessionContext` record keeps each
record's meaning single. Proposed storage: one atomic file
`{agent_id}.context.json`, same `{version, agent_id, writer, generation,
sequence, lease_ms, received_at, expires_at, observed_at, retired_at}`
fencing envelope as the channel records plus `session: <uuid>|null`. First
publish binds the writer to the agent's own handle; replacement only after
retirement or lease expiry with an explicit publish from another registered
handle advancing the generation; equal-sequence identical replay is
idempotent and does not refresh the lease.

Reject a third `Channel` variant through `Snapshot`: it would carry
meaningless activity/outcome/action fields beside every session report.
Reject stuffing `session` into `Snapshot`: the value would be publishable on
any channel by any writer and could not be fenced as one fact.

### 2. Session UUID only, nothing private

The context record SHALL carry a canonical session UUID or an explicit null
(unassociated), and no path, argument, environment or raw error text. A null
session reports "no current session", while no record at all reports "never
published". Readers see the record presence, the value, its freshness and
its source; writers keep everything else where it already is (private
launch data, publisher internals).

Known consequence to document rather than fix here: a registration's private
launch session names the session at registration time, while context names
the live one. When they disagree the live one wins for any future resume
path, and today's daemon executes nothing either way.

### 3. Same aging rules, honest absence

Daemon receipt time and the serving epoch decide freshness exactly as for
channels; `observed_at` stays provenance and never extends a lease. Restored
records read stale after a daemon restart until republished. Stale or absent
context is unknown, never evidence of process exit, idleness or completion.

### 4. Additive wire, shared capability

`agent.context` params mirror the other writes minus the snapshot:
`{agent_id, writer, sequence, lease_ms?, observed_at?, context:
{session}}`. Reads extend `agent.get` and each `agent.list` entry with a
`"context"` key that is null when never published. Old clients ignore the
added key; a daemon without this build refuses `agent.context` as unknown.
Advertise nothing new in ping: `agent_registry` covers it.

## Risks / Trade-offs

- A publisher that republishes only on change leaves a long lease when the
  process dies unobserved → bounded leases still age it out; a stale lease
  is not exit evidence, only unreadability.
- Two writers racing one agent's context → incumbent rules plus explicit
  replacement decide; no last-writer-wins file overwrite.
- Future executors may read a lagging registration launch session → resolved
  for now by documenting that live context wins; the executor change enforces
  it when one exists.

## Migration Plan

1. Implement records and fencing (1.1), then the endpoint, reads, docs,
   fixtures and example (1.2), then the independent gate (1.3).
2. Pin the landed commit and send the method/fixture reference to
   pi-extensions; Herdsman publishes context on session_start/switch/fork
   with newer sequences under the same writer handling.
3. Leave registration, channels, retention and the mux paths untouched; the
   change rolls back by publishers simply not calling the new method.

Rollback: stop publishing context; existing registrations, channels and
managed controls are unaffected, and absent context reads as it did before.
