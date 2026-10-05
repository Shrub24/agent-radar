# Agent Radar

A full-screen fleet overview for coding agents in one local Herdr
instance. Agents are grouped by workspace and nested under their reported owners;
tabs remain location details. Herdr is a connector, not the application model.

Radar reads Herdr. Its one action on it is `Enter`, which asks Herdr to focus the
selected row's pane; everything else Radar shows is observed, never changed.

`plan.md` is the roadmap: interactivity, the nested tree, sorting, peek, process
links, session search and failure reporting, each with the decision it still
needs.

## Development

The flake uses current nixpkgs-unstable, recorded in `flake.lock`. It supplies
Cargo, Rust, rustfmt, Clippy, rust-analyzer and Python for the terminal regression
check. Rust 2024 and Rust 1.88 or newer are required outside the shell.

```sh
direnv allow                  # with nix-direnv; once for this checkout
# or
nix develop

cargo run --locked            # launch radar; q quits
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo build --locked
python3 tests/terminal_smoke.py
```

Herdr must be installed separately and available on `PATH` to observe a live
fleet. It is not a build dependency. The dashboard stays usable and reports a
source diagnostic if Herdr is unavailable. No daemon or additional runtime
configuration is required. Version control uses **jj**.

## Keys

| Key | Action |
| --- | --- |
| `j` / `↓`, `k` / `↑` | Select the next / previous visible row |
| `space` or `←` / `→` | Fold or unfold the selected branch |
| `/` | Enter a case-insensitive filter; Backspace edits |
| `Enter` | Focus the selected row's pane (or workspace) in Herdr; while entering filter text, keeps the filter instead |
| `Escape` | Clear the filter and leave text entry |
| `p` | Cycle the pane view: agents → running → all |
| `s` | Cycle the order: source → state → name |
| `n` / `N` | Select the next / previous row that needs attention (blocked, lost, waiting, unknown) |
| `w` / `W` | Select the next / previous working row |
| `d` | Show or hide the details panel |
| `e` | Show or hide finished sessions (panes whose label is a session that has gone) |
| `q` | Quit, except while entering filter text |
| `Ctrl-C` | Quit, including during filter entry |

With a mouse:

| Gesture | Action |
| --- | --- |
| Wheel over the tree | Scroll the list |
| Wheel over the details | Scroll the details |
| Left click on a row | Select it |
| Left click on a row already selected | Focus its pane or workspace, as `Enter` does |
| Left click on a workspace heading | Select it and fold or unfold it |

Mouse capture is on, which is what makes the wheel and the clicks arrive at all.
It costs the terminal's own text selection: with capture on, dragging selects
nothing until your terminal's bypass key is held (usually `Shift`, `Option` in
iTerm2). Capture is released on every exit, a failure included.

Filtering matches labels and any available role or assignment text, preserving
matching rows' ancestors. Clearing it restores previous folds. Selection follows
surviving row identities through refreshes — and through a re-sort, which is why
both are keyed by row identity rather than by position. The tree scrolls to the
selected row and resizes with the terminal. The key hints stay at the bottom and
switch to the filter's own keys while text is being entered; when one line cannot
hold them all they wrap onto a second rather than lose a key.

Ordering is per level and never changes structure: a child stays under its
parent, and a group ranks by the most urgent row beneath it, so a workspace is
placed by its most urgent agent. Source order is what the observation
arrived in; state order puts what is stuck, unanswered or unreadable first, then
a handoff in progress, then work that is moving, then parked and finished rows;
name order is alphabetical by the label a row shows. `n` and `w` move to the next
row of a kind, wrapping at the ends. A row hidden by a fold or excluded by the
filter cannot be reached that way, because it is not drawn.

`Enter` on an agent or pane row focuses that pane, and on a workspace row focuses
that workspace. Radar stays open and keeps its selection. A row whose pane the
current observation no longer reports, or any row while the displayed inventory is
last-good rather than current, is refused: nothing is sent and the last line says
why. A failure — Herdr unreachable, the request refused, or the request past its
timeout — is stated on that line too, which clears on the next key press or after
a few seconds. Nothing Radar asks Herdr for changes an agent, a pane's contents or
a layout. The focus request uses `herdr workspace focus` for a workspace and the
socket API's `pane.focus` request for a pane; `herdr pane focus` moves by
direction only.

## Presentation

The marks and colours are taken from the tools around Radar rather than
invented, so a Radar pane and its Herdr sidebar agree on what a row says. The
vendor marks, the title rules and the parked/finished/exited shapes come from
`herdr-radar`; the semantic states come from Herdsman's own status widget, which
already draws them:

