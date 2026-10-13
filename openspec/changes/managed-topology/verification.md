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

## Deletion

`topology.delete` (`topology.delete.v1`, requiring `mux.close`) removes a
container and everything recorded inside it. The request names the container and
the place and occupant the operator confirmed; the daemon refuses when the
container is unknown, already removed, bound to another place, recorded under
another serving instance, or when a live managed worker is registered inside what
it would remove. That last refusal names each worker and the stop-then-delete
sequence, and nothing is dispatched.

The close itself is the same guarded dispatch `mux.close` runs — one fresh
inventory, the frozen identity, the registry — and the removals are recorded only
after the runtime confirms the close. A deletion claims no process exit, no
unregistration and no assignment outcome; an uncertain dispatch records the
uncertainty and leaves the container unremoved. The verdict is that a place Radar
owns can be removed by the operator who made it, without a live worker inside it
being ended silently, and without a reused handle being closed as if it were the
recorded container.

Evidence: a deletion closes the container's own place exactly once and records
each removal; a live registered worker inside the target is named and no close is
dispatched (mutation-checked: suppressing that refusal fails the test); a
container bound to another place is refused; a container recorded under another
serving instance is refused; an already-removed container is refused under a
fresh id; and a replayed request id answers its own record and dispatches nothing.

## Reload and the recorded recipe

`topology.create` accepts an optional `recipe` for a pane creation only. It is
preflighted against the backend before anything is created, held in the daemon's
owner-only store (`lifecycle/private/container-<id>.json`, mode 0600) before the
record that says so, and run in the created pane by the daemon itself with no
spawn token, so the child is not a spawn child. The record says only that a recipe
is held: the command, its working directory and its environment appear in no
record, read or event.

`topology.reload` (`topology.reload.v1`, requiring `mux.launch` and
`mux.exit_observation`) runs that recipe again in the same pane slot. It requires
a typed retained exit the backend reports now, and refuses when the command is
still running or its exit cannot be established — naming the stop-then-reload
sequence rather than signalling anything itself, which is how deletion treats a
live worker too. It records its own outcome, changes no container, and is not a Pi
session restart, a new container or an assignment fact.

Evidence: a recipe creation holds its command privately (asserted on the file's
mode and on the absence of the command and its environment from every read) and
runs it in the created pane with `spawn_token: None`; a recipe on a creation that
makes no pane is refused before any effect; a reload with no recipe, with a
command still running, and with an exit the backend cannot establish each refuses
without a second launch; a reload over a typed exit runs the recipe again in the
same pane and answers with the container unchanged.

## TUI control plane

A workspace or pane row now carries the container Radar records for its place
(`tree::ContainerRef`), and the details panel shows that identity beside the
location. `a`, `D` and `R` open the same confirmation dialog the lifecycle
actions use, frozen when it opens: the container id, its place and that place's
occupant. Cancel is the default, Confirm revalidates against the latest current
observation, and a container removed or rebound before Confirm refuses with
nothing sent.

A confirmed control is dispatched through `control_plane::client::ContainerControls`,
implemented by the dashboard client over the same socket and handshake as its
reads, one control at a time and off the UI thread. The request id is minted at
Confirm, carried in the request, and kept in the outcome line the footer shows, so
a lost answer leaves an operation to read back rather than an effect nobody can
name. A control the daemon does not advertise is refused before it is sent, a run
with no daemon dispatches nothing and says so, and nothing is removed from the
tree optimistically.

Evidence: app tests cover the container confirmation (frozen id, place and
occupant; Cancel by default; a removed container refused at confirm; a row with no
container refused before the dialog opens; a reload of a pane with no recipe
refused). Client-level socket tests dispatch each control under the caller's own
id, answer a replay with the same container and no second effect, and refuse a
control whose contract the daemon does not advertise before it is sent.

## Acceptance

Run on the committed tree (`87a82ec9`, `main`) with every gate in one pass:

- `cargo fmt --check`, strict Clippy (`-D warnings`) and `cargo build --locked
  --all-targets`: clean.
- `cargo test --locked --all-targets`: every suite green — 410 library tests and
  131 control-plane socket tests among them, with the container coverage named
  above.
- `openspec validate --all --strict`: 21/21.
- The publisher fixture smoke: a disposable daemon registering, acquiring,
  publishing, reading back, switching sessions, retiring and reading events, with
  replay, fencing, restart freshness and privacy.
- The PTY smoke: source failure/empty/recovery, a registered agent and its facts,
  the stub publisher, details focus and page cycling, a scroll, a disclosure
  opened and closed, process-row navigation, a folded branch, Escape back to the
  tree, filter entry, a stalled quit with a client connected, mouse capture taken
  and released, and terminal restoration.
- The disposable-tmux acceptance: two registrations joined a real tmux server
  through a disposable daemon, their execution/assignment/self facts kept their own
  values and freshness, an unjoinable registration stayed unresolved and silent, a
  finished execution did not complete the assignment, close was refused for both,
  Herdr was never called, and every resource was removed.
- The checked Nix build: `/nix/store/mww8blfbg9cqwy47mgq3fjakxvsby7fi-agent-radar-0.1.0`.

Two mutation checks confirm the new coverage is load-bearing: accepting a
`running` pane exit in the reload gate fails the reload test, and skipping the
serving-incarnation comparison in deletion fails the deletion-fence test; both
pass again when restored.

## Limits

- A container whose creation established no verified serving incarnation is
  recorded and never called present; deletion and reload of one refuse for the same
  reason. Every backend that reports no server process — Herdr and the test
  backends — lands in that state, so those two controls are tmux-shaped today.
- Container bindings are recorded from the daemon's own create paths. A pane the
  operator made outside Radar is not a container record, so it has no identity to
  act on and the TUI refuses it as "not a container Radar records".
- The tree has no tab row, so a tab container is shown as a row's tab label and
  its id in the details, and is not selectable: tab-level create and delete are
  deferred rather than guessed.
- Roots and nesting still follow the backend's groups, with containers naming
  them, rather than being built from the container parent links; and a recorded
  container the backend no longer reports is shown as an unresolved detail line
  rather than as a row of its own.
- The TUI creates an empty slot: it takes no recipe, so a created pane runs
  nothing until something is typed into it or a later slice adds the input field.
- The `spawn`-launched pane's own launch context is not the container recipe, so
  a spawned pane is not reloadable through this verb yet: its recipe lives on the
  edge.
- A reload's evidence is the pane's retained exit, not a tracked process. A
  container-launched command is not captured with a pidfd, so reload cannot stop
  one itself; the operator stops it first, exactly as deletion asks.
