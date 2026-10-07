# fleet-overview Specification

## Purpose

Provide a full-screen fleet overview that makes agent ownership, runtime location and observation freshness legible without depending on a mux's built-in sidebar.

## Requirements

### Requirement: Connectors describe the visible tree

Radar SHALL draw branch and continuation connectors for visible agent, task and ordinary-pane rows beneath workspace roots. Connectors SHALL reflect the current order, filtering and folds, including a branch with only one child. Existing state and kind marks SHALL remain distinct from those connectors.

#### Scenario: Several siblings
- **WHEN** an expanded branch has several visible children
- **THEN** intermediate children carry branch connectors, the final child carries a last-child connector, and deeper rows carry only the continuation lines their visible ancestry requires

#### Scenario: A single visible child
- **WHEN** a branch has one visible child after filtering or folding other branches
- **THEN** it uses the last-child connector without a continuation for hidden siblings

#### Scenario: A narrow terminal
- **WHEN** nesting leaves little width for a row
- **THEN** its content fits the available width without corrupting adjacent panels or losing terminal cleanup

### Requirement: Background tasks are children of their agent

When task rows are enabled for the current view, Radar SHALL present each distinct unresolved background task as a selectable child of its unambiguously matched agent. A connected complete bus list SHALL be authoritative, including an empty list; without a matched connected list, reported token ids and phases SHALL provide the limited fallback. Task identity SHALL be scoped to its owner and reported session where known, never to a command title alone.

#### Scenario: Rich tasks
- **WHEN** task rows are enabled and a connected publisher matches an agent by the established exact-session or unambiguous pane-fallback rule
- **THEN** its tasks appear beneath that agent with published command or task id, phase and available age
- **AND** the parent count and details use the same task list

#### Scenario: No rich publisher
- **WHEN** task rows are enabled and an agent reports unresolved task ids but has no matched bus list
- **THEN** one child per distinct id appears with its published phase where present and no invented command or age

#### Scenario: Empty list and disconnect are different
- **WHEN** the connected publisher replaces its task list with an empty list
- **THEN** the task children disappear immediately even if tokens still name them
- **AND** a later disconnect permits token fallback without proving those tasks completed

#### Scenario: No safe join
- **WHEN** a publisher matches no agent, publishes a conflicting session UUID, or has an ambiguous pane fallback
- **THEN** its rich tasks are attached to no agent

#### Scenario: Same id in different sessions
- **WHEN** two agents or successive reported sessions publish the same task id
- **THEN** they are distinct task identities and selection does not transfer across the session change

#### Scenario: Retained task facts
- **WHEN** a task row exists only through its retained agent's last-observed token facts
- **THEN** those facts are labelled last-observed and the task mark does not animate

### Requirement: Task details and navigation share the tree

Radar SHALL let the user select and filter tasks while retaining their agent and workspace ancestry. The selected task's details SHALL show its id, source basis and published phase, command, working directory, process id, start, last output, output size and exit code wherever available. Absent optional facts SHALL be omitted. Task selection and focus SHALL not consume or control the task.

#### Scenario: A task command matches
- **WHEN** the filter matches only a nested task's command or id
- **THEN** the task and its necessary ancestry are visible, and clearing the filter restores prior folds

#### Scenario: Focus a task's location
- **WHEN** Enter or a second click is used on a task and its parent's pane is in the current observation
- **THEN** the existing focus action targets that pane without calling a task-consumption operation
- **AND** stale or missing location evidence produces the existing refusal instead

#### Scenario: A selected task resolves
- **WHEN** a task is removed by its publisher while selected
- **THEN** selection falls back to its surviving visible parent, or another valid row if that parent is gone

#### Scenario: Unsafe task text
- **WHEN** a command or id contains control sequences or exceeds available width
- **THEN** the rendered tree and details sanitize and bound it using the existing display rules

### Requirement: Agent branches have a disclosure control

