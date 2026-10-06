# Verification

## Independent acceptance

The lead verified the combined detail-page, disclosure, process-sampling and metric-display implementation, including the pre-existing PID presentation and exec-replaced-shell decoder corrections.

Commands and outcomes:

- `nix develop --command bash -c 'set -e; cargo fmt --check; cargo clippy --all-targets --locked -- -D warnings; cargo test --locked; cargo build --locked; python3 tests/terminal_smoke.py'`: exit 0; formatting and Clippy clean, 423 Rust tests passed (261 library, 162 integration), locked build passed, both PTY scenarios passed.
- `openspec validate --all --strict`: 12 items passed, 0 failed; informational long-requirement notices only.
- `nix build .#checks.x86_64-linux.package --no-link`: certified exit 0; checked package built through the configured remote builder `ssh-ng://dev@home-forge` and copied back as `/nix/store/ndfk3yqk68b90gmh536flcayvqfkimjw-agent-radar-0.1.0`.

The initial Nix invocation named nonexistent `checks.x86_64-linux.default`; it failed before building and was corrected to the actual `package` output. This is not a package failure.

PTY coverage exercises details focus, page cycling, a scroll key, Space/Enter disclosure opening and closing, Escape returning to the tree, source recovery, publisher replacement/disconnection, prompt quit, mouse capture release and terminal restoration. Render tests prove End reaches wrapped final metric content at short/narrow sizes and that disclosure targets follow actual wrapped rows after scrolling and resizing.

Sampling tests cover birth identity, CPU intervals, read failures, root and descendant PID reuse/disappearance, changed parent links and excluded subtrees, cycles, birth order, shared descendants, finite enumeration/validation budgets, cancellation and partial/unavailable coverage. Metric render tests distinguish measured zero from unavailable CPU, root figures from descendant sums, and partial totals from complete totals; stale/retained observations withhold live values. Published task PIDs remain unmeasured without captured-at-spawn identity.

## Limits

No manual live-fleet UX acceptance, live background-task metrics, Darwin run or aarch64 build is implied. The package is checked on x86_64 Linux; non-Linux resource collection remains unavailable. Descendant sums describe observed OS processes, not agent ownership or complete workloads, and RSS sums can double-count shared pages. The pi-bolt variant/shim staleness redesign remains deferred and unchanged.
