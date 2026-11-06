# The dashboard

The terminal client: how to drive it, what it shows, and how to configure it. The
dashboard is a view onto the daemon; it observes and requests, and every
lifecycle action it asks for is decided and recorded by the daemon. See the
[README](../README.md) for what Radar is and how to install it.

## Keys

| Key | Action |
| --- | --- |
| `j` / `↓`, `k` / `↑` | Select the next / previous visible row. Selection is this view's own: moving it sends nothing and never moves tmux |
| `space` or `←` / `→` | Fold or unfold the selected branch |
| `/` | Enter a case-insensitive filter; Backspace edits |
| `Enter` | Ask the session to focus the selected row's pane (or workspace); the daemon moves the client attached to it, and with none attached the dashboard opens a terminal for that place and asks again; while entering filter text, keeps the filter instead. The row selected here is the target, whatever another view has selected, and a row that has since gone is refused rather than replaced by the focused one |
| `Escape` | Clear the filter and leave text entry |
| `o` | Follow the terminal's focus: the sidebar starts on, the dashboard off. While following, a changed focus selects its row once; browsing is not undone, and following never asks tmux to move. `◆` marks the focused row in every view, apart from the selection |
| `p` | Cycle the pane view: agents → running → all |
| `s` | Cycle the order: source → state → name |
| `n` / `N` | Select the next / previous row that needs attention (blocked, lost, waiting, unknown) |
| `w` / `W` | Select the next / previous working row |
| `d` | Show or hide the details panel |
| `D` | Ask to close the selected row. The daemon freezes the selected target for confirmation; it never substitutes the focused row. Container closure stops placed managed executions before removing the resource. An execution whose exit cannot be verified is reported as unverified, not assumed exited |
| `r` | Restart the selected managed agent in its recorded session: the daemon stops it with a verified exit, then relaunches it. Offered for a bound daemon subject with a session |
| `a` | Add an empty tab, in the workspace directory, to the selected row's workspace; it asks nothing |
| `A` | Start a new workspace; nothing is selected, replaced or confirmed |
| `R` | Reload the selected container's recorded recipe |
| `L` | Launch the configured managed agent in a new tab or pane |
| `E` | Report the selected Pi session's claim standing (granted, duplicate, reserved, pending or released); sends nothing |
| `!` | Open the full text of the last outcome and the last message |
| `c` | Dismiss the lifecycle outcome lines |
| `e` | Show or hide finished sessions (panes whose label is a session that has gone) |
| `b` | Show or hide background-task children in the current view |
| `Tab` / `Shift-Tab` | Move the keyboard between the tree and the details |
| `q` | Quit, except while entering filter text |
| `Ctrl-C` | Quit, including during filter entry |

While the details hold the keyboard:

| Key | Action |
| --- | --- |
| `j` / `↓`, `k` / `↑` | Scroll the page one line |
| `PgDn` / `PgUp` | Scroll the page one viewport |
| `Home` / `End` | Scroll to the page's first / last line |
| `←` / `→` | Cycle the pages, wrapping at either end |
| `Space` | Move to the page's next openable block |
| `Enter` | Open or close the block the keyboard is on |
| `t` | With Processes focused and its process table open, enter or leave process-row navigation, where `j`/`k`, `Home`/`End` and `Enter`/`Space` move over the table's rows and fold their branches instead of the page |
| `Escape` | Give the keyboard back to the tree |
| `/` and the view keys | Still act on the fleet, as they do from the tree |

Reading a page never moves the selection, folds nothing and sends nothing: `Enter`,
`D` and `r` are the panel's keys there and do nothing, so a reader cannot
focus a pane or open a lifecycle confirmation by scrolling. Filter entry and an
open confirmation keep precedence — the confirmation's arrows swap its buttons
rather than turning a page, and typing goes to the filter.

The process table has a small mode of its own. With the Processes page focused
and the table open, `t` enters it and the table's own label says so. There `j` /
`↓` and `k` / `↑` select the next and previous row without wrapping, `Home` and
`End` reach the first and last, and `Enter` or `Space` fold or open the selected
process's branch — a leaf has none to fold. Outside the mode every key is the
page's again: scrolling, the block cycle, page switching, `Tab` and `Escape` are
unchanged, and so are `PgUp`/`PgDn` and the wheel. Selecting a row gives the
compact process block above the table that process's own name, PID, state,
interval CPU and resident set, while the root's binary comparison stays under
its own `foreground root` heading and is never attributed to a descendant. The
mode, the cursor and the folds belong to the selected row's own process, keyed
by its birth identity: another row, a process replaced at the same PID, a root
whose table is no longer offered, and closing the table all leave the page
without them, and a process that folds out of sight or goes away returns the
cursor to the root. Clicking a row selects that process and enters the mode;
clicking a row's fold marker only folds that branch. None of it focuses a pane,
changes the fleet selection or its folds, or issues a close or restart.

