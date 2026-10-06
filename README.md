# Agent Radar

A full-screen fleet overview for coding agents in one local Herdr
instance. Agents are grouped by workspace and nested under their reported owners;
tabs remain location details. Herdr is a connector, not the application model.

Radar observes Herdr, and acts on it only when asked. `Enter` focuses the
selected row's pane. `x` and `X` ask to close a pane or a tab, and `r` asks a
managed worker's owner to restart it: each opens a confirmation first and sends
nothing until it is confirmed, each routes a managed pane to its owner rather
than the mux, and a tab holding a managed pane is refused whole. Everything else
Radar shows is observed, never changed.

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

## Installation (Nix)

The canonical flake URL for this repository is `github:Shrub24/agent-radar`.
Package outputs are provided for `x86_64-linux` and `aarch64-linux`, with Rust
checks enabled. The native `x86_64-linux` build has been verified; `aarch64-linux`
has been evaluated, not built. Development shells retain their existing systems.

```sh
nix run github:Shrub24/agent-radar           # run the installed binary
nix build github:Shrub24/agent-radar         # ./result/bin/radar
nix build github:Shrub24/agent-radar#radar   # the same package by name
```

As a flake input, without a development checkout:

```nix
inputs.radar.url = "github:Shrub24/agent-radar";
inputs.radar.inputs.nixpkgs.follows = "nixpkgs";
# ...
environment.systemPackages = [ inputs.radar.packages.${pkgs.stdenv.hostPlatform.system}.default ];
```

Following the consumer's nixpkgs as above builds Radar with that nixpkgs, which
must therefore provide Rust 1.88 or newer — the floor in `Cargo.toml`, and what
the flake's own pin satisfies. Nothing here fetches a second toolchain.

### Home Manager

The flake exports a single Home Manager module, so a per-user install needs no
`environment.systemPackages` entry and no development checkout. Home Manager is
not an input of this flake: the module is evaluated with the consumer's own Home
Manager and nixpkgs, and only the default package comes from Radar.

```nix
{ inputs, ... }:
{
  imports = [ inputs.radar.homeManagerModules.default ];

  programs.radar = {
    enable = true;
    # Optional: without `settings`, only the package is installed.
    settings = {
      colors.done = "light-green";
      appearance.working = "pulse";
      processes.nvim = "N";
    };
  };
}
```

`programs.radar.enable` installs the package this flake provides for the
consumer's system; `programs.radar.package` replaces it with another one.
`programs.radar.settings` is freeform TOML, and an explicit attrset — an empty
one included — writes `$XDG_CONFIG_HOME/radar/config.toml`. Its `null` default
manages no file at all, leaving Radar with the user's own configuration file or
its built-in defaults. The module sets no `RADAR_CONFIG`, mirrors none of
Radar's defaults, and adds no service or runtime directory.

The package builds `radar` from `Cargo.lock` in the Nix sandbox and runs the
crate's own checks; building or running it needs no development shell.

Herdr is not bundled. Install it separately and keep it on `PATH`, which is
where Radar invokes it. The optional `Herdr Agent Icons Max` font comes from
`herdr-radar`; without it vendor marks use plain Unicode. `RADAR_ICONS` still
overrides detection. Configuration is unchanged: `$RADAR_CONFIG`,
else `$XDG_CONFIG_HOME/radar/config.toml`, else `~/.config/radar/config.toml`.

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
| `x` | Ask to close the selected agent's or pane's pane; opens a confirmation that sends nothing until confirmed |
| `X` | Ask to close the selected row's tab; refused whole if any member is managed or unverified |
| `r` | Ask the selected worker's owner to restart it; offered only for an owner-advertised idle managed worker |
| `c` | Dismiss the lifecycle outcome lines |
| `e` | Show or hide finished sessions (panes whose label is a session that has gone) |
| `b` | Show or hide background-task children in the current view |
| `q` | Quit, except while entering filter text |
| `Ctrl-C` | Quit, including during filter entry |

With a mouse:

| Gesture | Action |
| --- | --- |
| Wheel over the tree | Scroll the list |
| Wheel over the details | Scroll the details |
| Left click on a row | Select it |
| Left click on a row already selected | Focus its pane — or, for a task, its owner's pane — or workspace, as `Enter` does |
| Left click on an agent's disclosure marker | Fold or unfold that branch, without focusing |
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

