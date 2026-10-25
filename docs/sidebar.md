# Sidebar

`radar sidebar` runs the dashboard as a narrow tree for a split beside the tmux
client you work in. It is its own process on the daemon's shared observation, so
its selection, folds and filter are its own. It draws compressed rows (state
mark and name, no columns, no details panel) and answers to the dashboard's
navigation, filter, fold, `p` (pane view), `e` and `b` keys. `Enter` focuses the
selected agent through the daemon's ordinary focus, which moves the one tmux
client attached to the served server; one view is supported.

The sidebar offers none of the dashboard's lifecycle actions (`r`, `a`, `A`,
`L`, `E`, `D`, `R`) and no details; use the full dashboard for those.

## Hide and unhide

The terminal owns the split, so Radar only runs the commands you configure,
without a shell, to resize it:

```toml
[sidebar]
hide_command = ["kitten", "@", "resize-window", "--self", "--axis=horizontal", "--increment=-20"]
show_command = ["kitten", "@", "resize-window", "--self", "--axis=horizontal", "--increment=20"]
```

`B` runs `show_command` while the terminal gives the sidebar fewer than 16
columns (it then draws a one-column rail of state marks) and `hide_command`
otherwise. With the command for that direction unset, the footer names the
missing setting and nothing runs. A command that cannot start or exits non-zero
is reported in the footer.

## Kitty

`examples/kitty/radar-sidebar.session` lays out the sidebar and a
`tmux -L radar attach` client side by side. The sidebar window is launched with
`--allow-remote-control`, so the `kitten @` commands above work from it without
enabling remote control for every window. Start it with
`kitty --session examples/kitty/radar-sidebar.session`. To move focus between the
splits, bind `neighboring_window left` and `neighboring_window right` in
`kitty.conf`; `B` works from the sidebar while it holds focus, including on the
rail.

Whether `resize_window` and `kitten @ resize-window` shrink the sidebar to a rail
as intended in Kitty's tall layout is part of the live acceptance. Other
terminals need only their own launcher and resize commands; Radar does not know
which terminal hosts it.
