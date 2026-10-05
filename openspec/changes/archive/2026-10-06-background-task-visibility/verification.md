# Verification

Owner gate, run serially on 2026-10-06 after the worker's delivery:

- `cargo fmt --check`: pass.
- `cargo clippy --all-targets --locked -- -D warnings`: pass.
- `cargo test --locked`: 244 passed, 0 failed (lib 155, nested_tree 37, bus 20, bus_detail 12, collector 11, focus 7, animations 1, descendants 1).
- `cargo build --locked`: pass.
- `python3 tests/terminal_smoke.py`: both scenarios pass.
- `openspec validate background-task-visibility --strict`: valid.

The worker reported changing only `src/app.rs`, `src/ui.rs`, `tests/nested_tree.rs`, `tests/bus_detail.rs` and `README.md` for this change. The working copy also holds the earlier uncommitted nested-fleet-tree work, so the owner gate above covers both together.
