# Tasks

## 1. Development skeleton

- [x] 1.1 Create a single Cargo binary package with the minimal Ratatui/Crossterm/Serde dependencies and lockfile; verify `cargo check` succeeds.
- [x] 1.2 Add a pinned nixpkgs flake, lockfile, development shell, `.envrc` and appropriate ignore rules; verify `nix develop --command cargo check` and that the shell supplies rustfmt, Clippy and rust-analyzer.
- [x] 1.3 Document entering the shell, building, running, testing and the separately installed Herdr prerequisite; verify the documented development commands work.

## 2. Fixture-driven overview slice

- [x] 2.1 Add sanitized snapshot fixtures and connector-local decoding into distinct runtime locations, reported session identities, lifecycle and optional semantic facts; verify tests cover a real-shaped inventory, extra fields, missing lineage, successful empty inventory and malformed required inventory.
- [x] 2.2 Build the workspace/ownership tree from normalized observations, with tabs as details and optional ordinary panes without duplicate pane rows; verify projection tests cover explicit ownership across tabs, unavailable ownership and safe fallback for invalid links.
- [x] 2.3 Render the tree and selected detail panel with current/retained/stale distinctions and unavailable role/assignment fields; verify Ratatui test-backend output reflects normalized fixtures without inspecting Herdr tokens in presentation code.
- [x] 2.4 Implement selection, folding, text filtering with matching ancestors, and the ordinary-pane toggle; verify interaction tests cover a nested match, restoring fold state after clearing a filter, stable selection on refresh and selected-row removal.
- [x] 2.5 Wire terminal initialization, resize handling, quit and cleanup around the fixture-driven view, sanitizing runtime text at the display boundary; verify a terminal smoke test restores input/display modes and a rendering test neutralizes control-sequence text.
- [x] 2.6 Document the initial key bindings, hidden-by-default ordinary panes and missing-metadata presentation; verify the help/documentation agrees with the implemented interactions.

## 3. Local Herdr collection

- [x] 3.1 Connect the application to `herdr api snapshot` through a non-blocking collector with one collection in flight, bounded command execution and child cleanup; verify a fake executable exercises valid JSON, nonzero exit, malformed output and a stalled command without blocking input or quit.
- [x] 3.2 Apply successful observations and preserve last-good inventory with a stale diagnostic on failure; verify tests cover initial failure, failure after success, a successful empty inventory and recovery.
- [x] 3.3 Document the one-instance scope, polling defaults, source diagnostics and the difference between Herdr lifecycle and Herdsman semantic state; verify the running dashboard matches the documented failure/recovery behavior.

## 4. Pane-backed continuity

- [x] 4.1 Add targeted `herdr pane process-info --pane <id>` collection only for missing-agent continuity candidates, interpreting shell/foreground PID evidence inside the connector; verify fixtures distinguish a foreground shell, a non-shell command and unavailable/inconclusive evidence.
- [x] 4.2 Reconcile in-memory agent associations through idle-shell gaps, refreshing returning agents and clearing associations on pane loss or proven supersession; verify transition tests cover idle shell, inconclusive evidence, same-session return, new session, non-shell replacement and successful pane disappearance without treating source failure as disappearance.
- [x] 4.3 Integrate retained observations into tree/details without duplicate ordinary-pane rows or current-status claims; verify rendering tests show retained last-observed facts and source-wide stale state distinctly.
- [x] 4.4 Document memory-only continuity and its polling/source-evidence limits; verify restarting Radar does not restore retained records and the documentation does not promise historical outcomes or durable session recovery.

## 5. Integration acceptance

- [x] 5.1 Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` and `nix develop --command cargo build --locked`; verify all checks pass without unused abstractions, temporary fixture modes or debug configuration left in production.
- [x] 5.2 Smoke-test the full-screen dashboard against the local Herdr instance, checking workspace inventory, optional ordinary panes, navigation/filtering, available metadata and honest unavailable fields; record the observed result and source/version used, without requiring changes to external repositories. Operator accepted the live smoke on 2026-10-06 ("smoke is accepted"); the tested runtime version was not supplied.
- [x] 5.3 In disposable runtime panes or a controlled fake runtime, exercise agent-to-idle-shell continuity, replacement command, pane closure and source failure/recovery; verify responsive quitting and restored terminal modes, and record any source limitations rather than weakening freshness claims.

## 6. Herdsman agent facts

- [x] 6.1 Carry the Herdsman pane-metadata facts on an agent observation — role, published name, definition, run, request, pending ask, assignment text, assignment start, owner-projected state, model, provider, thinking level, context usage and session name — and derive a single state that prefers the owner's projection and falls back to the runtime's reported status; verify the decode against Herdsman's published `pane-metadata.fixture.json`, including its flattened records, its owner-state expiry and its consumer fallback.
- [x] 6.2 Draw the row facts (assignment age where a start exists, model and thinking level where known) and put the remaining facts in the details panel, labelling the owner's projection and the runtime's status distinctly; verify rendering tests cover a worker with an assignment, an agent without one, a projected state disagreeing with the runtime, and absent facts.
- [x] 6.3 Adopt Herdsman's state vocabulary and marks (`◷` waiting, `◐` blocked, `◌` settling, `×` lost, `?` unknown), animate settling as Herdsman's widget does with a per-state animation setting, and give waiting, blocked and settling their own colour roles; verify `--print-config` round-trips the new roles and animations, and that an unrecognised state value is shown rather than coerced.
- [x] 6.4 Draw the row's state word from the pane's activity state and show the owner's projection as its own labelled detail line, with the footer's toggle hints carrying their state word again (`d hide details`/`d details`, `e hide finished`/`e finished`); verify tests cover a projection disagreeing with the derivation, a fresh owner `lost` still winning, an unknown native state staying unknown, a row with no projection drawing no assignment line, and the hint line fitting 100 columns.

## 7. Pane rows carry their own mark

- [x] 7.1 Give an ordinary pane row two mark columns, as an agent row has: the leading column says what the pane is doing now (the running `command` frames while a command runs, the pane's own mark — Nerd Font dev-terminal glyph, text fallback `▭` — when nothing does, so a pane row is never a blank column beside a marked agent row), and the second carries the program's own mark where `[processes]` has one, the terminal-mode mark (`▣`, `❯`, `·`) where it does not. Verified by a rendering test that a commandless pane leads with the pane mark, that a running command's mark advances with the clock, and that the two columns survive the fixture; plus theme tests that a shipped mark is a Nerd Font codepoint and a user's plain character needs no font.
- [x] 7.2 Add one `[appearance] command` setting shared by the marks that mean a process is running — an ordinary pane's running mark and the mark beside an agent row's background count, drawn only while at least one unresolved task is running. `none` keeps them still, a pane whose command has stopped or whose tasks have all exited into `review` does not animate, and `--print-config` lists the setting. Verified: `cargo fmt --check`, Clippy with `-D warnings`, the locked suite (9 suites, 139 library tests) and both PTY smoke scenarios pass.
