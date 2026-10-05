# Design

## Context

See proposal.md for motivation and scope. The target repository currently contains only OpenSpec and agent tooling; there is no application or build configuration to migrate.

Observed references:
- `~/Projects/dev/custom/herdr-radar/lib/state.js:275–343,697–735` reads agent records and joins `pi_herdsman_session` / `pi_herdsman_parent_session` tokens. Its tree is constrained to a shared tab/workspace; that restriction belongs to its sidebar rendering.
- `~/Projects/dev/custom/herdr-radar/lib/subscribe.js:1–26,74–138` uses events as wake hints, not authoritative changes. Radar will initially poll instead.
- `~/Projects/dev/custom/pi-extensions/pi-herdsman/extension/core.ts:372–397` and `extension/index.ts:3009–3190` distinguish semantic control state from runtime lifecycle. Radar cannot reconstruct that projection from a Herdr status badge.
- `~/Projects/dev/custom/pi-extensions/pi-herdsman/extension/herdr.ts:625–635` calls `herdr api snapshot` and reads `result.snapshot`.

Local API inspection confirmed snapshot inventories for workspaces, tabs, panes and agents. Pane records lack process fields; `herdr pane process-info --pane <id>` returns `shell_pid` and `foreground_processes` with PIDs. The current fleet has no lineage tokens. This is an observed source limitation, not permission to invent ownership.

## Goals / Non-Goals

**Goals:** Keep transport parsing separate from application facts; build a working overview before adding infrastructure; make freshness and identity explicit.

**Non-Goals:** No general graph engine, plugin framework, connector registry, orchestration policy, or speculative process graph. Preserve seams through concrete modules rather than unused interfaces.

## Decisions

### 1. One Rust binary and a small development environment

Use one Cargo package with Ratatui, Crossterm and Serde JSON. Keep transport, normalized observations, continuity and presentation in separate modules within the package. Add concurrency support only as required for responsive collection. Do not start with a crate workspace.

Provide a pinned nixpkgs flake and lockfile, a development shell with Cargo, rustc, rustfmt, Clippy, rust-analyzer and the native tools actually needed to build. `.envrc` uses the flake. Herdr is a separately installed runtime executable, not a source dependency. Initial verification targets the user's Linux environment; cross-platform packaging is deferred. A simple flake is preferred over a module framework for this single-package repository.

### 2. CLI collection, outside the UI input/render path

Poll `herdr api snapshot` at an initial one-second interval. Permit at most one collection in flight and bound command execution (initial default: five seconds). A worker hands complete results to the application; input, redraw and quit do not wait for Herdr. Reap terminated children on timeout and shutdown.

Deserialize only consumed fields, accept additional fields, and reject malformed required inventory instead of treating it as an empty fleet. Successful empty inventory is valid. A failed collection preserves the last successful view and marks it stale. Retry on the normal polling schedule; do not add a socket subscriber or reconnect subsystem now.

The connector emits normalized workspace, runtime-location and agent observations. Herdr DTOs, token names, CLI arguments and shell/foreground-process interpretation stay inside it. UI code does not inspect metadata tokens.

### 3. Separate identity, location and evidence

Runtime locations carry a connector-scoped pane identifier plus tab/workspace references. Agent session identity is a separate optional value: explicit Pi UUID when available, otherwise a source-qualified reported session reference. A session file path is not silently converted into a Herdsman UUID. Missing session identity leaves a pane-associated observation; it does not manufacture an agent UUID.

Parse the known lineage UUID tokens when supplied. Nest only unambiguous same-workspace ownership links; absent owners, conflicting identifiers and cycles fall back to a non-nested row. Cross-workspace nesting is not a product requirement and receives the same simple fallback. Do not infer ownership from tab placement, cwd or a file basename.

Keep runtime lifecycle distinct from any explicitly supplied semantic state. Do not map Herdr `idle` to Herdsman availability, result delivery or safe actions. Role and assignment are optional reported fields; until a verified source mapping exists, display them as unavailable. Reading Herdsman mailboxes or introducing a publisher would be a separate change.

### 4. Pane-backed continuity, not historical storage

Store last agent observations only in this process. A successful snapshot that omits a previously observed agent but still contains its pane triggers a targeted process-info query. If only the pane shell is foreground, retain the previous observation with a `retained / not currently observed` marker; its old status is explicitly last-observed, never current.

A positively observed non-shell foreground command, a different reported agent session, or successful inventory proving the pane absent supersedes the old observation. For non-shell foreground work with no currently reported agent, discard the old agent association rather than guessing what the new command is. Shell foreground evidence uses shell PID/process-group information, not a hardcoded list of shell executable names.

