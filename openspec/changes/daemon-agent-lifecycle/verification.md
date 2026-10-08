# Verification

## Registration foundation (1.1–1.2)

Primary-session gate on the corrected tree: fmt, strict all-target Clippy, cargo test --locked (339 library tests; agent_registry 17/17; existing suites green), cargo build --locked; exit 0.

Reviewed immutable registration/public projection, strict local decoding of process claims, serialized find/create admission, persistence/privacy tests and stable UUID ordering. Initial unplanned state field and permissive nested process input were corrected before acceptance. Unknown execution vocabulary belongs to publication channels and will be verified in 2.1, not stored as an immutable registration fact.

No daemon endpoint or live publisher is claimed. Tests use temporary private roots. One owning registry instance is supported; cross-instance writers are unsupported. The concurrency test currently uses a start barrier; replace it with bounded rendezvous during the next slice so the suite cannot repeat the previous control-plane gate hang.

Following slices: publication/freshness, process verification, endpoints, producer contract and independent Nix/PTY integration gate.

## Publication and writer fencing (2.1)

Primary-session fmt, strict Clippy, locked tests/build passed (340 library; registry 26/26). Reviewed single-record acquire/publish transitions and epoch freshness. Channel binding and optional accepted snapshot are atomic; snapshot provenance remains immutable on replacement. Publisher identity is separate from the subject registration, old handles are fenced, equal snapshot replay does not refresh the lease, and reopening uses a new serving epoch regardless of wall-clock equality/rollback. Temporary-root tests only; no endpoints or process verification yet.

Limit: the write-failure regression uses directory permissions, which root bypasses. Make this test portable before the final gate; do not weaken the production trust rules.

## Process verification (2.2)

Primary-session fmt, strict Clippy, locked tests/build passed (341 library; registry 29/29). Reviewed local procfs verifier: boot/PID/start tuple compared in full, missing process directory yields absent, unreadable/malformed evidence yields unavailable. Registry registration is read before external verifier I/O, which does not hold the admission lock. Tests keep process evidence and publication freshness independent and prove responsiveness with a blocking verifier. Failure injection is now private/unit-test-only, avoiding root-dependent permission assumptions.

No endpoint, live publisher or physical lifecycle execution is claimed. Local process verification covers only the daemon host/process namespace.

## Registry endpoints (3.1)

Primary-session fmt, strict Clippy, locked tests/build passed (341 library; registry 29/29; control-plane 58/58). Reviewed daemon-wide verification admission: four outstanding jobs, permit retained until completion despite timeout, tracked/reaped handles, and bounded shutdown without joining blocked injected verifiers. Worker captures only claimed process identity, not private launch records. Public reads omit fencing handles and launch contents; acquisition returns writer binding to its caller. Stable pagination is bounded by count and complete encoded response bytes, including escaping.

Socket tests use disposable private roots and injected sources, not a live Herdsman publisher. A permanently blocked verifier can retain one of four slots and outlive daemon stop, with no registry/socket capability. Existing 52-test baseline was recovered from jj operation c483f96627ab following an accidental test-file truncation, then passed before endpoint additions.

## Publisher contract/example (3.2)

Primary session executed cargo build --locked, python3 tests/agent_publisher.py, Python byte-compilation and strict change validation; exit 0. Reviewed producer trust checks, immutable registration/reacquire flow and sequence replay. Smoke compares canonical fixture exchanges to real disposable-daemon response schemas/content, verifies private launch sentinel omission, generation fencing, expiry/replacement, retirement and restart freshness/reconnect. No Herdr calls or live Herdsman publisher integration. Production extensions must preserve appropriate publisher identity/sequence across reconnects; this example retains identity for its process lifetime only.

## Final independent gate (4.1) — accepted

Primary-session commands, certified exit 0:

- `nix develop -c bash -c 'cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked && cargo build --locked && python3 tests/terminal_smoke.py target/debug/radar && python3 tests/agent_publisher.py && openspec validate --all --strict'` (bg-1096). All Rust suites green (341 library, registry 29, control-plane 58); both PTY scenarios and disposable publisher/fixture smoke passed; strict validation 15/15.
- `nix flake check path:. -L && nix build path:.#radar --print-out-paths -L` (bg-1097). Native x86_64-linux checked package, including sandbox cargo tests, passed. Derivation `/nix/store/lp3hk910175yvvjl2z3cyxp9mkjfx4fn-agent-radar-0.1.0.drv`; filtered Rust source `/nix/store/p0rwp7gxi3h7wcxcv8r4rb69kzpycrvd-source`; output `/nix/store/mllmk661zw75hsg7rryrjk6pwawi3nzp-agent-radar-0.1.0`. Remote builder `ssh-ng://dev@home-forge`.

Certified complete logs under `/tmp/kendex-pi-bg/lanes/01a10a58-c4c1-7570-a8ba-a8b0ca9e8d91/`: `bg-1096-1791487324726.log` and `bg-1097-1791487350041.log`.

Explicit path input includes new files without staging/committing; this is not GitHub/git-backed snapshot verification. Known Nix warnings: unknown homeManagerModules output, omitted non-native systems. No aarch64 build claimed. Post-gate edits only record acceptance/planning and do not affect the filtered Rust source.

All seven tasks accepted. No live fleet/Herdr actions, Herdsman producer migration, TUI registry integration or launch/stop/resume execution claimed. This change has not been committed, pushed, synced or archived.
