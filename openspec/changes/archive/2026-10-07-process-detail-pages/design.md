# Design

## Context

See proposal.md for motivation. `ui::detail_lines` currently produces one flat list; `App` has mouse details scrolling but keyboard movement targets the fleet. `Geometry` already records details bounds and line count. The collector queries foreground evidence for live agent panes and relevant ordinary panes, then adds age, terminal mode and binary facts through `procfs`. The PID display and exec-replaced-shell correction are local changes to preserve, not reimplement.

The background bus supplies optional PID, but not a captured-at-spawn birth identity. A bare task PID cannot authorize metrics after reuse. Publisher coordination is pending; this change ships pane metrics without inventing a task identity format.

## Goals / Non-Goals

**Goals:** Make the selected row understandable before adding metrics; keep local sampling off the UI thread; distinguish process resources from agent/assignment ownership.

**Non-Goals:** A generic process browser, another agent-children tree, session token/cost statistics, historical storage, stalled classifications, background-task metrics without an agreed publisher identity, cgroup accounting, output consumption, new lifecycle controls or a second mux adapter.

## Decisions

### 1. Four detail pages, not a second fleet tree

Overview holds location, current PID, agent/activity, model/thinking and compact assignment/awaited summaries. Processes holds PID/birth identity, command, age, binary/terminal facts and resources. Tasks holds the existing authoritative joined task list and publication facts. Source holds freshness, retained basis, exact session/path, owner projection, run/request/ask identities and diagnostics. All previously exposed facts remain reachable, with clear labels for derived versus owner-published state.

Agent children remain in the fleet tree. A selected task uses the same page names: its Overview shows task identity/phase and parent location; Processes shows its published PID and explicitly says metrics need verified process identity; Tasks shows its own task facts. Workspace/group pages give applicable summaries rather than borrowing another row's process.

Rejected: another selectable process tree in the details. Resource summaries answer the present question without duplicating fleet navigation. Revisit individual descendant drill-down separately.

### 2. Focused keyboard navigation and limited disclosure

Tab switches tree/details focus (when details exists); Escape in details returns to tree without clearing the filter. In details, j/k and arrows scroll, PgUp/PgDn move a viewport and Home/End reach limits. Left/Right cycle pages. Clickable page headings use rendered Geometry; wheel scrolling remains under the pointer. The focused panel is visually identified using existing palette roles, not hardcoded colour.

Keep lifecycle keys inactive in details; confirmations retain their modal precedence and default Cancel. Filter entry retains text semantics. Tree focus preserves existing keys. Hiding details returns focus to tree. Enter only toggles the highlighted assignment/task disclosure in details, never focuses a pane or executes an action. Tab and Shift-Tab cycle panel focus; no page key steals tree sorting or state-jump keys.

Assignment text and per-task verbose blocks are collapsed initially, with labelled summaries and visible disclosure markers. Essential identity/state/PID/metric lines never collapse. Up/Down scrolls; use Space to cycle the page's disclosure target and Enter to expand/collapse it, with matching clickable markers. This is a small list of disclosure targets, not another full tree. Reset scroll and disclosure target on selection change; preserve active page for the run. Keep per-page scroll for the current selection, clamp after resize/refresh, reset expansion on selection change and preserve surviving task-id expansion on refresh.

Rejected: keyboard scrolling alone over the old flat list; it leaves the information hierarchy unchanged. Rejected: fold every field; it hides the facts the user came to see.

### 3. Known roots and Linux birth identity

Use current normalized foreground evidence as the root. Read boot UUID and `/proc/<pid>/stat` start ticks and validate identity before/after sampling. Cache CPU baselines by (boot UUID, PID, start ticks), never PID alone; missing or changed identity clears the baseline. Matching original-shell and foreground PIDs do not imply a shell when the named foreground leader is Pi or another non-shell executable.

The collector owns sampling history across serialized refresh jobs; the UI only receives immutable normalized facts. Extend the existing collection path, without another polling service or repeated reads during rendering. Reuse the existing clock-tick helper, add page-size conversion for RSS, and parse stat using the final closing parenthesis because comm can contain spaces/parentheses.

Rejected: attach metrics using the session title or command string; neither identifies a process incarnation. Rejected: add a timestamp-only task identity locally; sampling today cannot establish what a publisher spawned earlier.

### 4. Per-process metrics and distinct descendant totals

CPU is delta(utime + stime) / clockTicks / monotonic elapsed, expressed as percentage of one CPU (multithreaded work can exceed 100%). Do not add cutime/cstime, which would double-count reaped children. First sample, reused identity, counter regression or nonpositive elapsed yields unknown CPU, not zero. RSS is resident pages times page size. Show the kernel state with a readable label.

Build one PID/PPID/start-tick snapshot per collection for all known roots and derive descendants without adding fleet rows. Validate root identity, reject cyclic/reused ancestry and implausible parent/child birth order, and revalidate included process identities. Show root resources separately from descendant count and descendant CPU/RSS sums; never call those sums assignment or workload totals. A descendant's CPU needs its own matching baseline. Missing members or scan budget exhaustion make the aggregate partial; unknown is never displayed as a complete zero. If enumeration fails globally, omit totals with an unavailable reason. Do not compare task PIDs to this snapshot for membership.

Limit enumeration/read work with cancellation and a finite scan budget inside the collection worker. Prune history to current observed identities so memory does not grow with exited processes. A failed source collection never presents old metrics as current. Retained rows expose their retained basis, not live resource values.

### 5. Pane-first scope, task metrics gated

Ship this change without a bus wire change. Existing Tasks facts remain available. Before a later task-metrics slice, the publisher must define a captured-at-spawn Linux identity including a format discriminator, boot identity and start ticks; agree fixtures and launch coverage. Legacy missing identity stays unknown. A systemd-run launcher identity cannot claim service/cgroup workload coverage. The upstream question does not block detail pages or pane metrics.

## Risks / Trade-offs

- `/proc` is not an atomic snapshot → revalidate identities, qualify partial totals and avoid claims of complete workload attribution.
- CPU cache resets on Radar restart → first sample is explicitly unavailable.
- RSS sums double-count shared pages → label descendant RSS as a sum with that caveat on Processes.
- Processes reparent or exit between samples → current observed descendants only, not historical work totals.
- Four tabs consume narrow-panel width → compact labels or wrap page navigation without hiding pages; test stacked and narrow layouts.
- Shell-name classification is a heuristic → preserve the actual named PID, test known shells and exec-Pi, treat absent leaders/names as inconclusive.

## Migration Plan

Land details navigation first; verify the existing PID fixes in that layout. Add sampling and Processes presentation in separate delegated slices. No configuration migration or persisted state. Rollback drops the new presentation/sampling without changing publisher or lifecycle protocols.
