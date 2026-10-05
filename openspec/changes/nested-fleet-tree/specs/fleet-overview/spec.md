# Spec Delta

## ADDED Requirements

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

Radar SHALL present each distinct unresolved background task as a selectable child of its unambiguously matched agent. A connected complete bus list SHALL be authoritative, including an empty list; without a matched connected list, reported token ids and phases SHALL provide the limited fallback. Task identity SHALL be scoped to its owner and reported session where known, never to a command title alone.

#### Scenario: Rich tasks
- **WHEN** a connected publisher matches an agent by the established exact-session or unambiguous pane-fallback rule
- **THEN** its tasks appear beneath that agent with published command or task id, phase and available age
- **AND** the parent count and details use the same task list

#### Scenario: No rich publisher
- **WHEN** an agent reports unresolved task ids but has no matched bus list
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