Radar SHALL allow agent branches to be folded by Space, existing arrow navigation, or their disclosure marker. Clicking elsewhere on an agent row SHALL retain selection and focus behaviour. Folds and selection SHALL survive updates and per-level sorting for surviving row identities.

#### Scenario: Fold an agent with tasks and workers
- **WHEN** the user folds an agent branch
- **THEN** its worker and task descendants disappear and the agent remains selected

#### Scenario: Click the disclosure marker
- **WHEN** an agent's disclosure marker is clicked
- **THEN** that branch folds or unfolds and no focus action is sent

#### Scenario: Click outside the disclosure marker
- **WHEN** a previously unselected agent row is clicked outside its disclosure marker
- **THEN** it is selected without folding or focusing
- **AND** clicking it again uses the existing focus behaviour

#### Scenario: Sorting and live updates
- **WHEN** order or source facts change without removing the selected row or folded branch
- **THEN** selection and folds remain attached to those identities

### Requirement: Workspace-grouped fleet tree

Radar SHALL provide a standalone full-screen tree grouped by Herdr workspace. It SHALL nest agents using unambiguous explicit ownership within a workspace, without requiring a shared tab. Unresolvable or cyclic ownership SHALL leave agents visible without unsafe nesting.

#### Scenario: Owner and worker in different tabs
- **WHEN** an owner and worker share a workspace and have an explicit ownership link but occupy different tabs
- **THEN** the worker appears beneath the owner and its tab remains available as location detail

#### Scenario: Ownership cannot be resolved
- **WHEN** ownership is missing, ambiguous, cyclic or references an owner outside the workspace
- **THEN** affected agents remain visible in their physical workspace without fabricated nesting

#### Scenario: Provider prefix in a reported title
- **WHEN** a reported title begins with a provider mark or name followed by a separator
- **THEN** the row shows the title with that prefix removed, because the row already carries the provider's mark
- **AND** a title that would be emptied by the removal is shown unchanged

### Requirement: Pane views over ordinary panes

Radar SHALL offer three views of ordinary non-agent panes — hidden, running and all — cycled by one key, initially hidden. Each pane SHALL appear at most once in the fleet tree; a current or retained agent association SHALL suppress an additional ordinary-pane row in every view.

#### Scenario: Show every pane
- **WHEN** the user selects the view that lists all panes
- **THEN** panes without current or retained agent associations appear in their workspaces

#### Scenario: Show only running panes
- **WHEN** the user selects the running view
- **THEN** only ordinary panes whose foreground holds a command are listed, led by that command
- **AND** a pane whose foreground is its shell, or whose foreground has not been observed, is not listed

#### Scenario: A program and a command read differently
- **WHEN** one pane's foreground has taken the terminal over and another's is still using the shell's line discipline
- **THEN** their rows carry different marks and the detail panel states which it found
- **AND** a foreground whose terminal state could not be read is not presented as either

#### Scenario: The key cycles
- **WHEN** the user presses the pane-view key
- **THEN** the view moves from hidden to running to all and back to hidden

#### Scenario: Retained association
- **WHEN** a pane has a retained agent association and a pane view is active
- **THEN** only the retained row appears for that pane

#### Scenario: Pane whose label is a finished session
- **WHEN** an ordinary pane reports no agent and its label is a provider session title left there by Herdr
- **THEN** its row carries the session's own name and vendor mark, in the receded ink, with `exited` as its state and the pane's current identity in the detail panel
- **AND** the session title is never presented as the pane's current name

#### Scenario: A finished session in the fleet view
- **WHEN** ordinary panes are hidden
- **THEN** finished sessions are not listed among the live agents unless the user asks for them
- **AND** asking for them lists exactly those panes, and asking again hides them

#### Scenario: A pane in use again
- **WHEN** a pane whose label is a finished session has a command in its foreground
- **THEN** it is not presented as a finished session, and its row leads with that command

### Requirement: Selected details and truthful status

Radar SHALL provide a detail panel for the selected row with runtime location, reported identity, lifecycle, available role/assignment and observation freshness. Missing fields SHALL be shown as unavailable. Current runtime status, retained last-observed status and source-wide stale information SHALL be visibly distinguishable.

