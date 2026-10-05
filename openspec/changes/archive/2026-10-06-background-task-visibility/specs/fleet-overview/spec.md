# Spec Delta

## MODIFIED Requirements

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

## ADDED Requirements

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
