## Why

Refusals and outcomes are long single lines that the footer clips mid-word, successes stay until dismissed, and nothing records the full text: the daemon's diagnostics are bare `eprintln!` calls and the dashboard has no log.

## What Changes

- Footer messages and outcomes show one line cut with an ellipsis at the terminal width. A completed outcome leaves after 8 seconds; a failure stays until `c`, and `c dismiss` is shown beside the last outcome line.
- Both programs log through `tracing`: the daemon to stderr for the journal, the dashboard to `tui.log` beside the daemon's state directory. `RADAR_LOG` sets the filter. Every footer message and outcome is logged in full. Log lines carry identifiers and outcomes, never a command, an environment or a recipe.

Non-goals: a message history view or a details-panel line for the full text (the log holds it), log rotation beyond restarting a file past 2 MiB, and structured spans across the daemon.
