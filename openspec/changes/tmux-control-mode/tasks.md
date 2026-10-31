# Tasks

## 0. Audit and verify prerequisites

- [ ] 0.1 Inventory shared-selection writers/readers, navigation actions, publication adoption, command target defaults, configured Launch and gate reconciliation. Record which paths become focus and which move local, with no surviving shared browsing cursor; deliver the inventory in design.md.
- [ ] 0.2 (serving-version floor enforced in the adapter; packaging audit and Home Manager assertion outstanding) Audit serving-version enforcement, Home Manager tmuxPackage and flake/check tmux versions. Require the serving server to report 3.8+ and add only missing packaging support/assertions. Verify old-version refusal and version parsing, module evaluation and relevant Nix builds if packaging changes; update version docs.
- [ ] 0.3 (partly done, see verification.md) Record reproducible disposable-3.8 experiments in verification.md: server-wide client/session/pane/layout events, zoom, zero-session attach, read-only/ignore-size/no-output invariance, destroy-unattached, detach-on-destroy and loss of the observer's attached session. Confirm one-observer lifecycle without creating a placeholder or mutating operator state.

Stop point: finish the lifecycle/flag checks before relying on a persistent observer. The server-wide shape is chosen; do not introduce a per-session compatibility fallback.

## 1. Observer and current typed focus

- [x] 1.1 Add bounded notification ingestion and a coalesced refresh request through the existing sampler. Prove a hint pulls a read forward, bursts remain bounded and a hint during a read causes a subsequent refresh; unknown/output notifications cannot leak pane content to logs or storage.
- [x] 1.2 Add one supervised daemon-owned observer, lazy attachment on first session, reattachment after session loss and bounded retry. Fence it to the owned serving incarnation and cancel/reap on shutdown. Real-tmux checks cover zero sessions, session destruction, replacement and child cleanup; document its lifecycle and non-goals.
- [x] 1.3 Build typed unified focus from current client/location/zoom reads. Exclude the observer from all operator counts and focus targeting. Report no operator, unavailable evidence and unsupported multiple operators distinctly. Verify native navigation and zoom, observer-only attachment and refusal of ambiguous focus operations without choosing a preferred client.
- [ ] 1.4 Publish baseline focus and observer health with last notification/loss reason. Preserve hints during baseline and retain timer recovery on observer failure. Verify a baseline-time transition becomes visible without unrelated input and stale focus cannot authorize implicit commands; document degraded behaviour.

## 2. Coordinated shared-focus and local-selection cutover

- [ ] 2.1 Evolve existing shared-selection publication/adoption into observed focus and extract frontend selection/anchor/navigation. Keep canonical projection, filters, folds and ordering shared. Delete shared browsing-move actions and global-cursor reconciliation in this slice. Verify two Radar views select independently against the same canonical ordering without issuing focus or moving each other; document ownership.
- [ ] 2.2 Migrate every row-dependent TUI caller, including keyboard, mouse, palette and configured Launch, to pass its local selected row. With no selection report locally; intentional omitted-row callers resolve current observed focus. Converge both paths on existing eligibility and frozen plans. Verify selected B wins over focused A, vanished explicit B refuses, rowless commands use focus and unavailable focus refuses.
- [ ] 2.3 Freeze implicit and explicit targets before preparation/confirmation; retain domain revalidation and frontend no-client recovery for explicit focus. Verify later focus/selection changes cannot retarget confirmations, detached presenters, launch preparation or retries; update command docs.
- [ ] 2.4 Preserve gates, bounded shared-action FIFO, receipts and coherent publication adoption without global selection restoration. Verify a local move then immediate command captures the new row, a gate overtakes pending replies/content safely and non-following selection survives valid gate adoption.
- [ ] 2.5 Regenerate public contracts and update the wire revision for the incompatible UI contract. Remove obsolete selection fields/actions and compatibility interpreters. Run contract generation/checks and SDK tests; document matching daemon/frontend upgrade and rollback requirements.

## 3. Focus display and optional following

- [ ] 3.1 Render a focus marker distinct from local selection, plus unavailable/unsupported/outside-listing indications. Map a focused pane to its canonical terminal/agent row without marking arbitrary tasks or history occupants. Verify selection and focus can differ, and hidden focus never moves a marker to a neighbour or rewrites shared filters/folds.
- [ ] 3.2 Add a local follow toggle using existing binding conventions: sidebar on by default, dashboard off. Enabling and reconnect baseline adoption select representable current focus; changed focused place updates following selection, never executes focus. Verify unchanged focus, zoom-only updates and unrelated publications do not undo browsing; document the binding and behaviour in help/README.
- [ ] 3.3 Handle focus becoming representable after delayed projection without repeatedly overwriting deliberate browsing. Verify one pending adoption completes when its row appears, non-following views remain independent, and local selection reconciles safely under shared filtering/folding and graced history expiry.

## 4. Integrated validation and acceptance

- [ ] 4.1 Run fmt, strict Clippy, locked Rust suites, SDK tests, generated-contract checks and strict OpenSpec validation; record exit codes. Run Nix evaluation/build checks only for affected packaging. No skipped failing tests or claims of live acceptance from unit tests.
- [ ] 4.2 Exercise two Radar views sharing one operator tmux client: independent details/selection, native focus tracking, following on/off, frozen confirmation targets and zero-session launch. Test sustained navigation/focus and notification bursts for input responsiveness, bounded work and absence of obsolete focus-command backlog; record results and failures.
- [ ] 4.3 Complete operator live Kitty/nested-tmux acceptance: observer does not resize/change selection, native navigation updates focus, browsing does not switch terminals, Enter focuses, follow behaves on reconnect, daemon shutdown leaves no observer child and server replacement recovers. Keep this unchecked until actually run.

## Workflow follow-up

- Sync/archive predecessors in dependency order before this change: sidebar-mode/shared-ui-session, then unified-tui-publications, then duplex-tui-attachments. This change modifies capabilities introduced there; validation success alone does not mean archive is currently safe.
- Global inner-server hotkeys, sidebar pinning/show-hide/width, recovery, OSC 133 facts and exit-tracking replacement remain separate changes.
