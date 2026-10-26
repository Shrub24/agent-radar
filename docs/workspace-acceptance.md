# Workspace host: live acceptance runbook

Run this after switching the machine to the new build (Radar 0.7.0, wire 10). It
is the operator's check for task 4.2 of `sidebar-mode`; the agent cannot run it.
Record each step as pass, fail, or a note. A failure needs the step number, what
you pressed, and what you saw.

Keys used below: the **sidebar** is the left pane, the **agent area** the right
pane. In the workspace there is no tmux prefix and no status bar.

## 0. Preconditions

| # | Do | Expect |
|---|---|---|
| 0.1 | `radar --version` | `radar 0.7.0 (wire 10)` |
| 0.2 | `radar daemon restart`, then `radar daemon status` | both units active, from the same store path as `radar` |
| 0.3 | `radar server status` | the `radar` tmux server runs; if not, `radar server start` |
| 0.4 | Start two or three agents in it (a Pi, and anything else you run) | they appear in plain `radar` |
| 0.6 | `radar --print-config` | no `[sidebar]` Kitty commands are required for the workspace; it supplies its own |

| 0.7 | `tmux -L radar show -gv extended-keys` | `on` (set `programs.radar.tmux.extraConfig`/`configFile`, then `tmux -L radar source-file <file>` for the running server) |

If 0.1 or 0.2 fail, stop: a 0.6.0 TUI or daemon will refuse the other.

## 1. Launch and teardown

| # | Do | Expect |
|---|---|---|
| 1.1 | `radar workspace` (add `--session NAME` to pick one; it is created if the server has none) | sidebar on the left, about a quarter wide; the agent area on the right shows that session; focus starts in the agent area |
| 1.2 | Optional, only if the server holds nothing you need: `radar server stop`, `radar workspace` | exits non-zero saying the server is not running, creates nothing |
| 1.3 | Press `F8` in the workspace (and, on a second launch, close the terminal tab) | the launcher returns, `tmux ls` shows no `radar-ui-*` server |
| 1.4 | `tmux -L radar ls`; look at the agents | the agent sessions and processes are untouched |

Relaunch for the rest.

## 2. The agent area (nesting)

This is the main risk of the outer tmux. Compare against attaching directly:
`tmux -L radar attach` in a separate terminal.

| # | Do | Expect |
|---|---|---|
| 2.1 | Use Pi in the agent area | colours and layout match the direct attach |
| 2.2 | Resize the terminal | both panes redraw correctly, Pi reflows |
| 2.3 | Modifier keys you use in Pi (Shift+Enter, Ctrl, Alt combinations) | same as direct |
| 2.4 | Paste a multi-line block | bracketed paste, no stray characters |
| 2.5 | Click and scroll in the agent area | mouse works |
| 2.6 | Copy text from Pi to the system clipboard | it reaches the clipboard |
| 2.7 | Anything you use that draws images or uses OSC passthrough | behaves as direct, or note it |

## 3. Sidebar basics

| # | Do | Expect |
|---|---|---|
| 3.1 | `F10` | focus moves to the sidebar. `F12` moves it back |
| 3.2 | `j`/`k`, `space`, `/`, `p`, `e`, `b`, `n`/`N`, `w`/`W` | as in the dashboard. By default panes with a command show, finished sessions do not |
| 3.3 | `Enter` on an agent row | the agent area switches to that agent's pane; the sidebar stays put |
| 3.4 | `Enter` on a workspace row | the agent area switches to that session, on the window and pane you last left in it |
| 3.5 | Rows | compressed: mark and name, plus a state label only when exceptional |
| 3.6 | Click a row | selects it; a second click focuses it |

## 4. Hide and show

| # | Do | Expect |
|---|---|---|
| 4.1 | In the sidebar, `B` | the sidebar shrinks to a one-column rail of state marks; the agent area grows |
| 4.2 | `B` again, twice more | back to 32 columns each time, with no drift |
| 4.3 | Drag-resize the sidebar narrow by mouse | the rail appears below 16 columns on its own, and `B` restores it |

## 5. Pickers and confirmations over the whole workspace

Do these in the sidebar while it is compact.

| # | Do | Expect |
|---|---|---|
| 5.1 | `?` | the command list appears centred over **both** panes, not inside the sidebar |
| 5.2 | Type `fold`, then `Esc` | the list narrows; `Esc` closes it and the popup disappears |
| 5.3 | Select an agent row, `D` | a confirmation is centred over both panes, names the row you selected, and defaults to Cancel |
| 5.4 | `Esc` or choose Cancel | the popup closes and nothing changed |
| 5.5 | Select a plain pane you can lose, `D`, Confirm | the pane closes, and the sidebar drops the row within a refresh |
| 5.5a | `D` on a managed agent, Confirm | the agent's pane closes with it once the process exit is verified; the effect reads `pane cleaned up`, not `pane kept` |
| 5.6 | Select a managed worker, `r`, Confirm | the restart runs and the result shows in the popup before it closes; the successor appears in a new pane and the predecessor's pane is gone, with no `pane is dead` shell left behind |
| 5.7 | `A` | the workspace picker appears over both panes; choose a directory and a workspace opens |
| 5.8 | `L` on a workspace row | the configured launch runs from the sidebar with no popup |
| 5.8a | Filter the sidebar until no row matches, leave filter editing, press `D` | the sidebar answers that nothing is selected; no popup opens |
| 5.9 | Open `D` on a row, and from the agent area close that pane before confirming | Confirm is refused or the popup says the row is gone; nothing happens to another row |
| 5.10 | Mouse: click Cancel and Confirm | both buttons answer |

5.9 is the important one: the popup must never act on a neighbouring row.

## 6. Expand into the full dashboard

| # | Do | Expect |
|---|---|---|
| 6.1 | Select an agent row, press `Z` | the sidebar takes the whole window and draws the full dashboard (details panel, wide rows) on the same selection |
| 6.2 | Use a filter, fold a branch, then `Z` | back to the split with the same selection, filter and folds |
| 6.3 | Expand, then press `D` or `?` | the dialog is drawn centred inside the expanded pane |
| 6.4 | `F11` from the agent area | it expands the sidebar and focuses it; `F11` again, or `F12`, returns, and the sidebar is compact again |
| 6.5 | Zoom the window by hand with the tmux binding, then `Z` | no stuck state: after one or two presses the mode and window agree |
| 6.6 | Expand, then press `B` | the rail does not appear while expanded; note what happens |

## 7. Failure behaviour

| # | Do | Expect |
|---|---|---|
| 7.1 | `radar daemon stop`, watch the sidebar | it shows the stale or unavailable state and does not crash; `radar daemon start` recovers |
| 7.2 | Kill the sidebar process (`q`) | the sidebar pane closes; the agent pane keeps running; relaunch to continue |

## Report back

For each failed or noted step: number, keys pressed, what you saw, and for
section 2 whether the direct attach differs. The decision this feeds is whether
the outer-tmux host stays as the way to run the sidebar, or the same-server
sidebar pane (the workmux approach) replaces it. Section 2 decides that.

## Not in this run

The Kitty split layout (`examples/kitty/`, `docs/sidebar.md`) is a separate
host and is not exercised here. Herdsman and pi-extensions are not involved.
