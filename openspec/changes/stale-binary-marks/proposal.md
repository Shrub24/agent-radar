## Why

After a Pi update, sessions started before it keep running the old binary until they are restarted, and nothing in the fleet says which ones. Radar already reads `/proc` for each foreground process; the same read can tell a session started from a binary that is no longer the one on `PATH`.

## What Changes

- Compare each live agent's running executable with the one `PATH` resolves for the same program, and mark the row and details when the session is stale.
- Stale is a label, never a state: it does not change the row's word, ordering, attention jumps, filters or retained/exited rules.
- Query the foreground process of every live agent pane, not only continuity candidates, so an agent row has a process to inspect. Unreadable or inconclusive answers show nothing.
- Add a `[colors] stale` role for the mark.

## Capabilities

### New Capabilities

- `stale-binary`: how Radar decides a running program is not the installed one, and where that is shown.

### Modified Capabilities

None.

## Impact

`src/procfs.rs` (executable identity and PATH lookup), `src/model.rs` (`LocalFacts`), `src/observation.rs`/`src/collector.rs` (agent panes join the evidence sweep), `src/ui.rs`/`src/theme.rs`/`src/config.rs` (mark, details line, colour role), README. No publisher, bus, Herdr adapter, lifecycle or dependency change. Extension staleness is out of scope: an ahead-of-time Pi build with extensions compiled in is expected to make the binary the only moving part.
