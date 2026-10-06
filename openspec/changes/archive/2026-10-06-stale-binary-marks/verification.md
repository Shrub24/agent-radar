# Verification

Owner verification after the delegated detection, correction and presentation slices:

- `cargo fmt --check`: pass.
- `cargo clippy --all-targets --locked -- -D warnings`: pass.
- `cargo test --locked --quiet`: 278 passed, 0 failed (167 library, 1 animation, 20 bus, 12 bus detail, 10 collector, 1 descendants, 4 focus, 37 nested tree, 15 runtime, 11 stale binary).
- `cargo build --locked`: pass.
- `python3 tests/terminal_smoke.py`: both scenarios pass, including source recovery, publisher updates, stalled shutdown, mouse-capture release and terminal restoration.
- `openspec validate stale-binary-marks --strict`: pass.

Checks ran sequentially in the owner session. Review found and corrected PATH lookup choosing non-executable files or directories; injected-path regressions cover this. The final scope contains no adapter, runtime trait, bus, lifecycle or dependency change.

The warning uses Nerd Font U+F071 when available and `!` otherwise, coloured by `colors.stale` (default ANSI yellow). Details retain exact installation identities without parsing versions. Unavailable comparisons, retained agents and stale source observations make no binary claim. Tests cover detection, executable lookup, collection and rendering; no real session was restarted or closed.