If process-info fails or is inconclusive, retain the row as unverified rather than claiming supersession. If the agent is reported again, use the new observation as current. This is pane continuity; identity across moves, hidden source-side agent restarts and commands that begin and finish between polls are not guaranteed. No transcript or assignment-result archive is built.

### 5. Tree and selected details from normalized observations

Use Herdr workspace identity and label as the first grouping. Show agents under owners where known, and optional ordinary panes once per pane; a retained agent association suppresses an additional ordinary-pane row. Keep tabs as location details rather than introducing a mandatory tab level.

Tree rows show label, concise status and an attention/freshness marker. The selected detail panel shows location, reported identity, source/freshness, lifecycle, and available role/assignment. Missing fields remain unavailable. Retained and globally stale information are visually distinguishable from current observations.

Initial bindings: `j/k` or arrows select; `Space` folds and unfolds the selected branch (`Left`/`Right` are equivalents); `/` edits a case-insensitive text filter; Escape clears/exits filtering; `p` cycles the pane view; `q` quits. Filtering searches displayed labels and available role/assignment text while retaining ancestors of matches. Preserve selection and fold state by stable row identity where possible. Ordinary panes are initially hidden. These are proposed UI defaults, not a user-configurable layout system.

A persistent hint line names the bindings — including which pane view `p` is showing — and switches to the filter's own keys while text is being entered; the title of the focused panel is not the only place a key is discoverable. Content runs the full terminal less a two-column gutter: a wider terminal gives the panels width rather than margins.

Details sit beside the tree rather than beneath it, and `d` hides them. The tree is a list of short rows and the details are a few long lines, so a column costs the tree less than a stack of rows does; a terminal narrower than two readable panels stacks them instead, sized to the content so nothing is clipped.

Presentation follows `herdr-radar`'s visual vocabulary (`src/theme.rs`, `src/title.rs`): a distinct shape per lifecycle state (braille frames while working, `✓` done, `·` parked, `◌` unknown), the vendor's own mark and published colour on agent rows, a row's text in its state's colour with weight spent on the working row, a selection fill rather than an inversion, and green/red reserved for done and source failure.

