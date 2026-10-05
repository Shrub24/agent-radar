# Verification

## 1.1 Collection through the provider (owner gate, serial)

- `cargo fmt --check`: pass.
- `cargo clippy --all-targets --locked -- -D warnings`: pass.
- `cargo test --locked`: 251 passed, 0 failed (lib 156, collector 9, runtime 8, descendants 1, focus 7, bus 20, bus_detail 12, nested_tree 37, animations 1).
- `cargo build --locked`: pass.
- `python3 tests/terminal_smoke.py`: both scenarios pass.
- `openspec validate runtime-provider-seam --strict`: valid.
- `src/collector.rs` holds no Herdr command names or wire decoding (one doc sentence names what does not live there). Remaining `herdr` mentions in `app.rs`/`tree.rs`/`ui.rs` are test fixtures and message strings.

## 2.1 Focus through the provider, and 3.1 integration (owner gate, serial)

- `cargo fmt --check`: pass.
- `cargo clippy --all-targets --locked -- -D warnings`: pass.
- `cargo test --locked`: 255 passed, 0 failed (lib 156, collector 9, runtime 15, focus 4, descendants 1, bus 20, bus_detail 12, nested_tree 37, animations 1).
- `cargo build --locked`: pass.
- `python3 tests/terminal_smoke.py`: both scenarios pass.
- `openspec validate runtime-provider-seam --strict`: valid.
- Search of `src/focus.rs`, `collector.rs`, `app.rs`, `tree.rs`, `ui.rs`, `main.rs` and `runtime.rs` for sockets, Herdr command arguments, `pane.focus`, `Command::new`, `herdr::run` and `FocusConfig` finds one test fixture string in `ui.rs`. All Herdr transport sits inside `src/herdr.rs`.
- No destructive action, owner mailbox, dependency, source-selection setting or observable behaviour was added. `tests/focus.rs` no longer execs scripts, so its write-then-exec flake is gone; the adapter's exec-ing tests are serialized in `tests/runtime.rs`.
