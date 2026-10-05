# Tasks

## 1. Collection through the runtime provider

- [x] 1.1 Introduce the small normalized runtime-provider interface and the Herdr adapter, moving snapshot/process-info transport and its existing bounded runner behind it; route actual collection through this seam while preserving cadence, assignment ages, foreground/local facts, source failure and continuity. Verify a fake provider drives refresh success/failure without Herdr fixtures, and existing snapshot/process-info, cancellation and descendant-reaping tests still pass. Document the adapter ownership and collection invariants alongside this slice.

## 2. Focus through the same provider

- [x] 2.1 Move Herdr workspace/pane focus transport and socket discovery/decoding behind the adapter, inject it into Focuser and wire the executable assembly without a registry or new user option. Verify a fake provider receives normalized workspace/pane targets, retained/task focus and stale/missing refusals are unchanged, and existing fake-command/stub-socket deadline and shutdown tests still pass. Document the common seam and separate owner-control ownership; remove unused transport exports/wrappers instead of leaving the old path active.

## 3. Integration

- [x] 3.1 Run formatting, all-target Clippy with warnings denied, locked tests/build, both existing PTY smokes and strict change validation sequentially. Record the exact results and review that production App/tree/UI/collector/focuser contain no Herdr command names, wire decoding or socket discovery outside adapter assembly, and that no destructive action, owner mailbox, dependency, source-selection setting or observable behavior was introduced.
