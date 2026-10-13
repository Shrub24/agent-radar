# Design

## Context

The daemon already creates and closes backend locations: `mux.create` and `mux.close` are capability-gated methods, `CreateRequest`, `CreatedLocation`, `CreatedKind` and `CreateOutcome` exist on the runtime seam, and tmux implements them. Spawn edges record a location together with the serving instance, and registrations record a location string.

What is missing is ownership. Nothing durable records that Radar created a workspace, tab or pane, so the tree in `src/model.rs` is keyed by the backend's own ids, a location cannot be reacquired after a restart except through a registration, and there is no identity to fence a deletion against. Confirmed managed controls still leave the TUI as owner-file requests in `src/control.rs`.

## Decisions

### Containers are Radar's; the backend's space is a binding

The daemon records a container as a logical subject with its own identity, kind, parent, label and provenance, and keeps the backend name, serving instance and raw handle as a separate binding. A handle is evidence that a container exists somewhere; it never names one.

This is what makes the hierarchy an abstraction rather than a mirror. It also gives deletion something to fence: a handle that reappears under a different serving instance is not the recorded container, so nothing is dispatched for it.

### A container is the pane facet of the managed composition

The managed subject owns process and Pi-session identity; the container owns the pane slot. An agent occupies a container, and a container can outlive the process in it, because tmux retains a dead pane.

Consequences: a registration joins the tree through its location evidence; a registration that cannot be joined stays a distinct unresolved row rather than being attached to a guessed container; and the two replacement shapes stay distinct — restart launches a replacement into a fresh pane, reload re-runs the recipe in the existing slot.

### Provenance is explicit, and observation is not adoption

A container Radar created is its own; a container Radar only observed is shown as unmanaged. Ownership is what grants a recorded recipe and therefore reload. Without the distinction, upgrading a fleet would claim a user's own tmux session as Radar's, and a reload could overwrite a pane Radar never created.

Unmanaged containers keep the generic close path with its existing containment and identity checks. They never gain ownership implicitly.

### Recipes stay external and private

Radar owns the container lifecycle; the working directory, command and environment arrive in the creation request from external policy, which keeps project and JJ workspace decisions outside Radar. Recipe values are private launch data under the existing rule: bounded owner-only storage, and no value in any read, operation readback, log, diagnostic or event payload.

### Reload reuses the slot, restart does not

Reload exists because relaunching in place is what an operator wants when a workspace layout should stay put. tmux's respawn replaces whatever is running in the pane, so reload stops first through the common verified-stop path and requires verified exit (or an already recorded exit) before running the recipe again. A process that may still be running is refused, never replaced.

Reload records its own outcome. It is not a Pi-session restart, it does not create a container, and it is not an assignment fact. One measured limit matters here: a respawn destroys the retained pane-command exit status, so reload cannot lean on that status for anything — which is another reason its outcome is recorded on the container.

### Deletion never silently ends a live managed process

Removing a pane ends whatever runs in it. When the pane holds a live managed process, deletion refuses and names that process, and the operator runs the recorded stop first. The two effects keep separate facts: the stop reports verified exit, the deletion reports the pane removal, and neither claims the other.

### Reconciliation reports drift and changes nothing

Container reads carry per-source freshness. Reconciliation reports what it observed — a missing container, a foreign occupant, a changed handle, a replaced serving instance — as source-labelled facts, without rewriting identity, adopting a foreign handle, deleting a record or dispatching an effect. Records of containers that are gone are kept, marked unresolved.

### The TUI is rewritten once

Two pieces of TUI work are in flight: the tree moving onto container identity (this change) and the managed-subject controls moving off owner files (the lifecycle change's section four). Doing either alone would leave the client targeting backend ids, or two writers for the same control. They land as one pass over the tree model, the confirmations and the control client.

Ordering inside this change: records and reads first, because they are what the tree renders; then creation, deletion and reload; then the actions.

## Deferred

- **Per-workspace refresh from a saved layout template.** It recreates a session, which is destructive to whatever is live there, and the layout engine should be tmux's own layout mechanism or an external tool rather than one Radar builds. Deferred until the container model has settled.
- **Rename, move and reorder.** Not needed for a usable control plane; the first pass orders by the binding's natural index and labels from the container record.
- **Client-local navigation and the sidebar.** Selecting a window is session state, so a client identifier alone does not give independent navigation; the sidebar prototype owns that question.

## Risks

- **Handle reuse across a replaced server.** Fenced by the serving instance, so a reused handle is not mistaken for the recorded container. A container whose instance changed stays unresolved rather than being cleaned or reloaded.
- **Records against reality after a crash.** Creation records before it reports, an uncertain creation stays unresolved, and reconciliation reports missing containers without deleting their records. A container that vanishes therefore leaves a visible, removable record rather than silence.
- **Recipe secrecy.** Reads must project away the command, working directory and environment exactly as the lifecycle read projects away a private launch specification.
- **Two close paths for one container.** An observed container is shown as unmanaged yet remains closable through the generic path. The rule is deliberate: ownership gates recipes, ownership does not gate closing with safeguards.

## Relationship to other changes

- **`daemon-managed-lifecycle`** supplies the verified stop, pidfd process control, durable operation records and the managed-subject controls this change reuses and does not duplicate.
- **`managed-lead-restart`** stays superseded.
- **tmux 3.8 observation** is additive beneath these reads. Container identity does not depend on it, and this change does not require it.