With a mouse:

| Gesture | Action |
| --- | --- |
| Wheel over the tree | Scroll the list |
| Wheel over the details | Scroll the page under the pointer |
| Left click on a page heading | Show that page and hand the details the keyboard |
| Left click on a disclosure marker in the details | Open or close that block alone |
| Left click in the details | Give the details the keyboard without acting on the row |
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

## Detail pages

The selected row's facts are split into four pages, so what a reader wants first
is not buried under the long published text a task list brings with it:

| Page | Shows |
| --- | --- |
| Overview | What the row is: its kind and lifecycle, its location, the live foreground PID, what it is running, and the row's activity and state. For a registered agent, each authority is a labelled line with its own publisher and freshness — physical, execution, assignment and facts — followed by the owner's role, definition, assignment text and awaited items, and the session's model, provider, thinking level and context usage |
| Processes | The process holding the location: the command name the runtime reports for it, the package it runs from and the executable inside it, how that compares with the program `PATH` resolves now, the PID, how long it has run, the terminal mode, and what that process was measured using — scheduler state, interval CPU, resident set and the descendants observed beneath it, summed and then drawn one row each in a table of their own — with the store, the roots' own parts, the birth identity, what the state it is showing means and every reason held behind the page's `details` block |
| Tasks | The background tasks published for the row — each task's id, phase, source, command, directory, PID, timings, output and exit — and the pane's own token ids when no publisher joins |
| Source | Where the facts came from: source freshness, the diagnostic when the source failed or a page is retained, the identities the row carries (session name, run, request, ask), its lineage when it has one, and the diagnostics |

Every fact an earlier version showed is still reachable, on one page rather than
repeated across all four, except a foreground process's arguments: the Processes
page names the process and its executable instead of drawing a command line that
can carry a prompt, and the row still leads with what the pane is running. A
page shows only the selected row's facts: a task's published PID is named with
its source and never borrowed as a live process of the row above it.


Long published text starts collapsed behind a `▸` marker: an assignment long
enough to bury the facts around it, each task's command and directory, and the
table of processes observed beneath the page's own foreground process. The
assignment keeps its opening readable while closed, a closed task block keeps
the task's line with its id, phase, age, output and exit code, and the closed
table keeps its one-line marker — identity, state, PID and the measures beside
them never collapse, so only the text that would wrap over the panel is behind
the marker. `Space` picks the page's next openable
block, `Enter` opens or closes the one it is on, and neither focuses a pane or
starts a lifecycle action; clicking a marker toggles that block alone. An opened
block stays open across refreshes while its task survives, leaves with the task
it belongs to, and closes when the selection moves to another row.

The page headings name all four pages, and they move onto a second row when the
panel is too narrow to hold them, so every page stays reachable on a stacked or
narrow terminal. A page keeps its own scroll for the selected row; changing the
selection brings every page back to its top, and a shorter page, a resize or a
refresh clamps the offset to the rows actually drawn rather than leaving an empty
tail. The unit is a drawn row, so the rows a wrapped line takes are part of the
page like any other and scrolling reaches the last of them.

`Enter` on an agent or pane row focuses that pane, on a task row focuses the
pane its owner row names, and on a workspace row focuses that workspace. Radar
stays open and keeps its selection. A row whose pane the
current observation no longer reports, or any row while the displayed inventory is
last-good rather than current, is refused: nothing is sent and the last line says
why. A failure — tmux unreachable, the request refused, or the request past its
timeout — is stated on that line too, which clears on the next key press or after
a few seconds. Nothing the focus path asks tmux for changes an agent, a pane's
contents or a layout: it selects the window or the pane and nothing else.

Adding a pane or a tab, and `L`, move the client to what they created, because a
create is dispatched unfocused and focused afterwards: `[server] focus_created`
(default `true`) decides that, and a request may say otherwise. The focus is a
second effect on a completed creation, so a refusal is reported without undoing
or repeating the create; a completed focus says nothing, since moving a terminal
changes nothing in the tree. Creating a workspace is unfocused, because tmux
cannot create a session and switch a client to it in one command.


## Closing and restarting

`D` ends whatever the selected row is, and `r` restarts a managed agent. A
confirmation names the row and lists what the action may lose — the agent and its
assignment, outstanding background work, the tab's member panes — and starts on
**Cancel**. `Tab` or an arrow swaps the selection, `Enter` activates the selected
button and `Escape` cancels; the mouse only ever hits the two buttons drawn.
Nothing is sent until Confirm, and the target is revalidated against the current
observation first: a stale inventory, a disappeared target or a target whose
identity changed cancels with a reason.

