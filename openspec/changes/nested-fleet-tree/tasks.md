# Tasks

## 1. Task projection

- [ ] 1.1 Extend the row model with task identities and one normalized task projection, joining rich bus facts by exact session or the unambiguous established pane fallback and using distinct token ids only when no connected list is available; verify rich, token-only, explicit empty, disconnect, unknown phase, ambiguous join and same-id/different-session cases at the projection seam.
- [ ] 1.2 Rebuild task children and reconcile selection on both observation and bus events, keeping parent badges and detail summaries on that projection; verify a bus replacement updates rows without a runtime refresh, a selected removed task falls back to its parent, unchanged identities preserve selection/folds, and retained fallback facts stay labelled and still. Document source precedence and identity limitations beside the new tree behaviour in README.

## 2. Connected rows and task details

- [ ] 2.1 Derive visible sibling/ancestor continuation metadata after filtering and ordering and draw connectors beside the existing marks; verify TestBackend frames for one child, multiple siblings, nested workers/tasks, reordered/filtered siblings, folds and narrow widths, including existing text sanitization. Document the tree notation in README.
- [ ] 2.2 Render task labels, published phases, available ages and configured command motion, and selected-task details from the normalized projection; verify absent fields stay absent, unknown phases survive, optional process marks use current configuration, retained facts do not animate, and parent summaries/counts agree with task children.

## 3. Navigation and integration

- [ ] 3.1 Include tasks in ordinary selection/filtering and agent branches in disclosure-marker hit testing while preserving existing agent-state jumps and row-click focus; verify scrolled/folded click geometry, marker-only folding, filter ancestry/restoration and task-parent focus with current, stale and missing locations. Update README controls without adding a separate view mode or new hotkeys.
- [ ] 3.2 Run cargo fmt --check, cargo clippy --all-targets --locked -- -D warnings, cargo test --locked, the locked build and both existing PTY smoke scenarios; record actual outcomes and any timing-sensitive failures without treating a passing retry as resolution. Validate nested-fleet-tree strictly and confirm no Herdr command or socket operation was introduced into app/tree/UI code.
