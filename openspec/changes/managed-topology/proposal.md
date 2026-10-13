# Proposal

## Why

With a tmux backend the fleet tree is still the multiplexer's own topology. `src/model.rs` keys workspaces, tabs and panes by the backend's own ids, and nothing durable records a container Radar created, so the view can show and close physical panes but cannot present Radar's hierarchy or create, delete and reload a location durably. Confirmed managed controls also still travel as TUI-published owner files.

A control plane that abstracts the backend's space into Radar's own hierarchy is what makes the fleet visible and controllable rather than a raw tmux session.

## What Changes

- Make containers daemon-owned logical subjects, mirroring the managed-subject model: durable identity, kind (workspace, tab or pane slot), parent link, label, provenance, and a separate backend binding. The handle is evidence, never identity.
- Group the fleet tree by those containers, with the binding, registry facts and the execution and assignment facets carried as attributes with per-source freshness. Containers Radar only observed stay visible as unmanaged and are never recorded as Radar's own.
- Add container verbs to the daemon and the TUI: create (workspace, tab, pane or agent) from an external recipe, delete with confirmation, a generation fence and a durable outcome, per-pane reload, focus, attach, and reconcile reads that report drift instead of guessing.
- Reload relaunches one pane's recorded recipe in the same pane slot, after the common verified stop, and never replaces a live process. Restart stays the lifecycle change's new-pane replacement; the two are distinct operations with distinct records.
- Keep the recipe external: Radar owns container lifecycle, not project or JJ workspace policy. Per-workspace refresh from a saved layout template is deferred, and its mechanism is tmux's own layout engine rather than a Radar-built one.
- Route confirmed container controls through daemon operations with request-ID readback, in the shape lifecycle controls already use.

Non-goals: a Radar-built layout engine, project or JJ workspace creation, rename, move and reorder, automatic creation or deletion, adopting observed containers as owned, client-local navigation (the sidebar prototype owns it), and any change to assignment or result authority.

## Capabilities

### New Capabilities

- `managed-topology`: durable logical containers, their backend bindings and provenance, creation, deletion, per-pane reload, reconciliation reads and the control operations the TUI confirms.

### Modified Capabilities

- `fleet-overview`: the tree is grouped by the daemon's containers instead of a backend's workspace ids, and carries identity and binding as separate facts.

## Impact

- Daemon registry, store and server in `src/control_plane/`, the runtime create/close seam in `src/runtime.rs` and `src/tmux.rs`, and a container module beside the lifecycle one.
- The tree model and its rows in `src/model.rs`, `src/tree.rs` and `src/app.rs`, and the control client in `src/control.rs` for the container verbs that replace managed owner-file requests.
- Consumer documentation, canonical fixtures, socket and PTY coverage, and a disposable tmux acceptance run.
- Depends on the managed lifecycle change for the verified stop, pidfd process control and durable operation records that deletion and reload reuse. Containers are the pane-identity facet of the composition; managed subjects keep process and Pi-session identity.
