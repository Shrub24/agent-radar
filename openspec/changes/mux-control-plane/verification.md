# Verification

## Slice 1.1–1.2 — accepted

Primary-session source gate on the corrected working tree (2026-10-08):
`nix develop -c bash -c 'cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked && cargo build --locked'` exited 0. Library tests: 319; control-plane integration: 15/15; all existing integration/doc-test targets passed.

Review checked bounded synchronous connection-worker execution, exact JSON-content deduplication, fail-closed private record reads and inode-bound socket cleanup. Initial review found unbounded detached operation threads, unbounded/silently skipped record reads and unconditional socket unlinking; those were corrected before acceptance. Request records can contain literal terminal input/metadata and are private, not redacted.

Primary experiments against the newly built `target/debug/radar daemon`, using temporary socket/state directories only:

- A client sent a large unknown method and did not read its response; SIGTERM completed in 0.199 seconds and removed the owned socket.
- Replacing the old daemon's socket pathname with another listener, then stopping the old daemon, left the replacement socket intact.
- A loose-permission state directory refused startup and left no owned socket behind.

No Herdr/tmux calls, agent operations, TUI changes, live-fleet destructive checks, checked Nix gate or final change acceptance are claimed by this slice. Backend and client integration remain pending.

## Slice 1.3 and 2.1 — accepted

Primary-session gate on the corrected tree: `cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked && cargo build --locked` exited 0 (319 library tests; control-plane 21/21; runtime 21/21; focus, lifecycle, managed and existing suites green).

Review of `server::execute` found every focus `Err` recorded as `refused` unless shutdown was in progress, so a lost reply or timeout after possible dispatch would have been reported as a known refusal. Corrected before acceptance: focus certainty is typed `Completed`/`Refused`/`Unknown` at the runtime seam; a Herdr CLI failure (which both connects and dispatches) is `Unknown`; a pane-focus socket `error` reply or a pre-dispatch connect/setup failure is `Refused`; an `Unknown` record keeps its target suppressed and is never replayed.

Evidence: injected fake backend over the real daemon socket (observe, process_info, focus, exact-ID replay, changed-content refusal, uncertain focus, Started record visible while a second connection reads, stop cancelling into a durable unknown) and the actual `HerdrRuntime` bound to the daemon against a disposable scripted `herdr` and stub socket.

Limits: no live Herdr server was contacted; unknown outcomes use the `backend_unavailable` category until a dedicated category lands; the checked Nix build and PTY smokes are not run for this slice (task 4.1). Guarded close, create, input/output, metadata reporting, the Radar client and docs remain pending.

## Slice 2.2 — guarded close accepted

Primary-session gate: fmt, strict Clippy, `cargo test --locked` (319 library; control-plane 29/29; runtime 22/22; lifecycle unchanged and green) and build exited 0. Review of `execute_close`: the daemon deserializes Radar's own frozen `CloseRequest`, re-reads fresh inventory and runs the same `lifecycle::close_unmanaged`/`guard` as the direct client, so local and daemon closes share one gate and one freeze. Started is recorded before dispatch; Herdr CLI failures are `Unknown` and stay suppressed; a confirmed close is recorded as completed with a caveat that removal was not re-observed.

Limits: the guard verdict and the close are not atomic; the daemon never re-reads post-state; `CloseOutcome::Refused` has no Herdr producer today; no live Herdr or fleet was contacted (fakes plus a scripted `herdr` stub only); managed close and restart stay owner-routed through Herdsman and are not served here.

## Slice 2.3 - create, input and bounded output accepted

Primary-session gate: fmt, strict Clippy, `cargo test --locked` (320 library; control-plane 36/36; runtime 22/22; 528 total) and build exited 0. Review findings corrected before acceptance: the created identity now travels as structured data on the durable record (kind plus the runtime id, absent rather than inferred, stable across same-ID replay) instead of only as effect prose; and the unbounded `Barrier` rendezvous that parked the gate for 40 minutes was replaced with bounded handshakes that name the missing side, with a missed-rendezvous regression.

Two real defects were found by verification rather than by the author: the Herdr read decoder expected `/result/text` while the published schema nests the read under `pane_read`, which a live server would never have satisfied (the fixtures had been invented shapes and are now schema shapes), and the input validator named the wrong field.

Evidence: fake backends over the real daemon socket for every operation plus wire-shape validation, same-ID at-most-once and unknown suppression; the real `HerdrRuntime` composed with the daemon against a scripted `herdr` and stub socket for create, input and read.

Limits: no live mux or Herdr server was contacted; the capability gate is uniform and declaration-driven, so a provider that does not declare an operation is refused it; `CloseOutcome::Refused` still has no Herdr producer; the guard verdict and the close remain non-atomic. The daemon subcommand test can still leak its child if the test aborts before the signal, which the next slice hardens.

## Slice 2.4 - metadata reporting bridge accepted

Primary-session gate: fmt, strict Clippy, `cargo test --locked` (326 library; control-plane 42/42; runtime 22/22; 540 total) and build exited 0; no daemon child leaked.

The bridge forwards a caller's own state/session/display facts through `pane.report_agent`, `pane.report_agent_session`, `pane.report_metadata` and `workspace.report_metadata`, grounded in the installed `herdr 0.9.3` help and schema, and cross-checked against the real callers in pi-extensions, pi-subagents and pi-bash-processes. Unknown fields and unknown kinds are refused rather than ignored, and `resume_argv` is not forwarded, so the daemon never becomes the place a replayable resume command is registered. A backend without reporting refuses the operation and advertises nothing.

