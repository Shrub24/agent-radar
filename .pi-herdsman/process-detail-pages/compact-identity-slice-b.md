# Processes page — deduplicated identity behind one block (slice B)

Worker slice B of the compact Processes layout. Scope: the page's identity
rendering, one `Disclosure::Process` block and its registry, the Processes
assertions in `tests/bus_detail.rs`, and the README's Processes presentation.
Metric values and honesty rules from slice A are untouched except where a fact
moved into the block. Resolver/procfs/model semantics, page navigation keys,
OpenSpec and real panel-width pairing were not touched.

## Delivered

`src/app.rs`

- `Disclosure::Process`, with `disclosures_on(Processes)` offering it exactly
  when the row draws a live process (`process_detail_is_drawn`: a pane or a
  non-retained agent whose foreground is `NonShell`). Shell, inconclusive,
  no-evidence, retained, stale-source and task rows offer nothing, so no marker
  answers to nothing.

`src/ui.rs`

- `identity_lines` (was `foreground_lines`) now draws the visible identity short:
  `observed`, `package`, `executable`, and a verdict line only when there is one.
- `package_text` names the running installation once: `pi-bolt-0.7.1`, plus the
  first 7 characters of the store hash (`pi-bolt-0.7.1 aaa`) only when the
  comparison has two different builds of that same version to tell apart. A path
  outside the store is named whole.
- `relative_to` + sanitize-first cutting: `executable: lib/pi-bolt/pi` instead of
  the whole store path. A file outside the root is drawn where it is; one that
  *is* the root adds nothing and is left to the `package` line.
- `verdict_line`: `Current` with equal identities (or with a side missing) draws
  nothing at all — no `current` prose; `Current` with differing roots draws
  `binary: other build`; `Unknown` draws a word (`no counterpart`, `payload
  unknown`, `unreadable`, `not the named program`) and only when a reason exists;
  `Stale` draws `<mark> (stale) installed <package> <hash>` with the mark and
  `(stale)` in `palette.stale`.