- Every state has its own **shape**, not only a hue: `⣀`-style frames while an
  agent works, `◷` waiting on work it depends on, `◐` blocked on its owner,
  `◌` settling a handoff, `✓` finished, `×` lost, `?` a state the source did not
  report, `·` parked, `⊘` a finished session. A row's **text carries the same
  state colour as its mark**, so a glance reads the state without parsing a
  word. A retained row is the exception: it recedes to second-rank ink and is
  labelled `retained`.
- Working, waiting and settling marks move by default. Every state except
  idle, done and exited can use its own configured animation; `none` keeps
  its fixed mark. Retained rows never animate.
- Agents are marked with their vendor glyph and the vendor's own colour where
  one is published; an agent with no published hue takes the row's ink rather
  than an invented colour. Green and red stay reserved for done and for a source
  failure.
- The working row is set in **bold** as well as coloured: weight is the second
  axis a coloured list needs, and spending it on the lifecycle that is *moving*
  keeps the finished and parked rows quiet.
- An agent row reads `name · age · model:thinking · N bg` and drops missing facts,
  where `N bg` counts the pane's background tasks that are still unresolved.
  Working, waiting and idle are carried by the mark and colour, not repeated in
  words; other states keep their label. The details always name the state. The name is a
  managed worker's runtime label or a lead's session name when Herdsman
  publishes one, and the reported title otherwise. The age is the active
  assignment's, so only a worker has one. Everything else — role, definition,
  assignment text, what the pane is awaiting, background tasks, the full model
  and provider, context, session name, and the run, request and ask identities —
  is in the details, with the owner's projected state named there as its own
  line when Herdsman publishes one.
- The selected row is filled rather than inverted, so the state colour
  underneath stays readable.
- A workspace heading is drawn **bold** in its own `heading` role, so the
  hierarchy reads before the words do. A terminal cell has no font size —
  weight and ink are what a heading has to work with. Panel frames and their
  titles take `border`, which is where a theme usually wants its chrome
  dimmer than its content.
- Details sit **beside** the tree, not under it: the tree is a list of short rows
  and the details are a few long lines, so a column costs the tree less than a
  stack of rows does. A terminal narrower than 80 columns stacks them instead.
  `d` hides the panel entirely, which gives the tree the width.
- The content column runs the full terminal less a two-column gutter. A wider
  terminal gives the panels width rather than margins.

### Colours

Radar's colours are yours to set (see [Configuration](#configuration)). Nothing
is fixed in the code: every slot defaults to a **terminal palette name** — one of
the ANSI slots a terminal theme actually defines — so out of the box Radar wears
whatever theme the terminal is already using, hand-written or generated. Ratatui
has no theme abstraction to hook into; the terminal palette is the only thing
that follows the user.

Vendor colours are the exception: a published brand colour is the brand's, so it
is a literal `#rrggbb`, and Radar ships the ones it knows. A vendor with no
published colour takes the palette's own slot rather than an invented hue.

### Motion

`[appearance]` has separate animation settings for `working`, `waiting`,
`blocked`, `settling`, `lost` and `unknown` (also used for unrecognised states),
plus `command` for the two marks that mean a process is running — an ordinary
pane's foreground command, and the mark beside an agent row's running background
work. Defaults are `pulse`, `clock`, `none`, `orbit`, `none`, `none` and
`pulse`, respectively.
Idle, done and exited stay still. The animations are
single-character frame sequences — `pulse`, `orbit`, `clock`, `moon`, `arc`,
`classic`, `braille`, `diamond` and others; `radar --print-config` lists them
all, and `none` is a still mark. `fps` sets the rate for every one of them.

