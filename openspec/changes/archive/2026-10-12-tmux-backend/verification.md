# Verification

## Source and behavioral checks

- `cargo fmt --check` — passed.
- `cargo clippy --locked --all-targets -- -D warnings` — passed.
- `cargo test --locked` — passed; all unit, integration and doc-test suites passed, including the tmux adapter and control-plane integration tests.
- `cargo test --locked --test control_plane tmux_spawn_exit_persists_and_dead_child_close_is_refused -- --exact --nocapture` — passed against an isolated named tmux server. The test launches and binds a child, confirms the typed exit through `spawn.get` and `spawn.list`, verifies execution/assignment publications are unchanged, confirms dead-pane `child.close` refuses before writing a close record, and reads the exit after daemon restart.
- `python3 tests/terminal_smoke.py` — both controlled PTY scenarios passed, including terminal restoration and the bus-off path.
- `python3 tests/agent_publisher.py` — passed the disposable-daemon registry/event, spawn binding, topology and managed-close fixture checks.
- Both `docs/control-plane.fixture.jsonl` and `docs/agent-registration.fixture.jsonl` parse as JSON Lines.
- `openspec validate --all --strict` — passed, 19/19.

## Nix provenance

- `nix flake check -L` — passed on `x86_64-linux`; the package check compiled the crate and ran the locked Rust suite. Nix omitted `aarch64-linux`, `x86_64-darwin` and `aarch64-darwin` as incompatible systems.
- `nix build .#radar --print-out-paths -L` — passed; output `/nix/store/43qs4hmg3i8xl8c1fghn5cdqh841gsa6-agent-radar-0.1.0`.

## tmux status replacement experiment

On tmux 3.7c, an isolated named server retained `pane_dead_status=23` after the pane command exited. An external `respawn-pane -k` on that same pane, with a command exiting 17, changed the retained status to 17. This confirms that a same-UID client can replace an unread exit observation. The adapter therefore reports tmux pane-command status, not independent proof of the bound agent's process incarnation; the same-UID limitation is documented in `design.md` and `docs/mux.md`.

## Limits

- Herdr remains the default physical backend. The tmux backend requires explicit `RADAR_CONTROL_BACKEND=tmux` and `RADAR_TMUX_SERVER=<name>` selection; Herdsman adoption and direct Herdr removal are not part of this change.
- Exit observations are available only when a supporting backend reports a retained exit for a current v2 daemon-spawn edge. Herdr and missing panes remain unknown/unresolved; legacy v1 edges omit the field. Exit facts do not update execution or assignment publications and are not delivered as backend-push events.
- A recorded exit does not authorize dead-pane close: `child.close` still requires live foreground process birth-identity evidence and refuses when that evidence is absent.
- No process stop, resume, restart, recovery, live Herdr workspace or user tmux server was used or added.
