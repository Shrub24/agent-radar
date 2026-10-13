# managed-topology verification

Section 1 (container records, bindings and reads) is implemented. Task 2.3 landed
with it: the spawn path now records the pane slot it launched into.

## What is implemented

**The log is daemon-wide, not lifecycle-specific.** `src/control_plane/lifecycle/events.rs`
moved to `src/control_plane/events.rs` and its on-disk directory from
`lifecycle-events/` to `events/`, because containers are the second domain in it.
Container transitions (`ContainerCreated`, `ContainerObserved`, `ContainerRemoved`)
reduce into their own projection and replay on reopen; the delivery checkpoint
field became `source_seq`, since it now covers both domains. Container transitions
reach subscribers as `container.created`, `container.observed` and
`container.removed`.

**The record** (`src/control_plane/containers.rs`) carries a durable id, kind
(workspace, tab, pane), parent link, label, provenance (created by Radar, or
observed) and a binding kept strictly separate from identity: the backend, the
verified process incarnation serving the handle, and the raw handle. The
incarnation is a `ProcessIdentity` — the same evidence the spawn edge and the pane
close already use — because a declared server name cannot tell a restarted tmux
server from the original, so a reused handle would have been called present. An
observation writes `observed_at` and a source-labelled `unresolved`; it never
rewrites a binding, adopts a handle or deletes a record.

**Creation records.** `mux.create` records the container named by the request's own
kind before the creation is reported — the other identities a runtime names are
context, and the parent link is resolved only against a container this daemon
already records, otherwise it stays absent. `spawn.create` records the pane it
dispatched into (2.3).

**Reads.** `topology.get.v1` and `topology.list.v1` are registry-class reads served
without a backend (paginated, `has_more`/`next_after`). `topology.reconcile.v1`
requires `mux.observe`: it writes one observation per recorded container in a
bounded pass of 128, keeps every record, and dispatches nothing — no close, reload
or creation can come from a reconciliation. It refuses when no inventory can be
read, rather than recording that the whole fleet is missing.

## Evidence

- `cargo fmt --check` and `cargo clippy --locked --all-targets -- -D warnings`: clean.
- `cargo test --locked`: every binary green, including 405 library tests and 119
  control-plane socket tests.
- `openspec validate --all --strict`: 21/21, 0 failed.
- Mutation checks: recording the wrong identity for a create, dropping the
  incarnation fence, and treating any candidate handle as present each fail their
  test, and the restored code passes.

## Section 2 (partial): creation

`topology.create` exists as a daemon operation and contract (`topology.create.v1`,
needs `mux.create`). It shares one body with `mux.create`, so a creation is
validated once, its parent is resolved against containers the daemon already
records, the container is recorded before the creation is reported, and only the
operation a caller is recorded under differs. What it adds over the primitive: a
label bounded at 128 characters and refused if it carries control characters, an
owned provenance, and an answer that names the container it made.

A replayed request id whose method, target, requester and params match is answered
from the record; for `topology.create` that answer carries the same container, and
no other operation's answer gained a field. A container records `created_by`, the
operation that made it.

Deliberately not in this slice: the launch half of the recipe — working directory,
command, environment and backend options. A creation carrying a command, and the
reload that pairs with it, stay open and are recorded as such in `tasks.md`.

Evidence: `cargo fmt --check` and strict Clippy clean; the full locked suite green,
with 122 control-plane socket tests, including a creation this backend cannot serve
being refused before any effect, a label refused at the boundary without reaching
the backend, and a replayed creation dispatching nothing (the fake backend's create
log stays at one call). `openspec validate --all --strict`: 21/21.

## Section 5.1 (partial): the tree is named by containers

Containers reach the tree through one read. The dashboard client reads them
beside the agent facts and the spawn topology (`RegistryRead { agents, topology,
containers }`), from `topology.list`, only when the daemon advertises
`topology.list.v1`: a daemon that predates containers reads as no containers, and
the refresh does not fail. `FleetObservation` carries them to the client side — a
backend inventory never does, and Herdr and tmux report none — and the collector
fills them from the same read as the agent facts.

`FleetTree::build` then composes its group labels from the recorded containers
first and the backend's own labels second. A workspace or tab Radar created is
titled with the name it was created under; a workspace Radar never created keeps
the backend's name exactly as before. A container the daemon removed names
nothing, because its handle may belong to a later backend instance that is not
the container recorded under it — the same generation fence as pane close, and
naming a live group from a removed record would be inventing identity from a
reused handle.

Evidence: `cargo fmt --check` and strict Clippy clean; the full locked suite
green, with 406 library tests and 123 control-plane socket tests. New coverage: a
tree test for all three label cases (unrecorded → the backend's name, recorded →
ours, removed → the backend's name), and a socket test that the dashboard client
receives the container through the registry read rather than a second protocol.
`openspec validate --all --strict`: 21/21.

## Limits

- A container whose creation established no verified serving incarnation is
  recorded and never called present. Every backend that reports no server process —
  Herdr and the test backends — lands in that state.
- Container bindings are recorded from the daemon's own create paths. A container
  Radar never created (a pane the operator made in tmux) is not a container record
  yet; `fleet-overview`'s unmanaged rows and section 5's tree are where that
  becomes visible.
- Containers reach the tree but do not yet drive it. Rows still carry the
  backend's own identifiers rather than a container identity and binding, so
  section 5.2's targets have nothing to name; roots and nesting still follow the
  backend's groups, with containers naming them; and a recorded container the
  backend no longer reports is neither shown nor resolvable from the tree.
- Deletion and reload are not implemented. The TUI can display the hierarchy the
  daemon records, and still writes control through the owner file.
- The contract table in `docs/control-plane.md` (its sample handshake line and its
  contract list) and the contract discussion in `docs/agent-registration.md` are
  stale until the documentation section lands.
