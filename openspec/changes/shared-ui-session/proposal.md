# Proposal

## Why

Sidebar, dashboard and popup processes currently share fleet facts but own separate navigation and command continuations. A popup therefore reconstructs another App and its process lifetime can lose interaction state or results. Centralize the shared interaction session before choosing a different sidebar host.

## What Changes

- **BREAKING**: all dashboard/sidebar clients attach to one volatile daemon-owned UI session; selection, folds, visibility, ordering and live fuzzy filtering are no longer process-local.
- Extract typed commands and a terminal-independent reducer; retain local geometry, details, viewports, informational overlays and picker editing.
- Publish ordered references to content-addressed tree and session payloads, with bounded retrieval and reconnect semantics.
- Use attachment-local action IDs with bounded replayable receipts, one shared-text editor sending full queries, and a modal input-generation barrier.
- Use a reusable ordered action/RPC connection and a dedicated `ui.subscribe` push stream in Slice B, with gap-free baseline delivery, bounded async writers and latest-state coalescing. Adopt Tokio select! plus Crossterm EventStream with one synchronous Ratatui rendering owner. Keep the dedicated actor thread synchronous initially; measure before adding async projection workers.
- Give shared command interactions one inline or detached presenter. Presenters submit typed results directly over IPC; tmux exit codes and parent callbacks do not carry choices.
- Reuse existing domain admission and request IDs, preserving frozen targets and authoritative outcomes, with explicit scheduling refusal and uncertain-submission recovery. Remove migrated frontend dispatch, synthetic-key replay and modal reconstruction paths.
- Design the full session/interaction contract together as a soft freeze and migrate the frontend/shared commands in one coordinated pass; no interim UI long-poll path or staged compatibility layers. Existing domain reads and tmux observation are not migrated to subscriptions here.
- Evaluate Strum and tui-actions during semantic extraction, including leader sequences. No actor framework, new daemon or terminal-engine change.

## Capabilities

### New Capabilities

- `ui-session`: canonical navigation, attachment/action ordering, live filtering, content-addressed projections and volatile session lifecycle.
- `ui-interactions`: globally blocking command interactions, single-presenter delivery, typed results and domain-operation handoff.
- `tui-commands`: typed command identity, frontend-local key/mode resolution and consistent command metadata.

### Modified Capabilities

- `lifecycle-actions`: project candidates no longer depend on the requesting process's working directory; a configured base directory replaces the leading start-directory entry as the fallback.
- `fleet-overview`: shared canonical navigation/filtering and session-scoped task visibility, without moving local details/layout into the actor.
- `row-navigation`: semantic jumps operate on the canonical visible tree.
- `daemon-transport`: UI actor calls and bounded push subscriptions use the existing socket without holding synchronous handler/control capacity for subscription lifetimes; slow clients cannot block the actor.
- `sidebar-view`: shared tree state and detached interaction presentation replace process-local state and reconstructed popup Apps. This capability is currently introduced by `sidebar-mode`; its baseline must be synchronized before applying this delta to main specs.

## Impact

Slices A–D are one change because they transfer one ownership model: command/model extraction; daemon session/protocol; project-picker modal vertical slice; remaining command migration and deletion of old paths. Slice E is the dependent `tui-interaction-ux` change and may start after milestone C.

Affected seams include `src/app.rs`, `src/app/commands.rs`, `src/main.rs`, `src/ui.rs`, `src/picker.rs`, `src/project.rs`, observation demand, transport dispatch and existing operation services/schema generation. Keep Ratatui/Crossterm/Tokio; propose BLAKE3 and nucleo-matcher, and evaluate Strum/tui-actions compatibility early. Public UI methods require generated contracts and coordinated release discipline, not legacy fallback.

No lifecycle journal redesign, UI persistence, multi-user editing, multiple named sessions, independent physical-host focus routing or mux replacement. Existing unfinished domain changes and `sidebar-mode` acceptance remain separate; do not archive or silently supersede their unchecked work.
