# Verification

## Slice 1.1 — accepted

The primary session reviewed the delivered sampler/model projection and independently ran `cargo fmt --check`, Clippy with warnings denied, the full locked suite, a locked debug build and both `tests/terminal_smoke.py` scenarios in the Nix devshell. All passed: **448 tests, 0 failed** (278 library tests).

The rows are projected from the confirmed ancestry walk that supplies the existing sums. Parent references carry birth identity, names/state come from the same kernel read as counters, unavailable scans carry no members, and regressions pin unchanged process-read counts. Names remain raw kernel observations until the renderer sanitizes them; sibling ordering belongs to the table projection. The primary session corrected one doc comment to distinguish unavailable CPU from a measured idle zero.

Task 1.1 is accepted. Rendering, interaction and the full integration/checked Nix gate remain pending. This record does not imply a live table smoke, final change acceptance, archive or push.

## Slice 2.1 — accepted

The primary session reviewed deterministic parent-identity projection, disclosure gating, selected-child identity isolation and the corrected table cell-width layout. Its independent Nix-devshell gate passed formatting, Clippy with warnings denied, **458 tests, 0 failed** (288 library tests), a locked debug build, both PTY smoke scenarios and strict validation of this change.

Wide and combining names are measured with Ratatui's `Span::width()` for shortening and padding; numeric columns include their heading widths. Real-buffer regressions cover aligned wide/narrow columns, short-panel scrolling, deep branches, measured zero versus unavailable values, root-only versus unenumerated observations, and the child-selection render seam. The initial wide-name overflow limitation was corrected before acceptance.

Task 2.1 is accepted. Production process-row selection, branch folding, mouse targets and their PTY smoke remain task 3.1; the final checked Nix/all-spec gate remains task 4.1. No live descendant-table acceptance, archive or push is implied.

## Slice 3.1 — accepted

The primary session reviewed the production render seam, the cursor's deferred reveal, identity-keyed selection and folds, the pointer targets and the local mode's key map, then ran the Nix-devshell gate itself: formatting, Clippy with warnings denied, **468 tests, 0 failed** (298 library tests), a locked debug build and both PTY smoke scenarios.

Review confirmed that the mode keeps page switching, `PgUp`/`PgDn`, `Tab` and `Escape` intact; that no process key returns an action, opens a confirmation or reaches the mux or an owner; that cursor and folds are keyed by full birth identity and reconciled on refresh without a refresh moving the viewport; and that the root's figures are drawn once with its binary evidence under `foreground root`. One in-flight defect was corrected before acceptance: `render` passed no selection, so the picked process's block never reached the screen, and the reveal now runs from the draw the selection produced.

## Task 4.1 — integration gate

Run by the primary session on the accepted tree:

- `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked` — **468 tests, 0 failed** — and `cargo build --locked`.
- Both `tests/terminal_smoke.py` scenarios, including the new process-row navigation, fold and return-to-tree steps over a really sampled tree.
- `nix flake check -L` — **all checks passed**, producing `/nix/store/qdliifk59raw0sa6s2h3384j7iy6381n-agent-radar-0.1.0`.
- `openspec validate --all --strict` — 14/14, with the inherited requirement-length warnings resolved.

Limits this record does not claim: no operator-observed live Radar session against a busy fleet was run for the descendant table; `aarch64-linux` is evaluated only and Darwin is unsupported; the change is not yet synced, archived, committed or pushed.

## Validator baseline

OpenSpec 1.14.1 validates this change strictly. Its all-spec strict mode also flagged inherited >500-character requirements in fleet-overview, runtime-observation and stale-binary. The primary session has since split those five requirements into focused ones without changing their meaning, and `openspec validate --all --strict` now passes 14/14 with no length warnings. This cleanup is independent of the sampler, table and interaction slices.