Every level answers the same question, walked down from the row selected: what
here would be lost? An agent is lossy unless it reports idle or done; a pane is
lossy when a non-shell command is in its foreground; published background work
still running counts too. Because the answer is collated from every pane the row
covers, a workspace asks about the editor three panes down exactly as a pane row
asks about itself. An idle shell, an idle agent and finished work are not losses,
so the standard case is not a question: `D` ends the row and the outcome lands in
the `!` panel. When the answer is not empty the confirmation names the contents
rather than the record that holds them — `nvim is running`, `worker-3 is working`,
`pi has 2 background task(s) running` — so you read what ends, not a handle.

What `D` ends is the selected row's own scope:

| Row | `D` ends |
| --- | --- |
| Agent | That agent's process, then its pane when Radar launched the pane |
| Pane | That pane |
| Tab | The whole tab |
| Workspace | The workspace and everything recorded inside it |

The daemon decides this for the row you selected; a view never substitutes the
focused row or chooses for it.

Ending an agent is graceful first. Radar sends TERM, waits one window, and only
then ends a process that is still running by killing the systemd scope it
launched in. A process that cannot be ended, or whose end cannot be verified, is
reported as **unverified** and never recorded as exited. A place holding several
agents ends them together — every agent is signalled before any is waited on, so
a workspace with three agents costs one wait, not three — and a program's exit
code is captured before its pane goes, when the program reported one.

An agent that is still alive when the window closes does **not** hold the
deletion. You named the branch and its contents, so the place closes once and the
outcome says whose exit it could not verify. Nothing asks twice. While the close
runs the branch shows as `closing`, and a launch or restart that would land inside
it is refused as `busy` instead of arriving in a place about to be removed.

A container whose place is no longer served is **forgotten**: its records go and
nothing is dispatched, because there is nothing left to close. A container whose
place is still served is refused instead of forgotten around, so no live place is
orphaned. This is the recovery path for a fleet whose backend was replaced
underneath it.

The tree never removes a row optimistically; it follows observations after the
operation. Whether the program exited, whether its pane was removed and what code
it reported are separate facts, and none is inferred from another. The status line
names unverified exits and uncertain removals, and `!` opens the full outcome.

## Presentation

The marks and colours are taken from the tools around Radar rather than
invented, so a mark means the same thing here as in the sidebar next to it. The
vendor marks, the title rules and the parked/finished/exited shapes follow the
agent icon and title conventions Radar was built beside; the semantic states come
from Herdsman's own status widget, which already draws them:

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
  name the installed target beside the running package. A current comparison has
  no verdict at all: the absence of a stale mark is the verdict. A deliberate
  other build — a checkout or a second installation — has no mark and says
  `other build`, leaving both roots to the page's block. A process that has gone,
  or a comparison that could not be made, carries no mark and gives its reason in
  a word on the Processes page, with the whole sentence in that block.
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
  marker folds that branch and sends nothing to the backend; clicking anywhere else
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
| `agents` | Only live agents |
| `running` | Live panes, including idle shells and foreground commands. The default. |
| `all` | Every pane, including unresolved and finished rows |

The default view waits for conclusive shell or process evidence; unresolved,
exited and finished rows stay hidden unless explicitly revealed with `p` or `e`.
A pane an agent already represents is not listed twice. Workspace rows remain
visible in every view and under text filtering.

Which task children those views list is a choice of its own rather than the pane
filter: `agents` starts with them hidden, `running` and `all` with every
unresolved task shown. `b` flips the current view's choice, and each view keeps
its own answer; [Background-task detail](observation.md#background-task-detail-the-bus) has
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
them have taken the terminal over. The details panel spells out which it decided,
states the duration as `running for`, and — while the evidence is current — names
the running process's pid.

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
only thing the backend has for it is the directory its workspace is named after
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

An optional agent icon font (`Herdr Agent Icons Max`) carries drawn vendor
logos in the Private Use Area. When that font is present, Radar draws those
logos; otherwise it falls back to the ordinary-Unicode table (`π`, `§`, `✦`), so
no row depends on a font being installed. `RADAR_ICONS=font|text|none` overrides
the detection — `font` is what to set when the font was installed system-wide
rather than in the user font directory (`$XDG_DATA_HOME/fonts`, else
`~/.local/share/fonts`).

### Titles and finished sessions

Agents write their mark and name into the terminal title, and the backend keeps
the last title a pane had after the session is gone. Radar strips a leading provider
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

The backend latches a session title as the pane's **label**, and it survives the
session: on a fleet that has run many agents, most ordinary panes carry one.
Those are historical, not running, so they stay out of the default `running` view
— `e` lists them there anyway, and `e` again hides them.

