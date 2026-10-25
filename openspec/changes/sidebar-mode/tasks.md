## 1. Sidebar mode

- [x] 1.1 Add the `sidebar` invocation in `src/cli.rs` and `src/main.rs`, and a `Presentation` on `App` that the invocation sets. Verify with one app-level test that the sidebar presentation has no details panel and that navigation, fold, filter and `PaneView` keys behave as in the full dashboard.
- [x] 1.2 Draw compressed rows in `src/ui.rs` (mark, name, state; the name gives way first) and no details panel. Verify with a render test at a narrow width.
- [x] 1.3 Gate the destructive actions on the presentation so no key opens a confirmation dialog in the sidebar, and keep them out of the sidebar footer. Verify that such a key sends no request.

## 2. Hide and unhide

- [x] 2.1 Add `[sidebar] hide_command` and `show_command` to `src/config.rs` as argv vectors, with defaults unset and an entry in the generated config text. Verify parsing and the unset default.
- [x] 2.2 Draw the collapsed rail below the minimum width, and run `hide_command` or `show_command` on the toggle key by which of the rail and the tree is drawn. Run the command without a shell, and report an unconfigured command or a failed one in the footer. Verify the rail threshold and that an unconfigured toggle runs nothing.

## 3. Launchers and documentation

- [x] 3.1 Add a Kitty launcher under `examples/` (a layout with the sidebar on the left and `tmux attach` on the right; focus moves with Kitty's `neighboring_window`), taking the commands from Kitty's documentation. Document it, and the sidebar, in `docs/`.
- [x] 3.2 Record follow mode and the multi-view focus argument in `ideas.md`, and add the WezTerm launcher as a follow-up row.

## 4. Gate and acceptance

- [ ] 4.1 Run fmt, strict Clippy, the Rust test suites, strict OpenSpec validation and the Nix build. Record real exit codes. Blocker: `tests/cli.rs` fails on `radar 0.6.0 (wire 9)` versus the working tree's uncommitted `PROTOCOL_REVISION = 10`, which is another change's in-flight work; every other suite passes.
- [ ] 4.2 Hand the operator the live acceptance: in the Kitty layout, check that Pi renders correctly beside the sidebar, mouse and clipboard work in both splits, focusing an agent moves the right-hand client, hide and unhide work from both the sidebar key and the host binding, and closing the sidebar leaves the agents running.
