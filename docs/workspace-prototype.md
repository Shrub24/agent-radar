# Workspace host prototype

This is a presentation experiment, not a new multiplexer backend. It adds an
outer tmux server containing the Radar sidebar and one client attached to the
daemon's existing tmux server. The outer server owns splits, zoom and popups;
the inner server continues to own the agent panes.

## Try it

Build the current checkout, then run the actual binary from an interactive
terminal:

```sh
cargo build --bin radar
./target/debug/radar workspace
# Or pick an existing session on the daemon's configured tmux server:
./target/debug/radar workspace --session 0
```

The launcher checks that the backend session exists before creating a view. It
uses a private, per-launch UI server with no user tmux configuration, no status
bar and no prefix. It does not create agents or change the daemon's server.
The sidebar is on the left; keyboard focus starts in the attached agent area.

| Key | Action |
| --- | --- |
| F9 | Open the full dashboard in a centred popup over both panes |
| F10 | Focus the sidebar |
| F11 | Toggle sidebar pane zoom |
| F12 | Focus the agent area |
| B, in sidebar | Shrink to rail / restore to 32 columns |
| Z, in sidebar | Toggle sidebar pane zoom |

Inside the F9 popup, `?` opens the shared command picker and `A` opens the
workspace picker. Lifecycle confirmations use the ordinary dashboard path.
`q` closes the dashboard popup once its local picker/confirmation has closed.
The launcher supplies tmux hide/show/zoom commands to its sidebar instead of
using the Kitty commands in the user's configuration.

The outer server is disposable: detaching from it returns to the launching
terminal and cleans up that UI server. The inner server and agents are not
terminated. Closing the terminal abruptly is not yet a guaranteed cleanup path.
F9–F12 are prototype bindings, not a settled hotkey scheme.

## What this establishes — and does not

## Pickers and confirmations float over the whole workspace

In the sidebar, `r`, `a`, `A`, `E`, `D`, `R` and `?` do not draw in the narrow
pane. The sidebar asks the outer tmux for a centred popup on the outer client,
so it covers the sidebar and the agent area, and runs `radar modal` in it:

- `?` and `A` (the command and workspace pickers) get a 76×20 block; the
  confirmations get 68×16. The modal fills its popup, and nothing of the
  dashboard is drawn behind it.
- The sidebar passes the selected row's **identity** (`--row`, a JSON `RowId`),
  not its position. The popup lists every row, selects that identity exactly,
  and refuses with "that row is gone; nothing was changed" if it no longer
  exists. A neighbour never stands in for it.
- The popup then runs the same key through the same handler, so the same
  confirmation, the same revalidation and the same dispatch run as in the
  dashboard. Nothing is added to the daemon.
- It closes when the modal is answered and the work it started has finished. A
  message (an outcome, or a refusal) stays until a key is pressed or it expires.
- `?` in the popup lists only the commands that open a modal; the rest act on a
  view the popup does not show.

`L` (launch in the configured pane) has no dialog and runs in the sidebar. F9
still opens the whole dashboard in a popup.

## Operator acceptance

1. Compare Pi in the agent area against the ordinary direct tmux attachment:
   rendering, modifier keys, bracketed paste, mouse, clipboard and resize.
2. In the sidebar press `?`, `A`, then `D` on an agent row: each should float
   centred over both areas, act on the row that was selected, and cancel with
   `Esc`. Confirm one and check the sidebar updates behind it. Open F9 and close
   the dashboard with `q`.
3. Test F10/F12 focus, B hide/restore, and F11/Z zoom/restore.
4. Navigate to another existing agent session through Radar; verify the outer
   sidebar remains in place.
5. Detach the outer workspace and verify the original agent sessions remain.

The agent has compiled and tested command routing, not performed these live
checks. Do not interpret that as acceptance of nested keyboard/clipboard support.

## Alternatives investigated

- [Workmux sidebar](https://workmux.raine.dev/guide/sidebar.md) adds an ordinary
  sidebar pane to existing and new tmux windows, globally by default or scoped
  by session. Its [dashboard](https://workmux.raine.dev/guide/dashboard.md) is
  launched with `display-popup`. This avoids nesting, at the cost of adding
  presentation panes to the backend's layouts. The dashboard also offers live
  preview and forwarded input; that is distinct from replacing the terminal
  client with an embedded emulator.
- [Helvesec/rmux](https://docs.rs/crate/rmux/latest) advertises a ratatui widget
  for live RMUX panes and a separate local multiplexer daemon. That is a genuine
  embedding direction. The inspected crate page does not expose a working
  public widget example (the `rmux` package is marked as not a library), and does
  not establish that it can render existing tmux panes. Adopting it as the backend
  is not equivalent to adding an outer UI host. Embedding an RMUX-hosted tmux
  client would still introduce terminal emulation and input-forwarding concerns.
- [Kitty overlays](https://sw.kovidgoyal.net/kitty/launch/) cover one Kitty
  terminal window, not an entire split tab. [WezTerm's arbitrary popup
  issue](https://github.com/wezterm/wezterm/issues/270) remains open in the fetched
  upstream evidence. Neither establishes the same cross-pane popup primitive.

The prototype keeps the tmux backend and existing lifecycle ownership unchanged.
Revisit the host choice if the extra terminal layer fails the operator's input
or rendering checks; same-server panes are the closer analogue to Workmux.
