# Tasks

## 0. Audit and verify prerequisites

- [x] 0.1 Inventory shared-selection writers/readers, navigation actions, publication adoption, command target defaults, configured Launch and gate reconciliation. Record which paths become focus and which move local, with no surviving shared browsing cursor; deliver the inventory in design.md.
- [x] 0.2 Audit serving-version enforcement, Home Manager tmuxPackage and flake/check tmux versions. Require the serving server to report 3.8+ and add only missing packaging support/assertions. Verified: the adapter refuses an older server and parses version words either side of the floor; the flake's locked nixpkgs still packaged tmux 3.7c, so it was updated until `tmux.version` evaluated to 3.8; `programs.radar` now asserts the same floor, and a standalone module evaluation showed the 3.8 package accepted and a 3.7c one rejected; docs/mux.md and the option description state the floor. A full Nix package build was not run (no Rust or packaging inputs changed by the assertion).
- [ ] 0.3 (partly done, see verification.md) Record reproducible disposable-3.8 experiments in verification.md: server-wide client/session/pane/layout events, zoom, zero-session attach, read-only/ignore-size/no-output invariance, destroy-unattached and loss of the observer's attached session are recorded and tested; **detach-on-destroy with only the observer attached is still unverified**.

Stop point: finish the lifecycle/flag checks before relying on a persistent observer. The server-wide shape is chosen; do not introduce a per-session compatibility fallback.

## 1. Observer and current typed focus

- [x] 1.1 Add bounded notification ingestion and a coalesced refresh request through the existing sampler. Prove a hint pulls a read forward, bursts remain bounded and a hint during a read causes a subsequent refresh; unknown/output notifications cannot leak pane content to logs or storage.
- [x] 1.2 Add one supervised daemon-owned observer, lazy attachment on first session, reattachment after session loss and bounded retry. Fence it to the owned serving incarnation and cancel/reap on shutdown. Real-tmux checks cover zero sessions, session destruction, replacement and child cleanup; document its lifecycle and non-goals.
- [x] 1.3 Build typed unified focus from current client/location/zoom reads. Exclude the observer from all operator counts and focus targeting. Report no operator, unavailable evidence and unsupported multiple operators distinctly. Verify native navigation and zoom, observer-only attachment and refusal of ambiguous focus operations without choosing a preferred client.
- [x] 1.4 Publish baseline focus and observer health with last notification/loss reason. Implemented through the existing sampler: health rides the observation revision, a change republishes without a read, and a failed inventory read withdraws focus rather than presenting the last-good one. A baseline-time transition becoming visible without unrelated input is not separately tested; timer recovery on observer loss is unchanged.

## 2. Coordinated shared-focus and local-selection cutover

- [x] 2.1 Evolve existing shared-selection publication/adoption into observed focus and extract frontend selection/anchor/navigation. Shared browsing-move actions and global-cursor reconciliation are deleted; the same publication and adoption wiring now carries focus. Two attachments are covered by a unit test that adopting a publication preserves the other view's selection, and by the existing two-view socket test for shared settings. A socket-level "two views select different rows" case has no rows to drive it: the UI daemon test runs without a backend, and a real-tmux variant is left to acceptance (4.2). Documented in design.md's ownership table.
- [x] 2.2 Migrate every row-dependent TUI caller to pass its local selected row. Keyboard and mouse reach the same `command_row`; palette commands are tree commands. With no selection the view reports locally. An omitted row resolves observed focus, and a named row that is gone is refused where the command is frozen, never replaced by focus. Covered by unit tests over the fixture and a refusal test in the session.
- [ ] 2.3 (frozen at admission as before; no new test that a later focus change cannot retarget a confirmation; command docs not updated) Freeze implicit and explicit targets before preparation/confirmation; retain domain revalidation and frontend no-client recovery for explicit focus. Verify later focus/selection changes cannot retarget confirmations, detached presenters, launch preparation or retries; update command docs.
- [x] 2.4 Preserve gates, bounded shared-action FIFO, receipts and coherent publication adoption without global selection restoration. A local move immediately followed by a command is covered by the capture test; non-following selection surviving adoption is covered by the adoption test; gate fencing itself is unchanged from the previous slice.
- [x] 2.5 Regenerate public contracts and update the wire revision for the incompatible UI contract. Obsolete selection fields/actions are removed with no compatibility interpreter; the wire revision is 13 and `RADAR_MINIMUM_REVISION` matches. Contract generation, the generated-file check, the typecheck and 133 SDK tests pass; `docs/control-plane.fixture.jsonl` carries the new revision. The changelog states the matching daemon/frontend requirement.

## 3. Focus display and optional following

- [ ] 3.1 (drawn but untested) Render a focus marker distinct from local selection, plus unavailable/unsupported/outside-listing indications. A focused row ends with `◆` and the tree heading names `following`, `no operator`, `multiple operators`, `focus outside listing` or `focus unavailable`; a focused pane resolves to its current agent row, else its pane row, and a retained or history occupant is never marked. No render test covers the marker or the indications yet.
- [x] 3.2 Add a local follow toggle using existing binding conventions: sidebar on by default, dashboard off. `o` toggles it, documented in the README key table and changelog. Covered by unit tests for focus changes, browsing being preserved and the off case.
- [x] 3.3 Handle focus becoming representable after delayed projection without repeatedly overwriting deliberate browsing. Covered by a unit test: an unrepresentable focus stays pending, completes once when its row appears, and does not repeat after browsing away. A new attachment resets only the local follow baseline. Graced-history expiry has no separate test.

## 4. Integrated validation and acceptance

- [ ] 4.1 Run fmt, strict Clippy, locked Rust suites, SDK tests, generated-contract checks and strict OpenSpec validation; record exit codes. Run Nix evaluation/build checks only for affected packaging. No skipped failing tests or claims of live acceptance from unit tests.
- [ ] 4.2 Exercise two Radar views sharing one operator tmux client: independent details/selection, native focus tracking, following on/off, frozen confirmation targets and zero-session launch. Test sustained navigation/focus and notification bursts for input responsiveness, bounded work and absence of obsolete focus-command backlog; record results and failures.
- [ ] 4.3 Complete operator live Kitty/nested-tmux acceptance: observer does not resize/change selection, native navigation updates focus, browsing does not switch terminals, Enter focuses, follow behaves on reconnect, daemon shutdown leaves no observer child and server replacement recovers. Keep this unchecked until actually run.

## Workflow follow-up

- Sync/archive predecessors in dependency order before this change: sidebar-mode/shared-ui-session, then unified-tui-publications, then duplex-tui-attachments. This change modifies capabilities introduced there; validation success alone does not mean archive is currently safe.
- Global inner-server hotkeys, sidebar pinning/show-hide/width, recovery, OSC 133 facts and exit-tracking replacement remain separate changes.
