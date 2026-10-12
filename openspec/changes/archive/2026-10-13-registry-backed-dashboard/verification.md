# Verification

Task 3.1 records the final acceptance of the registry-backed dashboard. The owner
accepted tasks 1.1, 1.2, 2.1 and 2.2 before this gate; this page covers the
whole-change gate only. Source gates, smoke gates and Nix evidence are kept apart
because they establish different things: compiling and the Rust suite speak for
the code, the smokes speak for the running dashboard and daemon, and Nix speaks
for the packaged build in a sandbox that has no Herdr, no tmux server and no
Python.

## Source gates

All on one working tree, in this order:

- `cargo fmt --check` — clean.
- `cargo clippy --locked --all-targets -- -D warnings` — clean.
- `cargo build --locked` — passes.
- `cargo test --locked --no-fail-fast` — 749 passed, 0 failed, across 18 test
  targets. The count is unchanged from the accepted 2.2 gate: this change adds no
  Rust test, it adds the Python acceptance below.
- `openspec validate --all --strict` — 21 passed, 0 failed.
- `git diff --check` — clean.

## Smoke gates

- `python3 tests/terminal_smoke.py target/debug/radar` — both runs pass. The main
  run drives the dashboard from a disposable daemon with a registered worker; the
  bus-off run drives it with the direct adapter.
- `python3 tests/agent_publisher.py target/debug/radar` — full-response fixture
  comparison against a disposable daemon, the reconnecting events consumer, and
  the scripted spawn/bind/close replay, including child token binding and
  redaction.
- `python3 tests/tmux_acceptance.py target/debug/radar` — run twice on the final
  file, both exit 0. A third run pointed at a binary that does not exist fails as
  it should and still removes its server, panes and socket.

### The tmux acceptance

The dashboard is driven in a PTY against a disposable daemon whose physical
backend is a tmux server of this run's own name, started with `-f /dev/null` so
no machine configuration takes part. Its two panes each run a real foreground
process (`exec sleep 900`), so the pane's own process is the one the daemon names
and the harness claims: each registration's `process.claim` is read from the live
pane with the same procfs boot id and start ticks the verifier uses.

`herdr` on `PATH` is a fail-fast sentinel that records every invocation and exits
1. The run asserts the log is absent, so the dashboard made no Herdr call, and
that it drew no fallback diagnostic: with the daemon answering, nothing read
Herdr agent state. The daemon reports backend `tmux` and declares `registry.v1`,
`registry.facts.v1`, `events.v1` and `spawn.v1`.

Asserted on the running dashboard, not on raw endpoint reads:

- Two registrations, each named by its own location and birth identity, join
  their own pane (`physical: joined …`), and each row is named by its
  registration: the label where it has one, else the agent id it was registered
  under, never the native Herdr name it does not have.
- The three authorities keep their own values, publishers and freshness:
  `execution: done · from pi`, `assignment: working · from herdsman for
  accept-owner` and `facts: fresh · from pi` in one row.
- A session's finished execution never completes the owner's assignment: the same
  row reads `state: waiting` — derived from the open awaited items — with
  `execution: done` and `assignment: working`, and no `assignment: done` line
  appears.
- The owner's published facts reach the panel: role, definition, waiting reason,
  last outcome, awaited items, model, provider, thinking level and context usage.
- A registration that names a pane the inventory does not hold publishes nothing
  and stays unresolved and silent (`physical: unresolved`, `execution: unknown —
  nothing reported`, `facts: not reported`).
- A registration whose facts lease expires reads `facts: stale` while its
  execution stays unknown; its own pane still joins.
- Close is refused for both: a joined registration states that the pane is
  managed but publishes no session lineage, an unresolved one that it is not
  joined to a pane, and no confirmation is offered for either.
- Quitting with `q` exits 0, restores the alternate screen, releases mouse
  capture and leaves the terminal's input modes unchanged.

Resource hygiene is asserted rather than assumed: the daemon removes its control
socket when it is stopped, the dashboard removes its bus socket when it quits,
the tmux server is gone after `kill-server`, its pane processes are gone with it,
and the socket file tmux leaves behind for a dead server is removed by the
harness. The run deletes its temporary directory and fails if any of this
survives. The failure run above exercises the same path with nothing asserted.

## Nix evidence

- `nix flake check -L` — passes ("all checks passed"). It builds the package
  derivation and runs its check phase, which is the Rust suite, in the sandbox; it
  does **not** run the Python smokes, which are recorded above as separate
  evidence, as in previous changes. The check omits the incompatible systems it
  always has (aarch64-darwin, aarch64-linux, x86_64-darwin). The derivation was
  built by this machine's configured builder and the result substituted locally.
- `nix build .#radar --print-out-paths -L` —
  `/nix/store/0dc3yrag1h9slc18sdcf2wna9kwn7bdd-agent-radar-0.1.0`.
- The package source is `src/`, `tests/`, `Cargo.toml`, `Cargo.lock` and one
  fixture, so the harness's own content is part of the derivation: both Nix gates
  above ran on the frozen tree, after the last edit to `tests/tmux_acceptance.py`.
  An earlier run of the same gates, before that file's last edit, produced
  `8rnrwll5dp0n5vvqwz93rf2pw4wwxjfg-agent-radar-0.1.0` — same `src/`, older
  `tests/`.

The new source file `src/registry_observation.rs` was untracked when this gate
began, so Nix could not have seen it. `jj status` snapshotted the working copy,
after which `git ls-files` resolves it (and `tests/tmux_acceptance.py`) and the
build sees them. No commit, push, bookmark or other history change was made.

## Independent owner gate

The owner independently ran the full source gate (749 Rust tests, fmt, strict
Clippy and build), both PTY smokes and the canonical publisher fixture. The
final independent `tests/tmux_acceptance.py` run passed with zero Herdr calls
and all its resources removed. Both checked Nix commands passed and returned
the same `0dc3yrag1h9slc18sdcf2wna9kwn7bdd` package above. Strict OpenSpec
validation passed 21/21. All dashboard tasks are accepted.

## What this gate does not establish

- No live Herdr fleet is read or modified, and none is needed: the acceptance
  makes Herdr unavailable on purpose. Nothing here runs against this machine's
  tmux server, another user's session, or a Herdr workspace.
- Publishers are not exercised in production: `pi-extensions` is out of scope,
  and the acceptance registers and publishes with the repository's own publisher
  helpers. Until a publisher adopts the `facts` channel and the assignment
  extension, those rows read unavailable in a real fleet, which is accepted and
  visible.
- The tmux acceptance is the only check that joins a registration to a real pane
  through a real daemon and draws the result. The Rust suite covers the registry,
  the daemon and the projection against sandboxes, fake runtimes and disposable
  tmux servers, but it renders rows in memory rather than reading a live one.