#### Scenario: Missing assignment metadata
- **WHEN** an agent has no reported role or assignment
- **THEN** the detail panel shows those fields as unavailable without deriving them from its title or directory

#### Scenario: Retained last observation
- **WHEN** the selected agent association is retained through an idle-shell gap
- **THEN** the tree marks it retained and the panel labels its old facts as last-observed rather than current

#### Scenario: Source failure
- **WHEN** collection fails after a successful inventory
- **THEN** the dashboard marks the displayed inventory stale and exposes the collection diagnostic

#### Scenario: Details hidden and shown again
- **WHEN** the user hides the details panel
- **THEN** the tree keeps the whole content width and the selection is preserved
- **AND** showing it again on a terminal wide enough for two panels restores it beside the tree

### Requirement: An ordinary pane row carries its own mark

Radar SHALL draw a mark of its own in the leading column of every ordinary pane row, so that a pane is never presented with an empty mark column — an unmarked row following a marked one reads as nesting under it. The pane mark SHALL be drawn in the muted ink and SHALL come from the same icon table and font-detection fallback as the vendor marks. Where a foreground command gives the pane a terminal-mode mark, that mark SHALL be drawn instead, and the two meanings SHALL NOT be conflated.

#### Scenario: A pane with nothing in the foreground
- **WHEN** an ordinary pane row has no foreground command
- **THEN** it leads with the pane mark rather than a blank column

#### Scenario: A pane with a foreground command
- **WHEN** an ordinary pane's foreground has taken the terminal over or is still on the shell's line discipline
- **THEN** its running mark advances from the configured frames in the leading column, followed by that terminal-mode mark, so liveness and mode are both readable

#### Scenario: A command that has stopped
- **WHEN** an ordinary pane's foreground command is no longer observed
- **THEN** the leading column holds the pane mark, still

#### Scenario: No icon font installed
- **WHEN** the icon font is absent
- **THEN** the pane mark is drawn from the text fallback rather than as an unknown glyph

### Requirement: An agent row is named and dated from published facts

Radar SHALL draw an agent row as its name — a managed worker's runtime label, a lead's published name, or the reported title when neither is published — the activity state derived from the pane, the age of its active assignment where one exists, and its model and thinking level where those are known. Facts that are absent SHALL be omitted rather than drawn as an empty placeholder.

#### Scenario: A worker's name
- **WHEN** a managed worker publishes its runtime label
- **THEN** its row is named by that label rather than by Herdr's truncated agent name or its task-bearing title

#### Scenario: A worker with an active assignment
- **WHEN** a worker holds an active assignment with a start time
- **THEN** its row shows the assignment's age, and its model and thinking level

#### Scenario: A lead or an agent between assignments
- **WHEN** an agent has no assignment start time
- **THEN** its row shows no age, while a known model and thinking level are still shown

#### Scenario: A pane whose runtime state is unknown
- **WHEN** a pane's runtime state is unreported or unknown while work is outstanding
- **THEN** the derived activity stays unknown rather than being inferred as idle

#### Scenario: Facts that are absent
- **WHEN** an agent publishes no model, thinking level or assignment
- **THEN** the row omits them rather than drawing an empty placeholder

### Requirement: The background badge counts unresolved work

The row's background badge SHALL count the pane's unresolved background tasks, never the running-task count its publisher states. A background task's published phase word SHALL be preserved whether or not Radar recognises it, and a task published without a phase SHALL keep its whole text as its id.

#### Scenario: A pane whose tasks have all exited into review
- **WHEN** a pane's own publisher counts no running task while listing several tasks that exited with their captures unread, and the pane's runtime state is idle
- **THEN** the derived activity is waiting and the row badges the listed tasks
- **AND** the details keep the published running count and the oldest outstanding start beside the unresolved set

#### Scenario: A phase word Radar does not know
- **WHEN** a background task is published with a phase word this Radar does not recognise
- **THEN** the task is kept as published, counted as unresolved, and shown with that word rather than dropped