- `process_detail_lines` is the block's body: both store roots whole, the whole
  executable path, `comparison: unknown — <the full sentence>` where a comparison
  could not be made, the birth identity (`pid · boot id · start ticks`), the
  `states` glossary (`state_meaning` for each of the eight named letters, plus the
  row's own letter when the kernel wrote one Radar does not name), and the
  descendant-sum sentence.
- `Blocks::marker_line` draws a bare `▸ details` summary for a block whose content
  is only ever its body; `heading` was factored out of `metric_section` and is
  shared with the glossary.
- Slice A's `process` section lost its `birth` row and the `descendants` section
  its `note` row: both moved into the block. Every metric value, reason and
  honesty assertion from slice A is otherwise unchanged.

`tests/bus_detail.rs` — a task row's Processes page asserts it offers no block.

`README.md` — the Processes table row, the process-identity bullets, and the
stale-mark bullet now describe the short identity, the block and the new verdict
vocabulary.

## Rendered example

Real draw of the slice-B fixture (stale comparison between two builds of
`pi-bolt-0.7.1`, sampled cpu 12.4% / rss 86.0 MiB, two descendants with a partial
rss sum). Panel content is 36 cells at terminal width 120 and 28 cells at width
90. Captured from a temporary draw harness that was removed after capture; the
same fixture is asserted by `the_process_block_holds_what_the_page_draws_short`.

```
--- 120 x 30, block closed ---      --- 90 x 30, block closed ---
│worker task                 │      │worker task                 │
│pid: 4242                   │      │pid: 4242                   │
│observed: pi-bolt           │      │observed: pi-bolt           │
│package: pi-bolt-0.7.1 aaa  │      │package: pi-bolt-0.7.1 aaa  │
│executable: lib/pi-bolt/pi  │      │executable: lib/pi-bolt/pi  │
│binary:  (stale) installed  │      │binary:  (stale) installed  │
│pi-bolt-0.7.1 bbb           │      │pi-bolt-0.7.1 bbb           │
│▸ details                   │      │▸ details                   │
│process                     │      │process                     │
│state  sleeping             │      │state  sleeping             │
│cpu  12.4%  rss  86.0 MiB   │      │cpu  12.4%  rss  86.0 MiB   │
│descendants                 │      │descendants                 │
│count  2 processes          │      │count  2 processes          │
│cpu  118.2%                 │      │cpu  118.2%                 │
│rss  ≥410.0 MiB             │      │rss  ≥410.0 MiB             │
│     a descendant exited    │      │     a descendant exited    │
│while the table was read    │      │while the table was read    │
```

```
--- 120 x 30, block open (first rows; the body continues with the rest of the
    glossary and the descendant-sum sentence) ---
│▾ details                           │
│running:                            │
│/nix/store/aaa-pi-bolt-0.7.1        │
│installed:                          │
│/nix/store/bbb-pi-bolt-0.7.1        │
│executable:                         │
│/nix/store/aaa-pi-bolt-0.7.1/lib/pi-│
│bolt/pi                             │
│birth: pid 4242 · boot              │
│6d9d2f0a-2f6f-4a1f-9c2d-2f6f4a1f9c2d│
│· start ticks 9812                  │
│states                              │
│running: executing, or waiting its  │
│turn on a CPU                       │
│sleeping: waiting, but wakeable     │
│disk wait: blocked in the kernel,   │
│usually on I/O                      │
```

The mark shown blank above is `theme::stale_mark()` (a Nerd Font triangle where
one can be drawn, `!` where it cannot).

## Validation

- `cargo fmt --check` — clean.
- `cargo clippy --all-targets --locked -- -D warnings` — clean.
- `cargo test --locked` — 443 passed, 0 failed (273 lib + animations 1, bus 20,
  bus_detail 17, collector 12, control 18, descendants 1, focus 4, lifecycle 12,
  managed 9, nested_tree 37, runtime 20, stale_binary 19).
- `cargo build --locked` — OK.
- `python3 tests/terminal_smoke.py` — **both** scenarios pass (source
  failure/recovery + bus + details keyboard/click, and the `bus off` squat).
- New regressions: `the_process_block_holds_what_the_page_draws_short` (closed
  page short at 90 and 120; every moved fact present with the block open),
  `the_process_block_is_offered_only_where_a_process_is_drawn`,
  `the_process_block_opens_from_the_page_at_either_width` (marker click and Enter
  from the page tab's keyboard, at 90 and 120). Existing identity tests updated to
  the new vocabulary, each still asserting its verdict; the wrapped-page scroll
  test now runs with the block open.

## Decisions (for keep-the-why)

1. **A fact that could not be compared shows a word, and its sentence is in the
   block.** One wording per typed reason (`no counterpart`, `payload unknown`,
   `unreadable`, `not the named program`). Rejected: keeping the full sentence on
   the line (the prose being removed) and dropping it entirely (losing the reason
   from the page).
2. **A current comparison draws no verdict at all.** Rejected: keeping
   `current — running and installed …` as a fact, which is the "current" prose the
   change removes; the package line plus the mark's absence already say it.
3. **The store hash appears only where two builds of one version must be told
   apart** (a stale or other-build comparison whose name-versions are equal).
   Rejected: always showing it (noise on every page, and the hash is not what a
   reader is choosing between when the versions already differ).
4. **The block is offered only where the page draws a live process.** Rejected:
   offering it on every Processes page so `Enter` always has a target — a marker
   whose body is empty, or one that answers on a page that drew nothing.
5. **A blocking fact never shares a line (slice A) extended here to reasons:** the
   block, not the line, is where ambiguity is resolved.
6. **`▸ details` is the marker's whole label.** Rejected: a summary value beside
   it, which would repeat the package line the page already drew.

## Deferred follow-ups

- `src/app.rs` cannot see the source freshness, so a *stale source* row still
  offers the block while the page draws only `withheld …`; Enter then toggles a
  block with no marker. Overview's assignment block has the same shape today.
  Fixing it means giving `disclosures_on` the observation state.
- Real panel-width pairing (slice A's fixed 28-cell threshold) is still deferred.
- OpenSpec delta specs for the Processes presentation were not touched this slice
  (out of scope); `openspec/specs/process-metrics` still describes the state value
  as the visible meaning.
- PTY smoke cannot reach the new block: the fake `herdr` publishes no pane
  process, so the smoke's agent has no foreground evidence. A future smoke fixture
  with one would exercise the marker end to end.