Motion is configuration in the same way: one animation per state that animates, named in `[appearance]`, with the frames taken from a crate built for exactly that (`tui-spinner`'s flux presets) rather than hand-rolled — a one-cell mark is a frame sequence, and a library of them is better than six. Radar draws the frames itself, because its rows are spans rather than widgets, so what it takes from the crate is the vocabulary. Animation is driven by the clock and a redraw happens only while something is animating: an earlier version advanced its frame counter per loop iteration, which made a mark jump whenever collection happened to finish, and spun a collection spinner that could never complete a rotation in the time a refresh took. Source freshness moved into the fleet heading for the same reason a status line did not earn its row: it is a property of everything the panel shows, and the failure itself is written out in the details.

Roles are named for what they mean rather than where they are painted, and there is one per kind of thing a theme wants to move: headings have their own (`heading`, drawn bold — a terminal cell has no font size, so weight and ink are the whole vocabulary), chrome has its own (`border`, which styles a frame and its title), and the states have theirs. Colours are configuration (`src/config.rs`), not constants. Ratatui offers no theme abstraction, and a dashboard that draws in RGB cannot follow a terminal theme — hand-written or generated — without being told which one it is, so every slot defaults to a terminal palette name and a file overrides any of them. The one thing a palette name cannot express is a vendor's published brand colour, which is the brand's; those are literal and settable per agent name. A file that cannot be used is reported and the built-in colours still draw, because a mistyped colour is not a reason to lose the fleet.

Vendor marks come from the icon font `herdr-radar` installs when it is present and from the ordinary-Unicode table when it is not, so no row depends on a font being installed. `RADAR_ICONS=font|text|none` overrides the detection.

`p` cycles three pane views — agents, running, all — because the two questions the overview answers with panes are different ones: *where is someone working* (`running`, panes with a foreground command, which costs one `herdr pane process-info` per pane, measured at ~3 ms) and *what is in this workspace at all* (`all`). Foreground evidence is kept per pane and only swept while a pane view is active; a pane an agent already reports is never listed twice.

A pane row distinguishes an editor someone is working in from a build in progress, and says how long either has been going. Neither fact is in the Herdr API, so `src/procfs.rs` reads them from the machine: the process's own start time from `/proc/<pid>/stat` and the boot uptime (so a build already running when Radar starts is reported as hours old, not as seconds), and the terminal's line discipline from the terminal that process holds — `tcgetattr` on its own `fd 0`, not Radar's, because the two are different devices. Raw mode with echo off is what taking the terminal over looks like from outside; canonical mode is a command printing into a terminal the shell still owns. It is a heuristic and it is labelled as one: `ssh`, pagers and an editor launched by `git commit` all read as full-screen, because all of them did take the terminal over. Nothing is inferred from it beyond a mark and a phrase, and a platform whose state cannot be read reports nothing rather than guessing.

Titles are normalised before display: a leading provider mark or name is stripped, since the row draws the mark already and Herdr latches a finished session's title as a pane's label. Only a prefix with a separator is removed and never when nothing would remain. A title that is only the directory its workspace is named after is dropped for the pane id, because the group header has already said it. A pane that reports no agent while its label is such a title is shown as `exited` — the row carries what the pane is now, greyed, and the detail panel supersedes the old title.

### 6. Verify observable contracts at their owning boundaries

Use sanitized snapshot/process-info fixtures to verify parsing, extra-field compatibility, missing metadata and transport failures. Test continuity transitions and ownership projection independently of terminal rendering. Use Ratatui's test backend for selected details, filtering and freshness presentation. Finish with a live Herdr smoke test and a terminal cleanup check.

### 7. Herdsman's pane metadata is the semantic source

Radar reads Herdsman's published pane-metadata contract rather than inferring anything from pane activity. Three rules come from that contract, not from taste:

- **Ownership is matched by exact session identity.** `pi_herdsman_session` maps a session to its pane and `pi_herdsman_parent_session` points at the direct owner; the human `session` name is never an identity and tab position is never evidence. An unmatched parent is an orphan root, and traversal is bounded so a malformed cycle cannot hang the tree.
- **The assignment state and the pane's activity state are two facts, and Radar keeps both.** `pi_herdsman_state` is the owner's control projection (`idle`, `working`, `waiting`, `blocked`, `settling`, `unknown`, `lost`) published with a 30-second TTL, so its presence *is* its freshness and an absent key means "no projection", never "the last one". The contract separately specifies how a consumer derives a pane's *activity* state from what is on the pane: a live `lost` from the owner wins, work in flight stays in flight, an unknown or unreported state stays unknown, a non-empty union of awaited facts (a child agent, an owner answer, a background task) is `waiting`, and otherwise the runtime's own state stands. Radar presents the assignment state on the row — the same state the lead's own fleet view names — derives the activity state when no projection is published, and shows the derived state beside the projection when the two disagree, so a settled handoff and a pane buried in background work are both legible rather than one silently winning.
- **Colours stay configuration, marks follow Herdsman.** `waiting`, `blocked` and `settling` are their own palette roles and `lost` reuses `failed`, so a theme can reach every state Radar draws without a code change. The marks and words are Herdsman's own (`◷`, `◐`, `◌`, `×`), so a Radar pane and a lead's fleet view agree on what a state looks like; only the herdr-radar-derived shapes (`·`, `✓`, `⊘`) stay Radar's.
- **An assignment's age comes from the assignment.** `pi_herdsman_started` is Unix milliseconds and exists only while a worker holds an active assignment, which is why elapsed time is drawn for worker rows and not for a lead: a lead has no start to measure from. It is read from the token rather than from `/proc`, because the fact is Herdsman's, and `/proc` is reserved for a pane's own foreground process.

Herdr's own agent `name` and `title` are not treated as Herdsman facts: `name` truncates the human label and `title` welds it to the task, so neither is a faithful name. A managed worker's name comes from `pi_herdsman_label`, a lead's from `pi_herdsman_name`, and the reported title is only the fallback for an agent Herdsman does not publish for.

The awaited facts arrive from two publishers — `pi_herdsman_awaited` from Herdsman and `pi_bg_*` from the background-process extension — and the contract requires a consumer to read them as one union, because neither can express a pane awaiting two background tasks and one agent at once.

### 8. Presentation of those facts

Marks and state words follow Herdsman's own status widget so the two views agree on what a state looks like: `◷` waiting, `◐` blocked, `◌` settling, `×` lost. Radar keeps its own `·` idle, `✓` done and `⊘` exited, and moves unknown to `?` so that settling can take `◌` without two states sharing a glyph. Working, waiting, blocked, settling, lost and unknown each have a named animation setting; `none` keeps the state's fixed shape. Working defaults to `pulse`, waiting to `clock`, settling to `orbit`, and the other states to `none`. Idle, done and exited remain static, and retained rows never animate. Routine working/waiting/idle words are omitted from rows because the mark and colour carry them; details retain every state label.

A row carries `name · state · elapsed · model:thinking`, with elapsed only where an assignment start exists, and every other fact — the assignment text, role, definition, provider, context usage, session name, run, request and pending ask — in the details panel. Colours stay configuration: `waiting`, `blocked` and `settling` are their own roles, and `lost` reuses `failed`, because a proven-absent execution is an error rather than a hue of its own.

### 9. Which facts Radar reads, and which it refuses to infer

- **`facts.definition` comes from Herdr's `agents[].display_agent`, not from a token.** Herdsman's contract has no definition token, and `pi_herdsman_role` already carries a worker's agent definition; `display_agent` is what the publisher sets from the worker's own definition, so the two agree rather than duplicate. *Rejected:* copying the role token into `definition` as a second name — it would invent a definition for a lead, which publishes none. *Revisit when:* Herdsman adds a definition token, or another tool claims `display_agent` (herdr-radar already reads it as a vendor key for panes declaring, for example, `glm`).
- **An unreadable or cleared published value is unavailable, never an inventory failure.** A malformed count, an unparsable start or a token this version does not know leaves that fact absent. Herdr's token map has several publishers and all-or-nothing report limits, so one other publisher's bad count must not empty the fleet.
- **A ported upstream fixture is copied verbatim.** `tests/fixtures/herdsman_pane_metadata.json` is Herdsman's published consumer fixture with one added `source` key naming where it came from; every other line is unchanged, and the copy is the evidence for the decode rather than a fixture this project invented.

### 10. The row's state word is the pane's activity state

Herdsman's contract specifies how a consumer derives one *activity* state from the facts on a pane, and says the owner's projection is not a second authority for it. Radar presents that derived state as the row's state word, and shows the owner's `pi_herdsman_state` projection as a labelled detail line with its age. The two describe different things and are written by different writers at different cadences: the projection is what the owner believes about the assignment (`settling`, `delivered`, `lost`), the derivation is what the pane is doing now. A briefly stale projection is therefore normal, and it must not contradict the row.

*Superseded:* the earlier reading took the opposite order — the assignment state on the row with the derived state in the details — because that is the state a lead's own fleet view names for the same worker. The publisher's owner and the operator both ruled for the derivation on 2026-10-05: the row answers "what is happening in this pane", and a projection that lags cannot hide it. `settling` and `delivered` remain reachable as details rather than being lost.

### 11. The awaited set is the task ids, never the running count

The awaited set is the union of `pi_herdsman_awaited` items and the `pi_bg_tasks` ids, and `pi_bg_running` takes no part in it. Each entry splits at its last colon into id and phase, an unknown phase word is kept as published, and the row's badge counts the distinct unresolved ids. A pane whose tasks have all exited and are awaiting retrieval publishes a zero running count beside a non-empty list, so a count-based rule would call it idle while it is in fact waiting; the live fleet has such panes.

*Rejected:* treating `pi_bg_running > 0` as an extra awaited signal. It reads as safer for a publisher that emits only the count, but it makes the derived state depend on a value the contract excludes. *Revisit when:* a live publisher emits `pi_bg_running` without `pi_bg_tasks`.

### 12. A pane row has two mark columns, and the program glyphs are Radar's own

A pane row mirrors an agent row: the leading column says what the pane is doing
now, the second what it is. The leading column moves while a command runs (the
shared `command` animation) and is the pane's own mark when nothing runs, which
is what fixes a blank column appearing under a marked agent row. The second
column is the program's own mark from `[processes]` where Radar knows the
program, and the terminal-mode mark where it does not.

*Trade-off, deliberate:* where a program has its own mark the mode mark is not
drawn, so `▣` versus `❯` is at a glance only for programs with no mark of their
own. The program's identity is the more useful of the two facts — `nvim` implies
its mode — and the details still state the mode in words with its duration.
*Revisit when:* reading the mode of a known program turns out to matter often.

The shipped table is a small selection of Nerd Font codepoints chosen for this
tool. [tmux-nerd-font-window-name](https://github.com/joshmedeski/tmux-nerd-font-window-name)
keeps a much larger list of the same kind and is the obvious place to take one
from, but the repository declares no licence, so its table is not vendored.
*Revisit when:* upstream adds a licence, or the selection grows enough to be
worth generating from a licensed source.

## Risks / Trade-offs

- [Semantic metadata is absent from the current runtime feed] → Show unavailable fields; do not promise complete roles, assignments or Herdsman control state in this milestone.
- [Polling misses short commands and snapshot/process-info reads are not atomic] → Supersede only on positive evidence; describe continuity as best effort, never durable history.
- [Targeted process queries add collection work] → Query only continuity candidates, keep collection off the UI thread, and keep one refresh in flight.
- [CLI formats or runtime IDs change] → Validate consumed boundaries with fixtures and reset retained associations when replacement identity is observed; do not promise continuity across Herdr instance restarts.
- [Collected labels and metadata are terminal-untrusted text] → Render text without allowing control sequences to alter terminal state.

## Migration Plan

No migration is required. Implement the development skeleton first, then a fixture-driven end-to-end view, then connect the local CLI and continuity checks. The application is read-only toward Herdr. Rollback is stopping Radar; no runtime metadata or agent state is modified.
