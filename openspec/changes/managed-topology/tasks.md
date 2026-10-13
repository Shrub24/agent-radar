# Tasks

## 1. Container records, bindings and reads

- [x] 1.1 Define the container record: durable identity, kind (workspace, tab, pane slot), parent link, label, provenance (created by Radar, or observed) and a separate backend binding (backend name, serving instance, raw handle), stored bounded and owner-only.
      Status: implemented in `src/control_plane/containers.rs`; the record is committed to the durable log as a `ContainerCreated`/`ContainerObserved`/`ContainerRemoved` transition.
- [x] 1.2 Record containers on the paths that create them, so a created workspace, tab or pane is owned before it is reported, and a created agent pane also carries the spawn edge that binds its child.
      Status: implemented for the `mux.create` path: only the identity the request asked for is recorded, the parent resolves to a recorded container or stays absent, and the container is appended before the creation is reported. An agent container's child binding is unchanged and still travels through the spawn contract.
- [x] 1.3 Add bounded paginated container reads carrying per-source freshness and the binding as evidence: missing, replaced, ambiguous or handle-reused bindings stay visible and unresolved.
      Status: implemented as `topology.get.v1` and `topology.list.v1` (`Method::Registry`, no backend needed). Each record carries `observed_at` as freshness, `unresolved` as a source-labelled reason and the binding as evidence.
- [x] 1.4 Add reconciliation that reports observed drift (missing, foreign, changed handle, replaced serving instance) as source-labelled facts without rewriting identity, adopting a foreign handle or deleting a record.
      Status: implemented as `topology.reconcile.v1` (needs `mux.observe`): one bounded pass writes one `ContainerObserved` fact per recorded container, keeps every record, and dispatches nothing. It refuses when no inventory can be read rather than recording that everything is missing.

## 2. Creation

- [x] 2.1 Add workspace, tab and pane creation from an explicit request naming the parent container and the recipe (label, working directory, command, environment, backend options).
      Status: partial. `topology.create` creates a workspace, tab or pane with a label, resolves its parent against recorded containers only, records the container before the creation is reported, and answers with that container (a replay of the same request id answers with the same one). The launch half of the recipe — working directory, command, environment and backend options — is not accepted yet; it lands with the tracked launch, so a creation carrying a command and the reload that pairs with it stay open.
- [x] 2.2 Keep recipe values private: bounded owner-only storage, and no recipe value in any read, log, diagnostic or event payload.
      Status: nothing to store yet. The label is bounded and control-character-free at the wire boundary; a working directory, command or environment value is not accepted at all, and no creation value reaches an event payload.
- [x] 2.3 Create agent containers through the existing spawn contract, so the child still binds to its exact `agent_id` and the container records the pane slot it was launched into.
      Status: implemented. `spawn.create` records the pane it dispatched into, bound to the verified serving incarnation, before the edge and the launch are recorded; the child binding is unchanged.
- [ ] 2.4 Refuse or leave unresolved rather than record a container whose creation could not be confirmed, and never retry an uncertain creation under a new request ID.

## 3. Deletion

- [ ] 3.1 Add container deletion as a durable operation with confirmation naming the exact container and the possible losses, a verified binding, and an outcome recorded before the caller is answered.
- [ ] 3.2 Refuse deletion that would end a live managed process, naming that process, and let the operator run the recorded stop-then-delete sequence, with pane removal and process exit reported separately.
- [ ] 3.3 Refuse a reused handle under a different serving instance, and keep deletion from claiming any process exited or any assignment completed.
- [ ] 3.4 Make replay of the same request ID report the recorded outcome without dispatching a second close.

## 4. Per-pane reload

- [ ] 4.1 Reload one pane slot from its recorded recipe: stop the pane's managed process through the common verified-stop path, require verified exit, then re-run the recipe in that same slot through the backend.
- [ ] 4.2 Refuse reload for a live process, a missing or stale binding, and a container with no recorded recipe, and require a fresh operator-supplied recipe for an observed container instead of inventing one.
- [ ] 4.3 Keep reload distinct from restart: reload reuses the slot and records its own outcome, restart launches a replacement into a fresh pane, and neither reports a Pi session or assignment outcome it did not establish.

## 5. TUI control plane

- [ ] 5.1 Build the tree from containers: group by logical workspace, nest tabs and pane slots, and show the backend binding, registry facts, execution and assignment facets with freshness, and unresolved containers honestly.
      Status: partial. Containers now reach the client and name the tree. The dashboard client reads them with the registry facts (`topology.list`, gated on the contract, so a daemon that does not serve it reads as no containers rather than failing the refresh), the collector carries them on the observation surface, and the tree titles a workspace or tab group from the container that binds that handle, falling back to the backend's own label. A container the daemon removed names nothing. Still open: rows carrying the container identity and binding (which 5.2's targets need), roots and nesting driven by containers rather than by the backend's own groups, and unresolved containers shown as such.
- [ ] 5.2 Add the create, delete, reload, focus and attach actions with the same confirmation discipline as existing lifecycle actions: nothing submitted while opening confirmation, frozen target, explicit confirm.
- [ ] 5.3 Read every confirmed container operation back by request ID, keep a lost answer unresolved, and never fall back to the owner-file transport, a local retry or optimistic row removal.
- [ ] 5.4 Show observed-external containers as unmanaged with the existing generic close safeguards, and never reload them or record them as owned.

## 6. Acceptance and consumer contract

- [ ] 6.1 Socket coverage for creation, uncertain creation, deletion fences, replay, live-process refusal and reload refusals, over the scripted backend fixture and a disposable tmux server.
- [ ] 6.2 PTY coverage for the container tree, the confirmations, operation readback and unmanaged containers.
- [ ] 6.3 Document the container contract, the verbs and their outcomes in the consumer, protocol and control-plane references, and extend the canonical fixture.
- [ ] 6.4 Run the disposable-tmux acceptance for create, delete, reload and focus with Herdr unavailable, and record the evidence.

## Deferred

- Per-workspace refresh that re-reads recipes from disk and rebuilds a session from a saved layout template, using tmux's own layout engine rather than a Radar-built one.
- Rename, move and explicit reorder of containers.
- Client-local navigation and the pinned sidebar, which the sidebar prototype owns.