`Enter` on an agent or pane row focuses that pane, on a task row focuses the
pane its owner row names, and on a workspace row focuses that workspace. Radar
stays open and keeps its selection. A row whose pane the
current observation no longer reports, or any row while the displayed inventory is
last-good rather than current, is refused: nothing is sent and the last line says
why. A failure — Herdr unreachable, the request refused, or the request past its
timeout — is stated on that line too, which clears on the next key press or after
a few seconds. Nothing the focus path asks Herdr for changes an agent, a pane's
contents or a layout. The focus request uses `herdr workspace focus` for a
workspace and the socket API's `pane.focus` request for a pane; `herdr pane
focus` moves by direction only.

`x`, `X` and `r` open a **confirmation** instead of acting: it names the pane,
tab or worker, lists what the action may lose (the agent and its assignment,
outstanding background work, the tab's member panes, or the worker's process),
and starts on **Cancel**. `Tab` or an arrow swaps the selection, `Enter`
activates the selected button and `Escape` cancels; the mouse only ever hits the
two buttons drawn. Nothing is sent until Confirm, and the frozen target is
revalidated against the current observation first: a stale inventory, a
disappeared target or a target whose identity changed cancels with a reason.
Task and workspace rows never redirect a close or restart to a parent.

Only a location with **positive unmanaged evidence** is closed directly through
Herdr, and only after a second inventory is taken immediately before acting. A
pane with no agent, or with an agent kind Pi Herdsman never manages, can be
closed; a Pi pane publishing any `pi_herdsman_*` key is the owner's and refuses
the direct path, and a Pi pane with no owner metadata is unverified and refuses.
A tab holding any managed or unverified member refuses whole, so nothing is
partly applied. A direct mux close has no conditional form, so this cannot be
atomic across the inventory read and the close — the checks narrow the window,
they do not eliminate it. Radar never kills a process itself and never removes a
row optimistically; a successful close is reconciled by the next collection.

A **managed** pane close is routed to its owner instead: `x` on a managed worker
asks the exact parent session, through `herdsman-control/v1`, never the mux.
`r` restarts a managed worker the same way, and only when the owner advertises it
idle: a working, waiting or blocked worker, a lead or standalone session, a
parentless worker and Radar's own continuity-retained history all refuse rather
than launching anything. The request names the exact published label, run UUID
and owner session, echoes the operation, label and run the operator saw, and
carries the pane and session cross-checks it observed; a missing label, run or
owner refuses. It is written atomically into the owner's directory —
owner-created, user-owned, mode `0700`, never created by Radar — so an absent or
untrusted directory is the transport being unavailable, not something to repair.
Existing owner processes must reload before their control directories exist: a
missing directory means unavailable, never unmanaged.

Radar never retries a request, never treats its own timeout as a verdict, and
never cancels one on quitting. Outcomes derive from the owner's files, and a
request already written stays executable whether or not Radar is still running.
Results are shown on the footer's lifecycle line, kept apart from the source
freshness, until `c` dismisses them. A row is never removed optimistically, and a
`closed` result reports the effects the owner actually applied — a lost
generation's close names `process_ended` and leaves its surviving shell pane, so
Radar never infers a pane close from the outcome alone.

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
- A live agent whose running executable no longer matches the program `PATH`
  resolves is marked with a warning glyph in the `stale` colour, and its details
  name the running and installed installations. A deliberate other build — a
  checkout or a second installation — has no mark and says `not the installed
  program`; a process that has gone, or a comparison that cannot be read, claims
  nothing.
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
- Ownership is drawn as a tree. A child that is followed by a visible sibling
  carries `├─`, the last child `└─`, and the rows beneath a branch that
  continues carry `│`. A workspace heading carries no connector. The prefix
  follows the rows actually drawn — filtering or folding a branch changes it
  rather than leaving a line behind — and takes the `subtle` ink, with its
  width part of the row's Unicode-aware fitting. Clicking a branch's disclosure
  marker folds that branch and sends nothing to Herdr; clicking anywhere else
  on a row selects it, and a second click focuses it.
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

Which task children those views list is a choice of its own rather than the pane
filter: `agents` starts with them hidden, `running` and `all` with every
unresolved task shown. `b` flips the current view's choice, and each view keeps
its own answer; [Background-task detail](#background-task-detail-the-bus) has
the phases and the rest.

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
stale     = "yellow"      # a live agent whose binary was replaced
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
no UUID falls back to the pane its `hello` named, and only while **exactly one**
connected publisher names that pane: two of them are an ambiguity, so that row
gets no publisher's list rather than whichever entry came first. Nothing else
joins them — not the cwd, not a title, not the human session name. A session
with no matching row is held and shown nowhere.

One projection decides what a row's tasks are, and the task rows, the `N bg`
badge and the details all read that same projection, so they cannot disagree. A
matched publisher's list is authoritative, an **empty list included** — the pane's
own `pi_bg_tasks` ids are never unioned into it. They are the fallback for a row
no publisher matches, and each of those rows is marked `tokens`, or
`last-observed` when the agent itself is retained. A row a publisher speaks for
shows the publisher's phase word, and when `pi_bg_running` disagrees the panel
says so instead of choosing. Since the token counts only `running` tasks, only
those are compared with it. A phase word Radar does not know is shown as
published.

Each unresolved task is a selectable row beneath its agent, labelled with its
published command or, without one, its id, then the phase as published, the age
its published start time implies, and the program's mark from the configured
`[processes]` table where the command names one. Whether those rows are listed
is a per-view choice: `agents` starts hidden, `running` and `all` start shown,
and `b` toggles the current view alone — the pane view, the finished-session
toggle, branch folds and the `N bg` badge are untouched. `running` and `all`
list every unresolved task whatever phase it published, including `flushing`,
`review`, a word Radar does not know and none at all: the processes view is a
view of outstanding work, not a claim that a task's process is still alive. The
rows read the current projection whenever they are shown, so a list that arrived
while they were hidden is what appears. A task hidden under the selection falls
back to its owner row, which is still there to be read from.

A task's identity is its owner row, the session its facts were reported in,
and its task id — never the command, and never the id alone, because ids are
reused between panes and between successive sessions in one pane. The same id
under a new session is therefore a different task, and a selection does not
follow it: it falls back to the agent row above it. A task row is a leaf: it
cannot be folded, and it has no disclosure control. `Enter` or a second click on
it focuses its owner's pane through the same focus action, with the same refusal
when that location is stale or no longer observed, because a task is read
through the pane its owner row names and is never consumed or controlled.

Selecting a task shows its own panel: its id, phase and source basis, and —
where the publisher sent them — its command, working directory, process id,
start, last output, output size and exit code. The owner's panel states the same
facts on its per-task line, drawn from the same projection, so the rows, the
parent's `N bg` count and both panels cannot disagree. A fact the publisher did
not send draws nothing: absent is not a zero, and a `tokens` row shows only the
id and phase its pane published. A state word this Radar does not know is shown
as written. `command` and `cwd` are the sensitive fields: both are sanitized and
bounded to 256 characters where they reach the screen, and are never written to
a file, the configuration or a log. Task rows move only while their source
reports the process alive now: a `running` word on a `last-observed` row is
stated and stays still.

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
- `src/runtime.rs` / `src/herdr.rs`: the runtime seam and its Herdr adapter.
  Collection asks the seam for a normalized inventory and per-pane foreground
  evidence, and focus asks it to move to a normalized workspace or pane target;
  the adapter owns Herdr's executable, CLI arguments, socket discovery, wire
  decoding and the bounded runner that kills and reaps a stalled command.
  Managed-agent lifecycle never uses this seam: a managed close or restart is
  asked of the worker's owner through the file client below, so a second mux
  would still be a second adapter at assembly rather than a change to the
  collector, focuser or the view.
- `src/collector.rs`: the refresh schedule. One refresh in flight at a time on
  its own thread, cancellation that abandons rather than waits, the
  assignment-age stamp taken once per refresh, and the local `/proc` facts
  composed into the evidence the adapter returned. Two facts are never lost on
  a failed call: an inventory error is a stale source that keeps the last-good
  inventory, and unreadable foreground evidence is inconclusive rather than
  proof that a pane went away.
- `src/bus.rs`: the bus line protocol and the listener that accepts extensions.
- `src/model.rs` / `src/observation.rs`: normalized facts and reconciliation.
- `src/config.rs` / `src/theme.rs`: the colours, where they come from, and the
  vocabulary they paint (state marks, vendor icons, title rules).
- `src/tree.rs` / `src/app.rs` / `src/ui.rs` / `src/title.rs`: projection,
  interactions, rendering and title normalisation.
- `src/focus.rs`: the focus worker run off the calling thread, its cancellation
  and message plumbing, and what `Enter` sends. The two transports behind it
  belong to the adapter.
- `src/lifecycle.rs` / `src/control.rs`: the lifecycle executor. `control.rs` is
  the `herdsman-control/v1` requester — trusted owner directories, atomic bounded
  publication, bounded result reads, file-derived terminal states. `lifecycle.rs`
  holds containment policy, the direct-close worker and the off-thread
  owner-control worker that publishes a confirmed managed close or restart and
  reads its outcome. Managed actions never touch the runtime seam; `x` on a
  positively unmanaged pane is the only lifecycle action the mux performs.
- `tests/`: sanitized fixtures, fake-executable and stub-socket transport
  checks at the adapter, collector and focus tests against an in-memory
  runtime, lifecycle tests against fake providers and a stub owner in temporary
  directories, bus listener and detail-join tests against a stub publisher, and
  a focused PTY check that also connects one to the running dashboard. No real
  Herdr runtime is modified by these tests, and no real lifecycle request is
  written.

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
adapters are outside this milestone, as is every action on Herdr beyond focus
and the unmanaged pane/tab close. Managed close and restart are not Herdr
actions; they are requests to the worker's own owner.
