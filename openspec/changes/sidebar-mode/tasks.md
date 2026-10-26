## 1. Sidebar mode

- [x] 1.1 Add the `sidebar` invocation in `src/cli.rs` and `src/main.rs`, and a `Presentation` on `App` that the invocation sets. Verify with one app-level test that the sidebar presentation has no details panel and that navigation, fold, filter and `PaneView` keys behave as in the full dashboard.
- [x] 1.2 Draw compressed rows in `src/ui.rs` (mark, name, state; the name gives way first) and no details panel. Verify with a render test at a narrow width.
- [x] 1.3 Retain management actions in the sidebar and its command picker; verify launch and restart bindings at compact and zoomed widths.
- [x] 1.4 Hand the sidebar's exact selection and pending action into a workspace-wide picker/confirmation through `radar modal`, preserving cancellation and revalidation.

## 2. Hide and unhide

- [x] 2.1 Add `[sidebar] hide_command` and `show_command` to `src/config.rs` as argv vectors, with defaults unset and an entry in the generated config text. Verify parsing and the unset default.
- [x] 2.2 Draw the collapsed rail below the minimum width, and run `hide_command` or `show_command` on the toggle key by which of the rail and the tree is drawn. Run the command without a shell, and report an unconfigured command or a failed one in the footer. Verify the rail threshold and that an unconfigured toggle runs nothing.

## 3. Launchers and documentation

- [x] 3.1 Add a Kitty launcher under `examples/` (a layout with the sidebar on the left and `tmux attach` on the right; focus moves with Kitty's `neighboring_window`), taking the commands from Kitty's documentation. Document it, and the sidebar, in `docs/`.
- [x] 3.3 Add `Z` and `[sidebar] zoom_command` / `unzoom_command`, toggling an explicit expanded mode of the same process that draws the whole dashboard; the mode never depends on width.
- [x] 3.2 Record follow mode and the multi-view focus argument in `ideas.md`, and add the WezTerm launcher as a follow-up row.

## 4. Gate and acceptance

- [x] 4.1 Run fmt, strict Clippy, the Rust test suites, strict OpenSpec validation and the Nix build. All exit 0 on the explicit expanded mode (18 suites ok, `radar 0.7.0 (wire 10)`).
- [ ] 4.2 Operator: run the live acceptance in `docs/workspace-prototype.md` (nested Pi rendering, keys, mouse and clipboard; sidebar `?`/`A`/`D` popups over both areas on the selected row; `Z`/F11 expand and return; `B` rail; detaching leaves the agents running), and the Kitty layout in `docs/sidebar.md` if it is kept.
