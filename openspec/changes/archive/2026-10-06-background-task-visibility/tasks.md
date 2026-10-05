# Tasks

## 1. View-local task visibility

- [x] 1.1 Add current-view task visibility to App and its shared Visibility predicate, defaulting agents to hidden and running/all to shown; verify running, flushing, review, unknown and missing phases are included when enabled, hidden tasks do not leak through filtering/connectors/disclosure geometry, and a hidden selected task falls back to its visible parent. Preserve full projection/badges/details, source precedence, folds, ordering and existing task-focus tests; explicitly enable rows in tests that previously relied on the old default.
- [x] 1.2 Add b outside filter entry and its full/terse footer hint, retaining each view's choice through p cycles for this run; verify toggle/reveal after bus updates, per-view independence, filter-entry b, narrow hints and no changes to e. Update README with defaults, unresolved phases and the toggle.

## 2. Integration

- [x] 2.1 Run format, all-target Clippy with warnings denied, locked tests/build, both existing PTY smokes and strict change validation sequentially; record actual results and any failures without weakening timing checks or treating successful retries as proof. Confirm no transport, source projection or lifecycle behavior changed.