### Requirement: Herdsman facts are reachable in the details

Every other Herdsman fact SHALL be reachable in the details panel: the owner's projected state, the assignment's display text, the role, the agent definition, the full model, the provider, context usage, the session's human name, the run, request and pending-ask identities, what the pane is awaiting, and its unresolved background tasks with the phases they were published with and the running-task count beside them.

#### Scenario: A pane working with work outstanding
- **WHEN** a pane's runtime reports working while its awaited set is non-empty
- **THEN** the derived activity is working, and the details still name everything it awaits

### Requirement: The owner's projection is distinguished from derived activity

The details SHALL label the owner's projected state as the owner's and SHALL distinguish it from the activity state derived from the pane.

#### Scenario: A projected state disagreeing with the pane
- **WHEN** the owner's assignment projection differs from the activity state derived from the pane's own facts
- **THEN** the row shows the derived activity state and the details name the owner's assignment projection beside it, labelled as the owner's

#### Scenario: A projected state the runtime does not report
- **WHEN** the owner projects `settling` or `blocked` while the pane's own facts say it is working
- **THEN** the row shows the pane's own activity, and the details name the owner's projection beside it

### Requirement: User-set colours

Radar SHALL draw every colour it uses from configuration, each with a built-in default, and SHALL read that configuration from a documented location without requiring one to exist. Vendor colours SHALL be settable by the agent name a source reports, including for a vendor Radar does not already know. A configuration that cannot be used SHALL be reported to the user without preventing the overview from running.

#### Scenario: No configuration
- **WHEN** no configuration file exists
- **THEN** the overview draws with the built-in palette and no diagnostic

#### Scenario: A configured colour is used
- **WHEN** the configuration sets a slot to a palette name, an index or a literal colour
- **THEN** every part of the overview drawn in that slot uses it

#### Scenario: An unusable configuration
- **WHEN** the file names an unknown slot or an unknown colour value
- **THEN** the diagnostic is reported and the overview still runs with the built-in colours

### Requirement: Animation settings are read from configuration

Radar SHALL read separate animation settings for working, waiting, blocked, settling, lost and unknown states, one shared by the marks that mean a process is running in a pane, and their shared frame rate, from the same configuration, each with a built-in default. It SHALL report an unusable animation or rate without preventing the overview from running.

#### Scenario: A configured animation is used
- **WHEN** the configuration names an animation for a state that animates
- **THEN** that state's mark is drawn from that animation's frames

#### Scenario: An unusable animation or rate
- **WHEN** the file names an animation that does not exist, or a rate outside the accepted range
- **THEN** the diagnostic is reported and the overview still runs with the built-in animation

### Requirement: Only observed change animates

`none` SHALL retain the affected mark without animating. Idle, done, exited, retained rows, and a pane whose command has stopped SHALL NOT animate.

#### Scenario: Nothing is animating
- **WHEN** no state on screen animates
- **THEN** the overview does not redraw on its own

#### Scenario: A row that is not being observed
- **WHEN** a retained row's last-observed state was one that animates
- **THEN** its mark is held still, because nothing is observing it move

#### Scenario: A command running in a pane
- **WHEN** an ordinary pane row has a foreground command that is still there on the next collection
- **THEN** its running mark advances from the configured frames, and a pane whose command has gone stops moving

#### Scenario: Background work running
- **WHEN** an agent row's unresolved background tasks include one that is running
- **THEN** the mark beside its background count advances, and a row whose remaining tasks have all exited is still

#### Scenario: Motion turned off for running marks
- **WHEN** the configured animation for running marks is `none`
- **THEN** those marks keep their fixed shape and the row is drawn without animation

### Requirement: Keyboard selection and folding

Radar SHALL support keyboard selection and expanding/collapsing tree branches using Vim-style keys and arrow-key navigation. Refreshes SHALL preserve selection and fold state for surviving row identities where possible. If a selected row disappears, selection SHALL remain on a valid visible row or become empty when none remain.

