# Sidebar

`radar sidebar` runs the dashboard as a narrow tree beside the tmux client you
work in. Selection, folds and filters belong to that dashboard process. Rows
keep state marks, names and exceptional state labels, but omit ages and model
metadata. There is no details panel.

Navigation, folds, filters, jumps, `p` (pane view), `e` (finished) and `b` (tasks)
work as in the dashboard, and `?` opens the command palette, which lists every
command with the key that runs it. `Enter` focuses the selected pane through the daemon.
On a workspace it switches to that session, preserving its active window and
pane. One view is supported.

The sidebar has the dashboard's management keys, including launch and lifecycle
controls. Narrow confirmations are not yet promoted to workspace-wide popups.
For the outer-tmux experiment and its full-width management surface, see
[Workspace host prototype](workspace-prototype.md).

## Kitty setup

### Beside the window you are already in

One key, from any Kitty window — a tmux client, a shell, pi:

```conf
map ctrl+shift+s launch --location=vsplit --bias=25 --allow-remote-control sh -c "radar sidebar; kitten @ close-window --self"
```

The sidebar takes a 25% split of that window and the tmux client keeps the rest.
`q` in the sidebar quits it and the wrapper closes the split. `Ctrl+Shift+Left`
moves focus to it, `B` hides it, `Z` zooms it. Add `map ctrl+shift+left
neighboring_window left` if you do not have it already.

### A tab of its own

Start the daemon and ensure its tmux server has a session, then launch Kitty with
the session file only, so your own `kitty.conf` still applies:

```sh
kitty --session /path/to/examples/kitty/radar-sidebar.session
```

The session uses Kitty's splits layout: the sidebar gets 25% of the width and the
tmux client gets 75%, and focus starts on the tmux side. If you changed
`[daemon] tmux_server`, change `-L radar` in the session file to match. Copy the
session file anywhere you like.

The sidebar is launched with `--allow-remote-control`; global remote control
need not be enabled. A window variable identifies it independently of its title.

Add these commands to Radar's configuration:

```toml
[sidebar]
hide_command = ["kitten", "@", "resize-window", "--self", "--axis=horizontal", "--increment=-10000"]
show_command = ["kitten", "@", "action", "--self", "layout_action bias 25"]
```

`B` hides the sidebar by shrinking it to Kitty's minimum width. Below 16 columns
Radar draws a rail of state marks. Pressing `B` in the rail restores 25% width.
The restore command sets the split's bias — the sidebar's share of the tab — so
repeated toggles cannot accumulate resize drift. `--self` makes both commands act
on the sidebar's own split rather than whichever window has focus. Manual resizing is
also supported; the rail follows the actual width.

`Z` expands the sidebar into the full dashboard and back. It is an explicit mode of
the same process, not a width: `Z` asks the host to give the sidebar the window
with `zoom_command`, and once that has succeeded the process draws the whole
dashboard, with the details panel and every key. `Z` again runs `unzoom_command`
and returns to the compressed tree. A host command that is unset or fails leaves
the mode as it was. The commands should state the layout they want rather than
toggle it, so a manual resize cannot leave the host and the mode disagreeing:

```toml
[sidebar]
zoom_command = ["kitten", "@", "action", "--self", "layout_action bias 100"]
unzoom_command = ["kitten", "@", "action", "--self", "layout_action bias 25"]
```

Each `action` argument is one argv element (`"layout_action bias 25"`), which is
how `kitten @ action` expects an action and its arguments.

Optional host keys, from `examples/kitty/sidebar.conf`, pasted into your own
`kitty.conf` or home-manager's `programs.kitty.extraConfig` — without them, click
the sidebar to focus it and `B` and `Z` work from there:

- `Ctrl+Shift+Left/Right`: move keyboard focus between splits.
- `Ctrl+Shift+B`: restore and focus the sidebar, including from the tmux split.
  It needs remote control, so if it does nothing, set `allow_remote_control
  socket-only`.

Commands are argv, not shell text. They run without blocking the dashboard.
Unset commands, start failures and unsuccessful exits produce a diagnostic.
A second resize is refused until the first command finishes.

## Operator acceptance

With the example layout open:

1. Check Pi rendering, resize, paste, mouse and clipboard in the right split.
2. Focus an agent with `Enter`; verify the right client shows that pane.
3. Focus a workspace; verify it opens at its last active window and pane.
4. Use `B` to hide/unhide repeatedly, then manually resize and repeat. Check
   that hide reaches the rail and restore returns to the configured proportion.
5. From the right split, use `Ctrl+Shift+B` to restore and focus the sidebar.
6. Press `Z`; verify the dashboard fills the window, the details panel and
   lifecycle keys work, and `Z` returns it to the split.
7. Close the sidebar; verify agent processes and their tmux sessions remain.

These desktop checks are performed by the operator. Command syntax has been
checked against Kitty's documentation and the installed CLI help, not by
running a live desktop acceptance session.

Other terminals need their own launcher and host commands; Radar does not know
which terminal hosts it. The WezTerm launcher is a follow-up.

References: [Kitty sessions](https://sw.kovidgoyal.net/kitty/sessions/),
[splits layout](https://sw.kovidgoyal.net/kitty/layouts/#the-splits-layout),
[remote control](https://sw.kovidgoyal.net/kitty/remote-control/).
