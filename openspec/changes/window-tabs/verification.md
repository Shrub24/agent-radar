# Verification

## Behaviour

- A window whose agents have exactly one root (an agent with no owner in that window) is drawn as a tab: that agent's row is the tab, and the window's other agents and ordinary panes nest beneath it. The window name stays on the tab row as its label.
- A window with no root agent, or several, keeps its agents flat beneath the workspace.
- An owner in another window is not used for nesting. The worker is grouped by its own window.
- The window rule is covered by `a_window_with_one_root_agent_is_a_tab_that_holds_its_other_agents`, `a_window_with_two_unowned_agents_stays_flat`, and the rewritten `nests_owner_across_tabs_and_keeps_tab_detail` and `retained_association_projects_a_marked_agent_row_without_a_pane_duplicate`.
- The fold and filter tests that relied on a worker nesting under an owner in another window now build a window-nested fixture (`nested_fixture`, `nested_fixture_state`). The shared fixture is unchanged.

## Acceptance

Full gate on the working tree, exit 0: `cargo fmt --check`, strict Clippy (`-D warnings`), `cargo test --locked --all-targets` (412 library tests and every integration suite), `cargo build --locked --all-targets`, `openspec validate --all --strict` (22/22), the publisher fixture smoke, the PTY terminal smoke, and the disposable-tmux acceptance (Herdr never called).

## Limits

- Tab-level delete and reload on a tab row are not in this change; they need the tab's container identity on its row.
- A window whose agents have several roots, or only ordinary panes, stays flat by design.
- Non-agent windows are not given a primary pane; the design for them is open.