#### Scenario: Navigate and fold
- **WHEN** the user navigates to an owner and collapses its branch
- **THEN** its descendants are hidden and selection remains valid

#### Scenario: Stable refresh
- **WHEN** a refresh updates facts without removing the selected row
- **THEN** selection remains on that row and existing fold state is preserved

### Requirement: Text filtering

Radar SHALL support a case-insensitive text filter over displayed labels and available role/assignment text. Matching rows SHALL remain visible with the ancestors needed to understand their placement. Clearing the filter SHALL restore the unfiltered tree and prior fold state.

#### Scenario: Match a nested assignment
- **WHEN** the filter matches a worker's reported assignment but not its owner's label
- **THEN** the worker and its owner/workspace ancestry remain visible

#### Scenario: Clear filter
- **WHEN** the user clears the filter
- **THEN** all otherwise eligible rows return with the pre-filter fold state

### Requirement: Safe terminal lifecycle

Radar SHALL handle resizing and quitting without blocking on collection and SHALL restore terminal input/display modes on normal exit and handled application errors. Runtime-provided labels and metadata SHALL NOT execute terminal control sequences through rendering.

#### Scenario: Quit during a stalled collection
- **WHEN** the user quits while Herdr collection is stalled
- **THEN** the application exits without waiting for the collection timeout and restores the terminal

#### Scenario: Control characters in a label
- **WHEN** a runtime record contains terminal-control text in its label
- **THEN** displaying that label does not change terminal modes or execute its control sequences

### Requirement: Background-task row visibility follows the view

Radar SHALL initially hide task children in agents view and show them in running and all views. These views SHALL include every unresolved task regardless of phase. Hiding task rows SHALL leave parent badges, details and activity unchanged, and SHALL NOT change task facts or branch folds.

#### Scenario: Default agent fleet
- **WHEN** Radar starts in agents view
- **THEN** background-task children are hidden while agent rows, nested workers, task badges and parent details remain available

#### Scenario: All unresolved phases in processes view
- **WHEN** running or all view is first selected
- **THEN** matched unresolved tasks are visible, including running, flushing, review, unknown and absent phases
- **AND** tasks are not excluded merely because their processes have exited

#### Scenario: Hidden tasks do not leak into the view
- **WHEN** task rows are hidden and the user filters, sorts or clicks the tree
- **THEN** hidden tasks contribute no visible rows, connectors, disclosure children or pointer targets
- **AND** filtering does not implicitly enable task rows

### Requirement: Background-task rows can be toggled

Outside filter entry, b SHALL toggle task-row visibility for the current view without changing the pane view or finished-session toggle. Each view's choice SHALL be retained for the current Radar run. The footer SHALL name the effective action. A hidden selected task SHALL fall back to its surviving visible parent.

#### Scenario: Toggle task rows
- **WHEN** b is pressed outside filter entry
- **THEN** task children switch between visible and hidden in the current view, and the footer changes between showing and hiding background-task rows

#### Scenario: Mode choices survive cycling
- **WHEN** task rows are hidden with b in running view, and the user cycles through all and agents views back to running
- **THEN** running keeps its hidden choice without changing the choices for other views

#### Scenario: Hide a selected task
- **WHEN** the selected task is hidden by a view change or b
- **THEN** its surviving visible parent becomes selected, or another valid visible row if that parent is absent
- **AND** showing task rows again preserves existing branch folds and never automatically unfolds an agent

#### Scenario: Updates while hidden
- **WHEN** bus or inventory facts change while task rows are hidden
- **THEN** the parent badge and details reflect the latest normalized projection
- **AND** showing task rows later displays that projection rather than a cached earlier list

#### Scenario: b while typing a filter
- **WHEN** b is typed during filter entry
- **THEN** it is filter text rather than a visibility command

#### Scenario: A new Radar run
- **WHEN** Radar restarts
- **THEN** view visibility returns to its built-in defaults rather than persisting the last choices to disk
