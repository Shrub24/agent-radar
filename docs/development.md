# Development

The flake uses current nixpkgs-unstable, recorded in `flake.lock`. It supplies
Cargo, Rust, rustfmt, Clippy, rust-analyzer and Python for the terminal regression
check. Rust 2024 and Rust 1.88 or newer are required outside the shell.

```sh
direnv allow                  # with nix-direnv; once for this checkout
# or
nix develop

cargo run --locked            # launch radar; q quits
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo build --locked
python3 tests/tmux_acceptance.py target/debug/radar
```

Radar's control daemon and its tmux server run as user units
(see [the README](../README.md#the-daemon-and-the-tmux-server)), and `radar` starts them. Start with the [daemon consumer guide](daemon.md)
for the API, lifecycle and adoption path. The [mux integration contract](mux.md)
covers physical guarantees, backend responsibilities and what a backend may promise.
See the [control-plane protocol and extension examples](control-plane.md)
and the direct [agent-registration
contract and publisher example](agent-registration.md). Version control
uses **jj**.

## Layout

One Cargo package, with `radar` as its binary:

- `src/main.rs`: terminal lifecycle and event loop.
- `src/runtime.rs` / `src/tmux.rs`: the runtime seam and its tmux adapter.
  Collection asks the seam for a normalized inventory and per-pane foreground
  evidence, and focus asks it to move to a normalized workspace or pane target;
  the adapter owns tmux's executable, server selection, format strings, wire
  decoding and the bounded runner that kills and reaps a stalled command.
  Managed-agent lifecycle controls go through the daemon's semantic lifecycle
  methods; they do not use the in-process adapter or an owner-file request.
- `src/collector.rs`: the refresh schedule the daemon's sampler drives. One
  refresh in flight at a time on its own thread, cancellation that abandons
  rather than waits, the assignment-age stamp taken once per refresh, and the
  local `/proc` facts composed into the evidence the adapter returned. Sources
  fail independently: an inventory error is a stale fleet that keeps the
  last-good inventory, a registry error is reported as the registry and leaves
  the fleet standing, and unreadable foreground evidence is inconclusive rather
  than proof that a pane went away.
- `src/control_plane/observation.rs`: the daemon's one observation sampler and
  the immutable revisioned snapshot every reader shares. It samples only while a
  reader or active TUI attachment has asked, and publishing appends nothing to the journal.
- `src/ui_session/publication.rs` and `src/ui_session/client.rs`: observation,
  tree and UI-session payloads adopted under one coherent push head. The TUI
  collects nothing itself and has no parallel observation-fetch connection.
- `src/content.rs`: shared bounded MessagePack encoding, XXH3-128 content,
  immutable byte retention and segmented framing; no UI-domain dependencies.
- `src/bus.rs` and `src/control_plane/ingest.rs`: publisher protocol decoding and daemon-owned volatile task ingestion; `src/control_plane/transport/` provides the shared bounded socket runtime.
- `src/model.rs` / `src/observation.rs`: normalized facts and reconciliation.
- `src/config.rs` / `src/theme.rs`: the colours, where they come from, and the
  vocabulary they paint (state marks, vendor icons, title rules).
- `src/tree.rs` / `src/app.rs` / `src/ui.rs` / `src/title.rs`: projection,
  interactions, rendering and title normalisation.
- `src/focus.rs`: the focus worker run off the calling thread, its cancellation
  and message plumbing, and what `Enter` sends. The two transports behind it
  belong to the adapter.
- `src/lifecycle.rs`: the containment policy the daemon and the views share.
  Every lifecycle action itself — close, restart, cancellation — is the daemon's.
- `src/timestamp.rs`: the contract's ISO 8601 timestamp parsed to Unix milliseconds.
- `tests/`: sanitized fixtures, fake-executable and stub-socket transport
  checks at the adapter, collector and focus tests against an in-memory
  runtime, bus listener and detail-join tests against a stub publisher, and
  a focused PTY check that also connects one to the running dashboard, plus
  `tests/tmux_acceptance.py`, which runs the dashboard against a disposable
  daemon on a disposable tmux server and asserts the registry-backed rows. No
  real tmux server is modified by these tests, and no real lifecycle request is
  written.

`Cargo.lock` and `flake.lock` record reproducible dependency versions. Specs and
the reasoning behind decisions are in `openspec/` and `context/`.

Manual acceptance: run `cargo run --locked` against your tmux server. Check the
workspace inventory, cycle the pane views, navigate/fold/filter, show and hide the
details, and inspect location, ownership, freshness and unavailable metadata.
Press `Enter` on an agent row, a workspace row and a retained row, and confirm
the focus lands where the row says; then the same with tmux stopped, which
must leave Radar usable and say why on the last line. Real
runtime/UX acceptance is deliberately manual; automated checks use controlled
fixtures and executables.

## Logs

The daemon logs to stderr, which its user unit sends to the journal (`journalctl --user -u radar-daemon`). The dashboard owns the terminal, so it logs to `tui.log` beside the daemon's state directory (by default `~/.local/state/agent-radar/tui.log`, started afresh past 2 MiB). The footer shows one line cut with an ellipsis; a failure stays until `c`, and the whole text is in the log. `RADAR_LOG` takes a filter such as `debug`. Log lines carry identifiers and outcomes, never a command or an environment.
