# Tasks

## 1. Detection

- [x] 1.1 Add an executable-identity fact to `LocalFacts` (current, stale, unknown) and read it in `src/procfs.rs`: the `/proc/<pid>/exe` target including the deleted marker, one PATH resolution per distinct program name per refresh, Nix store-root comparison and the conservative stale rule, with the running and installed identities kept for display. Cover wrapper-versus-inner binary, store-root change, deleted link, a non-Nix other build, no PATH match and a gone process with injected paths so no test depends on this machine's store.
- [x] 1.2 Extend the collector's per-refresh queries to every pane that currently reports an agent, in addition to continuity candidates and the process-view sweep, keeping the composition above the seam and the existing deadlines and cancellation. Verify a fake provider's agent panes receive evidence in the agents view, shell and inconclusive evidence yield no fact, and retention behaviour is unchanged.

## 2. Presentation

- [x] 2.1 Add the `[colors] stale` role (config, `--print-config`, default, README) and draw the stale mark on agent rows and the details line naming running and installed identities, or "not the installed program". Verify stale and current rows differ only by the mark and details, retained and unknown rows show nothing, and ordering, jumps, filtering and folds are unchanged.

## 3. Integration

- [x] 3.1 Run format, all-target Clippy with warnings denied, locked tests and build, both PTY smokes and strict change validation sequentially; record actual results. Confirm no Herdr adapter, bus, lifecycle or dependency change.
