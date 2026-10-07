# Processes page — compact metric sections (slice A)

Worker slice A of the compact Processes layout. Scope: `metric_lines`, the
state/cpu/rss/total formatting, and the tests that name them. Identity,
disclosures, page navigation and README are owned by later slices and were not
touched.

## Delivered

`src/ui.rs`

- `metric_lines` now draws two headed sections instead of a list of sentences:
  `process` (birth identity, state, cpu + rss) and `descendants` (count,
  cpu + rss sums, the shared-page caveat).
- New `metric_section(heading, rows)`: a heading line, then labelled facts in
  aligned columns. A row's labels are as wide as that row's longest, so a row of
  two facts spends only the width its own labels need. Two facts share a line
  when both fit [`METRIC_PAIR_WIDTH`] = 28 — the narrowest content width the
  side-by-side layout draws — and a fact that carries a reason never pairs.
- New `Metric { label, value, reason }` with `stated`/`measured`, `measured_value`
  (value or `unavailable` plus the reason), `state_word`, `cpu_percent`,
  `cpu_value`, `rss_value`, `descendant_count`, and `total_value` (replacing
  `total_line`). `state_line`, `cpu_line`, `rss_line` and `total_line` are gone.
- Values: `state` is one short name (`running`, `sleeping`, `disk wait`,
  `stopped`, `traced`, `zombie`, `dead`, `idle`, `unknown (W)`); cpu is
  `12.4%`; rss is `86.0 MiB`; a partial total is `≥2.0 KiB`; an unknown total is
  `unavailable`; every reason is verbatim on its own line, indented under the
  value it qualifies.
- Thresholds kept: a measured idle interval is `0.0%` while a first sample is
  `cpu  unavailable` with the reason; shell / retained / stale rows are still
  drawn with nothing (only `withheld — …`), and the no-sample line
  (`metrics: unavailable — this refresh took no sample of this process`) is
  unchanged. The shared-page caveat is still visible as `note`.

`tests/bus_detail.rs` — the task row's absence assertions were rewritten from
`!contains("birth:")` etc. (now vacuously true, since labels are no longer
`label: value`) to real ones over the new labels.

## Rendered example

Real draw of the fixture used by `the_metric_sections_stay_compact_in_a_narrow_panel`
(measured cpu 12.4%, rss 86.0 MiB, two descendants with a partial rss sum),
captured from a temporary draw harness that was removed after capture. Panel
content is 36 cells at terminal width 120 and 28 cells at width 90 — the
narrowest the side-by-side layout draws.

```
--- 120 x 30 ---                    --- 90 x 30 ---
┌Details─────────────────────┐      ┌Details─────────────────────┐
│ Overview  Processes  Tasks │      │ Overview  Processes  Tasks │
│ Source                     │      │ Source                     │
│worker task                 │      │worker task                 │
│pid: 4242                   │      │pid: 4242                   │
│observed: nix               │      │observed: nix               │
│process                     │      │process                     │
│birth  pid 4242 · boot      │      │birth  pid 4242 · boot      │
│6d9d2f0a-2f6f-4a1f-9c2d-2f6f│      │6d9d2f0a-2f6f-4a1f-9c2d-2f6f│
│4a1f9c2d · start ticks 9812 │      │4a1f9c2d · start ticks 9812 │
│state  sleeping             │      │state  sleeping             │
│cpu  12.4%  rss  86.0 MiB   │      │cpu  12.4%  rss  86.0 MiB   │
│descendants                 │      │descendants                 │
│count  2 processes          │      │count  2 processes          │
│cpu  118.2%                 │      │cpu  118.2%                 │
│rss  ≥410.0 MiB             │      │rss  ≥410.0 MiB             │
│     a descendant exited    │      │     a descendant exited    │
│while the table was read    │      │while the table was read    │
│note  observed processes    │      │note  observed processes    │
│beneath this one, not a     │      │beneath this one, not a     │
│workload or assignment total│      │workload or assignment total│
│(a page shared with another │      │(a page shared with another │
│process counts in each)     │      │process counts in each)     │
└────────────────────────────┘      └────────────────────────────┘
```

The 120-wide capture above is the width-120 panel; the two columns are shown side
by side here for brevity. `panel_text_at(…, 200)` reads back as
`process birth  pid 4242 · boot … state  sleeping cpu  0.0%  rss  8.0 MiB
descendants count  2 processes cpu  125.0%  rss  512.0 MiB note  …`.

## Validation

- `cargo fmt --check` — clean.
- `cargo clippy --all-targets --locked -- -D warnings` — clean.
- `cargo test --locked` — 270 lib + every integration suite, 0 failed
  (animations 1, bus 20, bus_detail 17, collector 12, control 18, descendants 1,
  focus 4, lifecycle 12, managed 9, nested_tree 37, runtime 20, stale_binary 19).
- `cargo build --locked` — OK.
- PTY smoke (`tests/terminal_smoke.py`) was **skipped** in this slice, as the
  brief allows: nothing here touches keys, geometry or the drawing surface.
- New regressions: `every_kernel_state_is_one_short_name`,
  `the_metric_sections_stay_compact_in_a_narrow_panel` (28-cell panel: headings,
  short state, values whole, each reason aligned under its own value, and a pair
  sharing a line). Existing metric tests updated to the new labels, keeping every
  semantic assertion (idle zero vs first sample, lower bound vs unknown, empty
  for shell/retained/stale, no sample → no metric).

## Decisions (for keep-the-why)

1. **A fact with a reason never shares its line.** A row of two facts plus a
   reason has no way to say which value the reason belongs to. Rejected:
   per-fact reason columns (correct but leaves 10 of 28 cells for the reason);
   reasons inline (the prose being removed).
2. **Pairs are sized for 28 cells, not for the panel.** The page is built before
   it is laid out, so the real width is unknown here. 28 is the narrowest content
   width the side-by-side layout draws, so a pair that fits it fits every panel
   without wrapping. Rejected for this slice: passing the panel width down from
   `body_areas` (geometry plumbing, outside the slice).
3. **The one-core base is no longer repeated on each value** — `12.4%`, not
   `12.4% (1 core)`. It is a property of the measure, and the suffix alone is
   what stops cpu and rss sharing the 28-cell line. `README.md` and
   `openspec/specs/process-metrics/spec.md` both state it ("percentage of one
   CPU"). Revisit if the owner wants it on the page: the change is one line in
   `cpu_percent`.
4. **The kernel state's meaning is not on the page in this slice.** The short
   name replaces `sleeping — waiting, and wakeable`; slice B's measurement
   disclosure is where that sentence belongs. Until it lands, the meaning is not
   reachable from the panel (it is in README).

## Deferred follow-ups

- Slice B (identity): deduplicated package + relative executable suffix, stale
  short hashes on both roots, the identity/measurement disclosure, and the state
  / cpu / rss / terminal meanings it should carry.
- The pairing threshold could use the real panel width once a slice owns that
  plumbing; until then pairs are sized for the narrowest panel.
- PTY smoke and the checked Nix package are the lead's to re-run with the other
  slices of this layout.
