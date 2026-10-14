# Radar ideas and deferred work

Parked work, with its options, a rough priority and how confident we are in the
direction. Nothing here is committed scope. Active work lives in
[plan.md](plan.md); an item moves there when its trigger is met.

Priority: **P1** needed soon after the current cutover, **P2** when a trigger
fires, **P3** nice to have. Confidence is how sure we are of the direction, not
of the detail.

## Lifecycle and daemon

| Item | Options | Priority | Confidence | Trigger |
| --- | --- | --- | --- | --- |
| Unfinished-operation set | (a) keep the full scan in `unfinished()`; (b) reducer-maintained set of unfinished request IDs, rebuilt on replay, exposed as a named view | P2 | high on (b) | Record volume makes the scan visible, or a second consumer needs it |
| Clear stuck unfinished operations | (a) operator command that marks an operation abandoned with a durable reason; (b) reaper after a deadline with visible outcome | P1 | medium | First real stuck record blocks a server stop |
| Restart rollback gap | (a) accept and reconcile on next start (current); (b) compensating successor record | P3 | medium | A failed successor write is observed in live use |
| Restart and recovery live acceptance | Disposable tmux/Pi run: daemon kill, TUI quit, server replacement, lost response | P1 | high | Live run (user) |
| Native systemd ordering and switch behaviour | Verify `keep-old` / `X-SwitchMethod` keeps a tmux server through a real Home Manager switch; add `Wants=`/`After=` for daemon edges | P1 | medium | Live switch on a real machine |
| Checked Home Manager evaluation (daemon-service 6.3) | (a) add a Home Manager input used only by a check; (b) rely on live acceptance (current choice) | P3 | medium | Module option wiring breaks unnoticed |
| Non-Nix unit generation | (a) keep Nix as the only module (current); (b) generate FHS unit files from the flake in CI | P3 | medium | A non-Nix install is actually needed, or packaging diverges from the module |
| Tmux 3.8 event-driven observation | hooks plus control-mode notifications, `set-hook -B` monitors, OSC 133 shell panes; keep reconciliation as the recovery path; no capability gate, 3.8 required | P2 | medium (not testable locally, tmux 3.8 not installed) | Obtain tmux 3.8 for isolated verification; a second source is needed for timeliness |
| Process evidence row | pidfd/procfs as a separate evidence lane beside tmux events | P2 | medium | Tmux 3.8 observation lands |
| Shared daemon-owned sampled observation | Move collection behind a daemon seam with immutable revisions, freshness and projection-tied pagination | P2 | medium | Sidebar or a second client needs one consistent view |

## Query and filtering

| Item | Options | Priority | Confidence | Trigger |
| --- | --- | --- | --- | --- |
| Generic query layer for daemon reads | (a) typed named predicates per read with one shared scan helper (current direction); (b) embed the `cel` crate (cel-rust, 0.15.0 per docs.rs) | P3 | medium | A second consumer needs structured predicates |
| Structured TUI filters | (a) keep text-only substring match with ancestors kept (current); (b) typed fields such as state or assignment; (c) fuzzy matching (nucleo or fuzzy-matcher, not verified) | P2 | medium | Operators need to filter by state or assignment, not just text |

## TUI and navigation

| Item | Options | Priority | Confidence | Trigger |
| --- | --- | --- | --- | --- |
| Tab row for container actions | Carry the tab's container identity on its row so tab-level delete and reload have a durable target | P1 | high | Section 4 of `daemon-managed-lifecycle` or managed-topology tab actions |
| Primary pane for non-agent windows | Rule for choosing the tab identity when a window has no agent | P2 | low | A window without an agent is confusing in the tree |
| Recipe input in the TUI | (a) keep recipes out of the TUI (current, recipe is external); (b) a typed command line with explicit argv entry; never shell-parsed | P3 | medium | Operators need to create panes without a recipe file |
| Container rename and move | Labels only first; no layout move | P3 | medium | Daily use shows the label is wrong |
| Layout templates per workspace | Reuse tmux's layout engine rather than building one | P3 | low | Repeated manual layout recreation |
| Client-local navigation | Per-client selected window and view state, given that tmux window selection is shared by clients on one session | P1 (part of the sidebar prototype) | medium | Sidebar prototype, plan.md Priority 1B |
| Sidebar host | Kitty outer split or nested tmux shell, compared on disposable resources | P1 (prototype) | low until measured | Sidebar prototype |
| Structured activity, reasons and outcomes | Typed reasons on execution and assignment rows | P2 | medium | Operators cannot tell why a row is idle or blocked |
| Known-process enrichment and descendant tree | Mechanisms recorded in plan.md | P3 | medium | Resource questions in daily use |
| Preview and search | Peek into pane output | P3 | low | Repeated need to see output without focusing |

## Herdr removal and housekeeping

| Item | Options | Priority | Confidence | Trigger |
| --- | --- | --- | --- | --- |
| `remove-herdr-backend` change | Remove backend code first; display labels and `pi_herdsman_*` tokens later, tokens only after Herdsman cutover acceptance | P1 | high | Daemon-service committed (done) and cutover accepted |
| Archive finished changes | `contract-v2`, `window-tabs`, `managed-topology` still unarchived | P1 | high | Next housekeeping pass |
| Supersede `managed-lead-restart` | Mark superseded by `daemon-managed-lifecycle` section 3; its Herdr readiness contract no longer applies | P1 | high | Next housekeeping pass |
| Socket-level enrollment and restart refusal tests | Lifecycle 3.1 to 3.3 socket tests, including malformed enrollment requests | P1 | high | Before section 4 wires restart |

## Deferred by decision (see plan.md)

- Generic asynchronous work, job supervisor, notification broker, durable inbox and
  universal wait coordination. Deferred until a separate migration proves a gain.
- Lead recovery and stale restarts beyond the operator-confirmed TUI path.
- Pi program status and OSC 7501 for Pi state. No native tmux support even in 3.8.
- Upstream Herdsman and pi-bash-processes contract changes, which are owned by
  the pi-extensions side.