Radar redraws **only while something is animating**, so a quiet fleet costs
nothing and the rate is what a busy one costs. Frames come from
[`tui-spinner`](https://crates.io/crates/tui-spinner)'s flux presets — Radar
draws the marks itself (its rows carry spans, not widgets) and takes the
vocabulary rather than the widget.

### Panes

`p` cycles which ordinary panes the tree lists, and the hint line names the view
it is showing:

| View | Lists |
| --- | --- |
| `agents` | Only agents — the fleet. The default. |
| `running` | Panes with a command in the foreground, led by that command |
| `all` | Every pane, including idle shells and finished sessions (`e` covers finished sessions in the other two views) |

The `running` view is what answers "what is actually happening in this fleet":
builds, editors, merge tools and editors that no agent owns. It costs one
`herdr pane process-info` per pane (about 3 ms each, so ~0.1 s for a
30-pane fleet) and only sweeps while a pane view is active; the first press of
`p` fills it in on the next refresh. A pane an agent is already reporting is not
listed twice — the agent row is the row for that pane.

A pane row has two mark columns, as an agent row does: what the pane is doing
now, and what it is. The first moves while a command runs, so a busy pane is
visibly busy rather than only labelled. The second is the program's own mark
where Radar has one and the terminal-mode mark where it does not, and a pane with
nothing running leads with the pane's own mark — so an ordinary pane row is never
a blank column under a marked agent row:

```
│    ⣿  nvim notes.md · 5h14m                  a program drawing the pane
│    ⣿ ❯ nix build .#radar · 4m                a command that will give it back
│    ▭ zsh                                     a pane nobody is using
```

A pane's id is not repeated here — it is a location, and the details panel is
where locations are stated.

`▣` is a program that has taken the terminal over — raw mode, no echo, which is
what an editor, a pager or an interactive session does — and `❯` is a command
still using the shell's line discipline, however long it runs. That is the only
signal the kernel offers for the difference, and it is a heuristic: `ssh`, a
pager and an editor opened by `git commit` all look full-screen, because all of
them have taken the terminal over. The details panel spells out which it decided
and states the duration as `running for`.

#### Program marks

The second column carries the program's own mark when Radar knows the program,
which is what makes an editor, a build or a database client recognisable at a
glance:

```toml
[processes]            # keyed by the program a foreground command runs
"my-editor" = "✎"      # any single character; a Nerd Font glyph is usual
htop = "none"          # take a built-in mark away
```

The built-in marks are Nerd Font codepoints, drawn only where a Nerd Font is
installed: Radar looks for one and falls back to the terminal-mode mark, because
a terminal without the font shows nothing rather than a mark. `RADAR_ICONS=font`
asserts the font is there, `text` says it is not. A mark a user sets themselves
is drawn either way, since a plain character needs no special font — and a
program that is not listed keeps the mode mark, which is the only thing left to
say about an unidentifiable process.

The selection is Radar's own and small on purpose. The much larger list in
[tmux-nerd-font-window-name](https://github.com/joshmedeski/tmux-nerd-font-window-name)
is the same kind of table, and it declares no licence, so it is not vendored
here — its entries go into `[processes]` if you want them.

A pane's title is the one it was given, minus a leading provider mark. When the
only thing Herdr has for it is the directory its workspace is named after
(`~/P/d/nix-fleet` under a `nix-fleet` header), the row shows just the pane id
rather than repeating the header; a path somewhere else is kept.

### Configuration

Radar reads `$RADAR_CONFIG`, else `$XDG_CONFIG_HOME/radar/config.toml`, else
`~/.config/radar/config.toml`. The file is optional: without one the built-in
colours are the configuration. `radar --print-config` prints the configuration
actually in use, which is the file to copy and edit.

```toml
# a name from the terminal's palette, an index, or a literal colour
[colors]
heading   = "default"     # workspace headings, drawn bold
border    = "default"     # panel frames and their titles
subtle    = "dark-gray"   # field names, pane ids
muted     = "default"     # ordinary text, and the parked mark
unknown   = "light-magenta"
done      = "light-green"
working   = "light-blue"  # for a vendor with no colour of its own
waiting   = "light-cyan"  # yielded, waiting on work it depends on
blocked   = "light-yellow" # waiting on its owner: the row to act on
settling  = "blue"        # a handoff converging, dimmer than working
retained  = "light-yellow"
failed    = "light-red"
selection = "dark-gray"   # the fill behind the selected row

[brands]                  # a vendor's own colour, literal by nature
pi     = "#d67079"
claude = "#d97757"

[appearance]              # how a state's mark moves
fps      = 10             # frames per second, for every animation
working  = "pulse"
waiting  = "clock"
blocked  = "none"
settling = "orbit"
lost     = "none"
unknown  = "none"
```

Values are `"default"` (the terminal's foreground), an ANSI name (`"red"`,
`"light-blue"`, `"dark-gray"`), a palette index (`0`-`255`), or `"#rrggbb"` for a
colour the theme cannot change. A vendor Radar does not know can be given a
colour in `[brands]`; the key is the agent name the source reports. A mistyped
key or colour is reported on startup rather than ignored — and Radar still draws,
with the built-in colours.

### Marks and the icon font

`herdr-radar` installs a font (`Herdr Agent Icons Max`) carrying drawn vendor
logos in the Private Use Area. When that font is present, Radar draws those
logos; otherwise it falls back to the ordinary-Unicode table (`π`, `§`, `✦`), so
no row depends on a font being installed. `RADAR_ICONS=font|text|none` overrides
the detection — `font` is what to set when the font was installed system-wide
rather than in the user font directory (`$XDG_DATA_HOME/fonts`, else
`~/.local/share/fonts`).

### Titles and finished sessions

Agents write their mark and name into the terminal title, and Herdr keeps the
last title a pane had after the session is gone. Radar strips a leading provider
mark or name (`π Lint sweep` → `Lint sweep`, `π - Inspect Bifrost - x` →
`Inspect Bifrost - x`) — but only at the very start, only with a separator, and
never when it would empty the title, so `gemini rocks` keeps its words.

A pane that reports no agent while its label is a finished session's title is
shown as the session it was, greyed, with `exited` as its state:

```
│    ⊘ π Deploy and verify herdsman model scopes · exited
```

`⊘` is a state no source reports; the session's own name and vendor mark stay on
the row, because that name is the only thing linking the pane to the work that
was done in it. The details then say what the pane is *instead* — `pane now:` and
its location. If the pane is in use again, it stops being history: the row leads
with the command in its foreground, and the finished session stays in the
details.

Herdr latches a session title as the pane's **label**, and it survives the
session: on a fleet that has run many agents, most ordinary panes carry one.
Those are historical, not running, so they stay out of the default `agents` view
— `e` lists them there anyway, and `e` again hides them.

## Observation and continuity

- Radar runs `herdr api snapshot` off the UI thread, with at most one refresh in
  flight. It waits one second between completed refreshes; each command has a
  five-second timeout. Input and quitting do not wait for a stalled command.
- Failed collection preserves the last-good inventory with a **stale** diagnostic.
  Initial failure is **unavailable**, not an empty fleet. A successful empty
  snapshot clears the inventory; successful recovery clears the diagnostic. The
  fleet heading carries the state (`Fleet · current`, `Fleet · STALE`,
  `Fleet · UNAVAILABLE`), and the selected row's details write the failure out —
  there is no status line of its own.
- Roles, assignments, ownership and semantic state come from Herdsman's own
  published pane metadata (`pi_herdsman_*`), never from Herdr's agent fields.
  Radar reads the role, a managed worker's runtime label, the owner's assignment
  projection, the active assignment with its start time, what the pane is
  awaiting and the background tasks it reports running, and the session's model,
  provider, thinking level and context. A fact the source does not publish stays
  unavailable; the source's token lifetime is its freshness, so an expired value
  is absent rather than remembered.
- The **row's state word is the pane's activity state**, derived from the pane
  as Herdsman's contract specifies, in this order: a fresh `lost` from the owner
  wins, work in flight stays in flight, an unknown or unreported state stays
  unknown, a non-empty set of outstanding work is `waiting`, and otherwise the
  runtime's own state stands. That set is the union of the owner's awaited items
  (`pi_herdsman_awaited`) and the ids in the pane's own background task list
  (`pi_bg_tasks`) — never `pi_bg_running`, so a pane whose tasks have all exited
  into `review` is waiting with nothing running. The details keep the published
  running count distinguishable from the unresolved set.
- The owner's **assignment state** is a different fact and is shown as its own
  labelled detail line (`assignment: settling (owner projection)`) rather than
  choosing the row's word. It is what a lead's own fleet view names for that
  worker, and it is written by a different writer at a different cadence: a
  `settling` or `delivered` projection can be briefly stale, and the row must
  not contradict the pane because of it. `settling`, `delivered` and `unknown`
  projections stay reachable in the details.
- Radar does not read Herdsman mailboxes, does not treat Herdr's agent name or
  title as a Herdsman fact, and never infers ownership from tab position or from
  the human session name — ownership is exact session identity.
- Explicit, unambiguous ownership can span tabs within a workspace. Missing,
  conflicting, cyclic or cross-workspace ownership falls back to workspace-level
  rows. Ordinary panes never duplicate current or retained agent rows.
- When an agent disappears but its pane remains, Radar keeps its last observation
  **in memory**, visibly retained rather than current. Targeted
  `herdr pane process-info --pane <id>` queries distinguish a foreground shell
  from a positively identified replacement command using PID evidence.
  Inconclusive evidence leaves the association unverified.
- A new reported agent replaces the old association. Positive non-shell evidence
  or a successful snapshot proving pane loss clears it. Retained facts are
  last-observed, not current status or historical outcomes. Restarting Radar clears
  them; it has no durable history.

Polling is best effort: snapshot and process-info reads are not atomic, short
commands between polls can be missed, and foreground confirmation can lag by two
refresh cycles. Continuity across Herdr restarts or session moves is not promised.

## Background-task detail (the bus)

The pane tokens are a pointer: `pi_bg_running` counts the background tasks a
pane currently has running, and `pi_bg_tasks` names what is unresolved as
comma-separated `<id>:<phase>` entries — `review` for a task that has exited with
its capture still unread, `flushing` for one whose capture is not yet certified.
Herdr's 32-key limit leaves no room for the rest, so the extension that owns them
— today `pi-bash-processes` — pushes their detail
instead. **Radar only listens**, on one Unix socket per user
(`$RADAR_SOCKET`, else `$XDG_RUNTIME_DIR/agent-radar/radar.sock`, else
`/tmp/agent-radar-<uid>/radar.sock`), and never connects back. Nothing in the
protocol acknowledges, settles or consumes anything, so reading a task through
the bus leaves it and its result exactly as they were; `pi-bg get` stays the only
way an agent consumes one. The protocol, its limits and the publisher's
obligations are in [`docs/radar-bus.md`](docs/radar-bus.md), with example lines
in [`docs/radar-bus.fixture.json`](docs/radar-bus.fixture.json).

A task list is joined to an agent row by the **exact session UUID** the row
publishes as its ownership lineage (`pi_herdsman_session`). A row that publishes
no UUID falls back to the pane its `hello` named, and nothing else joins them —
not the cwd, not a title, not the human session name. A session with no matching
row is held and shown nowhere. The publisher's list outranks the pane token:
the details list the tasks, and when `pi_bg_running` disagrees the panel says so
instead of choosing. Since the token counts only `running` tasks, only those are
compared with it. A phase word Radar does not know is shown as published, and so
is the published running count, beside the unresolved count the row badges.

Each task draws its id, its state word as published, how long it has run, when it
last produced output, its output size, its command, and — where published — its
working directory and exit code. A fact the publisher did not send draws
nothing: absent is not a zero. A state word this Radar does not know is shown as
written. `command` and `cwd` are the sensitive fields: both are sanitized and
bounded to 256 characters where they reach the screen, and are never written to
a file, the configuration or a log.

Bus data lives only as long as its connection. A disconnect removes that
session's entry immediately and the row falls back to its token facts, because a
gone publisher is not evidence that its tasks ended. An explicit empty list is
different: the publisher is connected and reports nothing unresolved, which the
details state in words.

The bus is a subsystem of its own, and its failures are never reported as source
failures: a socket that cannot be bound (a directory that is not ours, or
another Radar answering on it) leaves the fleet observed as usual, marks the
heading `· bus off`, and writes the diagnostic on the `bus:` line of the details.

## Layout and verification

One Cargo package, with `radar` as its binary:

- `src/main.rs`: terminal lifecycle and event loop.
- `src/herdr.rs` / `src/collector.rs`: wire decoding and bounded CLI execution.
- `src/bus.rs`: the bus line protocol and the listener that accepts extensions.
- `src/model.rs` / `src/observation.rs`: normalized facts and reconciliation.
- `src/config.rs` / `src/theme.rs`: the colours, where they come from, and the
  vocabulary they paint (state marks, vendor icons, title rules).
- `src/tree.rs` / `src/app.rs` / `src/ui.rs` / `src/title.rs`: projection,
  interactions, rendering and title normalisation.
- `src/focus.rs`: the focus request, its two transports, and what `Enter` sends.
- `tests/`: sanitized fixtures, fake-executable transport checks, focus tests
  against a fake `herdr` and a stub API socket, bus listener
  and detail-join tests against a stub publisher, and a focused PTY check that
  also connects one to the running dashboard. No real Herdr runtime is modified
  by these tests.

`Cargo.lock` and `flake.lock` record reproducible dependency versions. The approved
scope and task list are in
[`openspec/changes/archive/2026-10-06-initial-fleet-overview/`](openspec/changes/archive/2026-10-06-initial-fleet-overview/).

Manual acceptance: run `cargo run --locked` against your Herdr instance. Check the
workspace inventory, cycle the pane views, navigate/fold/filter, show and hide the
details, and inspect location, ownership, freshness and unavailable metadata.
Press `Enter` on an agent row, a workspace row and a retained row, and confirm
Herdr's focus lands where the row says; then the same with Herdr stopped, which
must leave Radar usable and say why on the last line. Real
runtime/UX acceptance is deliberately manual; automated checks use controlled
fixtures and executables.

Direct semantic feeds, process/resource trees, daemon clients and other mux
adapters are outside this milestone, as is every action on Herdr beyond focus.
