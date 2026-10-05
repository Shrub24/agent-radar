# Implementation verification

## Automated evidence

Checks run from this checkout using its Nix development shell:

- `cargo fmt --check` — passed.
- `cargo clippy --all-targets --locked -- -D warnings` — passed.
- `cargo test --locked` — 173 passed (128 library, 20 bus listener, 12 bus
  detail-join, 11 collector integration, 1 descendant-pipe cleanup regression,
  1 animation-configuration integration); none ignored. Run twice: identical
  results, no test depends on a clock or a live source.
- `cargo build --locked` — passed.
- `python3 tests/terminal_smoke.py` — passed against the built `radar` binary.
  A controlled fake CLI exercised initial failure, successful empty inventory,
  stale-after-success, recovery, filter-entry quit ownership, quitting during
  stalled collection, and restoration of terminal input modes/alternate screen,
  with a stub publisher connecting, replacing its list, disconnecting and
  leaving the pane-token baseline behind.
- `openspec validate initial-fleet-overview --strict` — passed.

Continuity and ownership checks use sanitized Herdr 0.9.3-shaped snapshot fixtures
and controlled fake executables. Collector tests cover shell retention, positive
replacement, invalid/unavailable process evidence, malformed snapshot output,
nonzero exit, timeout, output-pipe pressure and one-refresh-at-a-time execution.
Pure reconciliation tests additionally cover returning/new sessions, pane loss,
source recovery and empty state after restarting Radar. Rendering tests check
retained/stale distinctions, absent metadata and terminal-control sanitization,
and the presentation added after the first acceptance pass: agent state marks and
vendor logos, a row's text in its state's colour, the hint line switching for
filter entry, content centring on a wide terminal, the working mark's animation, and
the provider-prefix/title rules including `exited` panes.

During integration, an inherited output pipe reproduced a two-second shutdown
stall. The collector now assigns Unix commands an isolated process group and ends
that group before joining output readers. The regression and transport suite pass.

## The waiting/awaited contract

Herdsman's owner consolidated the waiting contract as one derivation over the set
of unresolved task ids — the union of `pi_herdsman_awaited` and the `pi_bg_tasks`
ids — with `pi_bg_running` a count of running tasks and no part of the predicate.
Radar's earlier reading used the running count as the awaited test, which read a
pane whose tasks had all exited into `review` as having nothing outstanding.

What now holds, and what verifies it:

- `HerdsmanFacts::is_awaited` is the union of the owner's awaited items and the
  pane's task ids; `cargo test` covers both publishers alone, both empty, and a
  running count with no task list (nothing awaited).
- A `pi_bg_tasks` entry splits at its last colon into id and phase. The id set is
  what is counted, so an id containing a colon survives, an entry with no colon is
  all id, and a phase word this version does not know keeps its task and is drawn
  as published. The `wA:p1:flushing`, `bg-2281` and `bg-2280:quiescing` cases are
  asserted in the model tests and in a rendering test.
- The derivation is tested in its five steps, in order, including the live shape:
  `pi_bg_running=0` with five `review` tasks decodes to `waiting`, both through
  the wire decode and through the rendered row and details.
- The row badge counts the unresolved ids (five for the live shape, two for a
  mixed running-plus-review set) while the details keep the published running
  count (`background running`) and the oldest outstanding start.

The row now shows the derived activity state; the owner's `pi_herdsman_state`
projection is a labelled detail, as recorded in the later row-state verification
below. `pi_bg_running` is still compared with the bus list on running tasks only.

## Scope and outstanding acceptance

The operator accepted the live Herdr smoke on 2026-10-06: "smoke is accepted".
Task 5.2 is complete on that authority. No individual scenario results or tested
Herdr version were supplied; this acceptance is not an agent-run experiment or
proof of the external publisher's implementation. The automated checks above
remain separate evidence.

The evidence covers the local Linux development environment. Other flake systems
are listed but have not been tested. Continuity remains memory-only and polling
based; source reads are not atomic, foreground confirmation can lag two cycles,
and short replacement commands may go unseen. No role, assignment or semantic
control state is invented where the connector lacks that metadata.

## Row state and footer (2026-10-05)

The row's state word is now the activity state derived from the pane, and the owner's
projection is a labelled detail line. Verified in the nix dev shell: `cargo fmt --check`,
`cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked` (9 suites, 136
library tests) and `python3 tests/terminal_smoke.py` (both scenarios) all pass. The two
Herdrs decode tests that asserted a projection-selected state now assert the decoded
projection and the derived row state separately; the two rendering tests that asserted the
old details lines were rewritten. The footer hint line measures 95 columns with the toggle
state words restored (`j/k` lost its "move", `p` names the view alone).

## Pane marks and the running animation (2026-10-05)

An ordinary pane row now has two mark columns: the leading one is the pane's own
mark when nothing runs and the shared running animation while a command does, and
the second is the program's own mark from `[processes]` or the terminal-mode mark
where Radar has none. `[appearance] command` is the single setting for the marks
that mean a process is running, including an agent row's background badge, and
`none` keeps them still.

Verified in the nix dev shell: `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked` (9 suites, 139 library tests) and `python3 tests/terminal_smoke.py` (both scenarios) pass. The suite covers the
commandless pane's mark, the running mark advancing between two ticks,
`app.animates()` staying true while a pane runs a command, the `[processes]`
table's override/removal/length rules and its document round-trip, and that every
shipped mark is a Nerd Font codepoint.