Limits: offline only (fakes, scripted `herdr`, stub socket); the state vocabulary is the schema's four words, so a newer backend word refuses `bad_params`; workspace display metadata has no consumer in the tree yet; `release_agent` and `clear_agent_authority` are deliberately not bridged. Section 2 is complete.

## Slice 3.1 - daemon client and explicit selection accepted

Primary-session gate: fmt, strict Clippy, `cargo test --locked` (331 library; control-plane 52/52; runtime 22/22; 555 total) and build exited 0.

Radar selects the configured backend before collection or action. The default remains direct. A daemon handshake requires protocol 1 and the capabilities needed by Radar; failure emits one stderr diagnostic and selects the direct adapter before any action. After selection there is no direct retry following a possibly dispatched operation. Managed close/restart still use their Herdsman owner path. Config supports `[runtime] backend = direct|daemon` and an optional socket override; Radar does not start/supervise the daemon.

Before any connect, client and daemon share the trust rule: socket path is a real socket, not a symlink, owned by the current uid; parent is a real current-user directory with the daemon's exact mode 0700. Untrusted and operator-supplied paths fail before dialing and use the single direct-fallback diagnostic. Tests prove an active planted daemon is never connected. The check rejects a 0755 parent even though it is not group/other-writable; this is intentionally the exact bind rule, so client accepts no path the daemon itself would refuse.

Limits: no live mux or fleet contacted; no collection/action is possible in the manual shipped-binary smokes because the environment lacks a TTY; unknown socket-file owner branch cannot be constructed without root, while the foreign parent branch is tested via `/tmp`.

## Slice 3.2 - protocol reference accepted

Owner verification: fmt, strict Clippy, `cargo test --locked` (331 library; all integration/doc targets green), `cargo build --locked`, and `openspec validate mux-control-plane --strict` all pass. `docs/control-plane.fixture.jsonl` parses as 24 valid JSON lines. Reviewed `docs/control-plane.md` for the supported method set, path/trust rules, reserved-but-unemitted busy code, input/record privacy, capability negotiation, uncertainty semantics and curated report schema. README now links the protocol reference and truthfully separates physical mux wrappers from deferred session authority/recovery; the operator starts the daemon.

Limit: the JSONL fixture is illustrative rather than captured golden output; keep it aligned with protocol tests. No source code changed in this slice.

## Slice 4.1 - final acceptance gate: PASSED

Sub-agent gate on the committed workstream (jj change `pktkyszm`, "Add the mux control plane: a Radar-owned daemon and backend-neutral runtime"), `rustc 1.98.1`, `cargo 1.98.0`, `nix (Nix) 2.35.2`, 2026-10-08 UTC. No live fleet/mux. Sandbox builds ran on `ssh-ng://dev@home-forge`.

The two blockers recorded for the previous failing gate are cleared:

- `src/control_plane/client.rs` and the other new files are now tracked in git (staged intent-to-add), so the git-backed flake source `/nix/store/mid5v4d96iz7xj1zn6ig0an6q9x0jzpn-source` includes the whole workstream and the sandbox compiles. `nix/package.nix` already covered `../src`; the prior omission was purely VCS tracking, not a package-input gap.
- `a_socket_path_nothing_vouches_for_is_refused` no longer assumes `/tmp` is owned by uid 0: it exercises the directory-ownership rejection only when the system temp directory's uid differs from the current uid, while unconditionally keeping the mode `0770`, symlink and non-socket refusals. Fix made by the owner in `src/control_plane/server.rs`; verified here, not otherwise edited.

Checked Nix, from the committed source:

- `nix flake check -L` exit 0 - "all checks passed!" (`checks.x86_64-linux.package`, derivation `/nix/store/3mx56cgvn0ml14qar692x6irln0jigq5-agent-radar-0.1.0.drv`).
- `nix build .#radar --print-out-paths -L` exit 0 - output `/nix/store/1dsfkgcj399cygai2m35l2gggr7yk5h9-agent-radar-0.1.0`.
- Sandbox `checkPhase` runs `cargo test`, so the checked build is a testing gate: 331 library tests (including the portable trust test), `control_plane` 52, `runtime` 22, `lifecycle` 12, `managed` 9, `nested_tree` 37, `stale_binary` 19, `bus` 20, `bus_detail` 17, `control` 18, `collector` 12, `focus` 4, `animations` 1, `descendants` 1, doc-tests 0 - all passed, 0 failed.

Re-confirmed on this tree (a test-only correction; no production behaviour changed):

- `cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked && cargo build --locked` exit 0 (331 library tests; integration suites green).
- Both PTY smoke scenarios: `python3 tests/terminal_smoke.py target/debug/radar` exit 0; binary sha256 `4b1308101771784bbad8ffc9f8385ccf0b33047759088bd2c58dddbba8fd17df` is unchanged from the previous gate, consistent with a test-only edit.
- `openspec validate --all --strict`: 14 passed, 0 failed.
- `docs/control-plane.fixture.jsonl`: 24 records re-validated - 12 requests (version 1, string method, object params), 12 responses (exactly one of result/error, id matching an earlier request), one `unknown_method` error on the last line. Structural check only; still illustrative, not captured golden output.

Limits: no live fleet or mux was contacted; the PTY smokes are local harness runs over controlled fakes; the JSONL fixture remains illustrative rather than captured golden output. Nothing was pushed.

Verdict: 4.1 is met - fmt, strict Clippy, locked tests/build, both PTY smokes, the checked Nix build and strict spec validation all pass - and the change is accepted. Follow-up: keep the illustrative JSONL fixture aligned with protocol tests.
