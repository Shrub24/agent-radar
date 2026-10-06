# Tasks

## 1. Readable detail pages

- [x] 1.1 Split selected-row facts into Overview, Processes, Tasks and Source pages, preserving all current facts and the local PID/exec-shell fixes; add render tests for agents, subagents, panes, tasks, retained/stale sources and narrow/stacked panels, and document the page contents in README.
- [x] 1.2 Add tree/details focus, page cycling, keyboard scroll, per-page clamping and geometry-based mouse page selection; verify selection/fold preservation, filter/modal precedence, hidden-panel focus return and suppression of lifecycle/focus actions from details, and document keys and pointer behavior.
- [x] 1.3 Add initially collapsed assignment and verbose task blocks with keyboard/click disclosures, reset/preservation rules and visible targets; test expansion, refresh/removal/resize, sanitized long text and Enter issuing no runtime action; document disclosure behavior.

## 2. Verified process resources

- [x] 2.1 Add normalized birth identity and root samples in the collector/procfs path, with bounded history across refreshes; deterministic tests must cover boot/PID reuse, disappearing processes, stat names with parentheses, first/zero/invalid CPU intervals, RSS conversion and platform/read failures; document CPU/RSS/state semantics beside the new model boundary.
- [x] 2.2 Add one bounded descendant snapshot per refresh for known roots with identity/ancestry checks, separate aggregates and partial coverage; test shared descendants across roots, cycles, child birth order, new/exited/reused children, permission failures, cancellation and budget exhaustion; document that totals are observed process sums, not workload or agent ownership.

## 3. Metric presentation and integration

- [x] 3.1 Render root and descendant metrics on Processes, including birth identity, CPU warm-up, partial/unavailable labels and shared-RSS caveat; preserve published task PID without sampling it and stale/retained withholding. Add page render regressions for busy children beneath an idle root, missing samples and unknown task identity; document limits and the deferred publisher contract in README.
- [x] 3.2 Independently run fmt, Clippy with warnings denied, locked tests/build, both PTY smokes, checked Nix package and strict OpenSpec validation; verify detail focus/page/disclosure interactions in PTY coverage and record commands, outcomes and unproven live/platform cases in verification.md. Include the pre-existing local PID corrections in the combined review.
