## Context

See proposal.md for the motivation. The sidebar adds no daemon behaviour. The TUI already reads the shared `observation.get` through `snapshot_fetch`, keeps tree, fold, selection and filter locally, and drops the details panel when `body_areas` gets none. `row_item` already gives way on the name span when a row is wider than its panel. Focus runs `select-window` and `select-pane` clientless against the one attached client (`tmux.rs`), which is correct for one view. `attach.rs` already launches a configured host program as a plain argv, which is the pattern for the hide and unhide commands.

## Goals / Non-Goals

**Goals:**
- A narrow tree that stays beside the agent's tmux client and costs no new daemon surface.
- Terminal-specific behaviour confined to launcher configuration.

**Non-Goals:**
- Multiple views, per-client window selection or a `view` argument on focus. Revisit only when a second view is wanted, with its own contract.
- Following the agent that is active in the view. It needs foreground evidence for the view's client and is recorded in `ideas.md`.
- Anything specific to Kitty or WezTerm inside Radar.

## Decisions

**A presentation mode on the existing App, chosen by invocation.** `radar sidebar` starts the same dashboard with `App::sidebar()`. `ui` reads it to pick the compressed row, to skip the details panel and to switch to the rail; `app` reads it to gate the actions. Alternative: a separate sidebar binary or App type, rejected because it would copy selection, filter and refresh logic that the projection should share.

**The rail follows the width, not a stored hidden flag.** Below a minimum width (tuned on the prototype, starting at 16 columns) the sidebar draws a vertical strip of state marks, one per visible row. A stored `hidden` flag would drift from the terminal's real size, for instance after a manual resize. The toggle key therefore runs `show_command` when the rail is drawn and `hide_command` otherwise, with no state of its own.

**Hide and unhide are argv commands run without a shell.** The terminal owns the split's width, so the only portable thing Radar can do is run a command the user configured, as it does for `[tui] terminal`. The command runs to completion, since a host resize returns at once, and a start failure or non-zero exit is reported in the footer. Alternative: a `kitten`-aware implementation in Radar, rejected because it ties Radar to one terminal.

**Unhide is the sidebar's key.** `B` runs `show_command` while the rail is drawn, so unhide needs the sidebar to hold focus, which a `neighboring_window` binding gives. A separate host binding that unhides without focus is not built.

**Destructive actions are omitted by presentation, not by dialog.** The sidebar does not bind them and the footer does not list them. A dialog does not fit in about 30 columns, and the full TUI already has them. Alternative: hand the confirmation to a `tmux display-popup`, deferred until someone needs it.

**Focus is unchanged.** The sidebar sends the same focus action as the full TUI. With one view and one attached client, the daemon's clientless `select-window` and `select-pane` move the client the user is looking at.

**Launchers are configuration.** A Kitty session layout with a left split running `radar sidebar` and a right split running `tmux attach`, plus the unhide host binding, ship under `examples/` and are documented in `docs/`. A WezTerm launcher follows with no Radar change. Exact Kitty and WezTerm commands are taken from their documentation and verified on the prototype, not assumed here.

## Risks / Trade-offs

[The sidebar loses focus after a focus action, so a second key press goes to the tmux pane] → The launcher documents `neighboring_window` bindings to move focus back, and acceptance covers it.

[A narrow terminal truncates names and several agents look alike] → The name span gives way last-in, and the mark and state stay readable; revisit with the hotkey revamp if rows prove ambiguous.

[A hide command that exits non-zero is silent otherwise] → The footer reports it; nothing else changes.

[A second dashboard process doubles the observation reads] → Reads are served from the shared sampler, which samples once for every reader.

## Open Questions

- The toggle key is provisional (`B`, which is free today) until the hotkey revamp assigns keys.
- The exact rail threshold is tuned on the prototype.
