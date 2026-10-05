# Proposal

## Why

Herdr's sidebar cannot provide the configurable fleet overview needed for monitoring coding agents across workspaces. Radar needs its own full-screen view without making Herdr's layout or lifecycle status the definition of an agent.

## What Changes

- Establish a Rust/Ratatui application with a pinned Nix development environment and direnv entry point.
- Observe all workspaces in one local Herdr instance through a simple snapshot connector.
- Present a workspace-grouped tree and selected-row detail panel, with agent ownership, runtime status, available role/assignment metadata, and optional ordinary panes.
- Support Vim-style selection, folding and filtering. Runtime focus actions are deferred.
- Keep agent identity, runtime location and future process bindings separate internally; use Herdr workspaces as the initial display grouping.
- Read the Herdsman pane-metadata facts: role, ownership, the owner's projected state with Herdr's own status as the documented fallback, the active assignment and when it started, and the session's model, provider, thinking level and context usage.
- Distinguish source-reported facts, missing semantic information and stale observations. Preserve an agent's last observation through an idle-shell restart gap until its pane dies or a replacement command is positively observed.

## Why it grew

The first pass read Herdr's own agent fields and left role, assignment and semantic state unavailable, because the Herdsman facts arrive as pane metadata tokens rather than as Herdr fields. Herdsman's published contract already carries them — `pi_herdsman_role`, `pi_herdsman_session` / `pi_herdsman_parent_session`, `pi_herdsman_state` (the owner's projection), `pi_herdsman_task`, `pi_herdsman_started`, plus `model`, `provider`, `thinking` and `context_usage` — so reading them is a consumer change, not new data.

Out of scope: independent daemon, durable history, direct Herdsman feed or mailbox reads, process/resource collection, remote or multiple runtimes, lifecycle controls, popup/sidebar integration, user-defined layouts, and alternative mux connectors.

## Capabilities

### New Capabilities

- `runtime-observation`: Snapshot-based runtime inventory, normalized agent observations and pane-backed continuity with explicit freshness.
- `fleet-overview`: Full-screen workspace and ownership presentation, optional ordinary panes, selected details and keyboard navigation.

### Modified Capabilities

None. This is a greenfield project with no existing capability specs.

## Impact

Introduces the Cargo project, Rust dependencies, `flake.nix`, `flake.lock`, `.envrc`, and minimal development documentation during implementation. Herdr remains an external runtime dependency; only its connector handles wire formats and metadata tokens. No changes to `herdr-radar`, `pi-extensions`, or Herdr are required.
