# Proposal

## Why

Radar's TUI is a full-screen dashboard, so watching the fleet means leaving the agent you are working in. A narrow, always-visible tree beside the agent's own tmux client removes that trip: the fleet stays in view and one key brings any agent forward. The daemon already serves a shared observation and an existing focus action, so the sidebar needs a presentation of the TUI, not new daemon behaviour.

## What Changes

- Add `radar sidebar`: the same dashboard, run as its own process on the shared `observation.get`, drawn as a compressed session tree for a narrow terminal split. Tree, fold, selection and filter stay local to that process, as they are in the full TUI.
- The sidebar draws compressed rows (mark, name and state), omits the details panel, and keeps the full TUI's navigation, filter, fold, `PaneView` and jump keys.
- The sidebar retains dashboard management, including launch and lifecycle actions. Confirmation and revalidation use the same path; where the host can float a popup over the whole workspace, the sidebar opens its pickers and confirmations there on the selected row, by identity.
- Selecting an agent and pressing the focus key uses the existing focus action. One view is supported: the right-hand split is one plain `tmux attach`, so a focus moves that client. No focus parameter is added.
- Hide and unhide are a pair of configured commands, `[sidebar] hide_command` and `show_command`, run by the TUI on one key. Below a minimum width the sidebar draws a collapsed rail. The width itself belongs to the host terminal.
- Ship a Kitty launcher (a session layout and a host binding for unhide) as configuration. A WezTerm launcher follows later and needs no Radar change.

Non-goals: more than one view or per-client window selection, highlighting the agent that is active in the view (follow mode, recorded in `ideas.md`), any code that knows which terminal hosts the sidebar, the hotkey and command-picker revamp, and remote (SSH) sidebars.

## Capabilities

### New Capabilities

- `sidebar-view`: the sidebar presentation of the dashboard: its compressed tree, the actions it offers and omits, focusing an agent in the view, and the hide and unhide commands.

### Modified Capabilities

None.

## Impact

- `src/cli.rs` and `src/main.rs` gain the `sidebar` invocation; `src/app.rs` gains a presentation mode; `src/ui.rs` gains the compressed layout and the rail; `src/config.rs` gains `[sidebar]`.
- Launcher configuration and docs ship under `examples/` and `docs/`; no daemon, wire or SDK change.
- Depends on the shared observation and focus action that already exist.
