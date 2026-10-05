# runtime-observation Specification

## Purpose

Observe a local terminal runtime and expose trustworthy agent and pane facts, including the limits of their identity, freshness and continuity.

## Requirements

### Requirement: Local runtime inventory

Radar SHALL observe all workspaces, tabs, panes and reported agents from one local Herdr instance. A successful empty inventory SHALL be distinguishable from a failed collection.

#### Scenario: Multiple workspaces
- **WHEN** a successful collection reports panes and agents in multiple workspaces
- **THEN** all reported workspaces and their observations are available to the overview

#### Scenario: Empty runtime
- **WHEN** a successful collection reports no workspaces or panes
- **THEN** Radar presents an empty inventory without reporting a connection failure

### Requirement: Source-proven agent facts

Radar SHALL keep reported session identity, runtime location, lifecycle and optional semantic metadata distinct. It SHALL expose missing role, assignment, semantic state or ownership as unavailable and SHALL NOT infer them from cwd, pane placement or runtime lifecycle.

#### Scenario: Runtime idle without semantic evidence
- **WHEN** Herdr reports an idle agent without semantic control metadata
- **THEN** Radar exposes runtime idle without claiming assignment delivery, availability for new work or safe lifecycle actions

#### Scenario: Explicit lineage
- **WHEN** records contain unambiguous explicit session and parent-session UUIDs
- **THEN** Radar exposes that ownership link without equating a pane ID or session file path with either UUID

#### Scenario: No lineage metadata
- **WHEN** an agent record contains no explicit ownership identity
- **THEN** ownership remains unavailable rather than inferred from the agent's tab or workspace

### Requirement: Herdsman pane-metadata facts

Radar SHALL read the Herdsman facts that Herdr republishes as pane-metadata tokens — the agent's role, a managed worker's runtime label, the owner's projected state, the active assignment with its display text and start time, the run, request and pending-ask identities, a lead's published name, what the pane is awaiting, the background tasks it reports, and the session's model, provider, thinking level and context usage — and SHALL keep them distinct from runtime facts. Awaited and background facts are the same thing twice, published by two extensions on one pane: the awaited set is the union of the awaited items and the `pi_bg_tasks` ids, and `pi_bg_running` is a count of that set's running members, never a substitute for it. A value this version does not recognise SHALL be preserved rather than dropped or coerced. A value absent from the metadata SHALL stay unavailable: Herdr's own agent name and title SHALL NOT be read as Herdsman facts.

#### Scenario: An owner-projected state
- **WHEN** the metadata carries `pi_herdsman_state` and Herdr reports its own lifecycle status for the same agent
- **THEN** Radar presents the state derived from the pane as the agent's state and the owner's projection as its assignment state, exposing both

#### Scenario: The owner's projection has expired
- **WHEN** no owner-published state is present for an agent, as happens once its published lifetime elapses
- **THEN** Radar derives the agent's state from the facts still on the pane rather than reusing a projection it saw earlier

#### Scenario: Deriving a pane's activity state
- **WHEN** an agent's state is derived, whether or not a projection is present
- **THEN** a fresh `lost` from the owner wins if it is present, work in flight stays in flight, an unknown or unreported runtime state stays unknown, a non-empty union of the awaited items and the background task ids is waiting, and otherwise the runtime's own reported state stands

#### Scenario: A task awaiting retrieval
- **WHEN** a pane's outstanding tasks have exited and none is running
- **THEN** the pane is waiting, because the set is what is outstanding and not the count of what is running

#### Scenario: A pane awaiting work with nothing in flight
- **WHEN** a pane reports outstanding awaited items or a non-empty set of outstanding background task ids and its runtime state is neither working nor unknown
- **THEN** Radar presents it as waiting rather than idle

#### Scenario: A worker's runtime label
- **WHEN** the metadata carries `pi_herdsman_label`
- **THEN** Radar exposes it as the worker's name and does not read Herdr's own truncated agent name or its task-bearing title as a name

#### Scenario: Assignment facts
- **WHEN** a worker's metadata carries an active assignment with its display text and a start time
- **THEN** Radar exposes the assignment and an age measured from that start time, and exposes no age for an agent that has no start time

#### Scenario: An unrecognised state value
- **WHEN** the metadata publishes a state value this version does not know
- **THEN** Radar shows it as reported rather than mapping it onto a state it does know

#### Scenario: Lineage from pane metadata
- **WHEN** a record carries exact session and parent-session identities
- **THEN** ownership is taken from those identities, and the human session name is never used as an identity

### Requirement: Per-pane foreground evidence

Radar SHALL expose, per pane, what the runtime reports about the command in the pane's foreground, and SHALL distinguish a pane where nothing was asked from a pane whose foreground is its shell. Foreground evidence SHALL be tied to the inventory that reported the pane, so a pane that is gone never keeps a process on display.

#### Scenario: Command in the foreground
- **WHEN** the runtime reports a non-shell process group leader for a pane
- **THEN** the pane's foreground command is available to the overview as the program and its arguments

#### Scenario: Idle shell
- **WHEN** the runtime reports the pane shell owning the foreground
- **THEN** the pane is known to have nothing in the foreground, which is distinguishable from never having been asked

#### Scenario: Pane disappears
- **WHEN** a successful inventory no longer reports a pane
- **THEN** its last foreground evidence is discarded with it

### Requirement: Failed collection preserves stale observations

Radar SHALL preserve the last successful inventory during collection failure, visibly mark it stale and report the failure. Failure SHALL NOT prove that panes or agents disappeared. Collection SHALL NOT block keyboard input or quitting, and successful recovery SHALL restore current observations.

#### Scenario: Herdr unavailable after a successful refresh
- **WHEN** the next collection fails or times out
- **THEN** the last inventory remains visible as stale and its retained associations are not removed as if the runtime were empty

#### Scenario: Failure before the first inventory
- **WHEN** the initial collection fails
- **THEN** Radar shows an unavailable-source diagnostic rather than a successful empty fleet

#### Scenario: Recovery
- **WHEN** a collection succeeds after a failure
- **THEN** Radar replaces the stale inventory with the successful observation and clears the source-failure indicator

### Requirement: Idle-shell continuity

Radar SHALL retain the last observed agent association in memory when its pane remains and no replacement is proven, including an idle-shell restart gap. Retained facts SHALL be marked not currently observed; old lifecycle, role and assignment values SHALL NOT be presented as current facts.

#### Scenario: Agent returns to its shell
- **WHEN** an agent stops being reported, its pane remains, and foreground evidence shows the pane shell
- **THEN** its last association remains visible as retained with last-observed facts

#### Scenario: Inconclusive foreground evidence
- **WHEN** the pane remains but foreground evidence cannot establish whether a replacement occurred
- **THEN** the last association remains explicitly unverified rather than falsely current or superseded

#### Scenario: Agent reappears
- **WHEN** a retained pane again reports the same agent session
- **THEN** the new observation replaces the retained facts and the row becomes current

### Requirement: Positive supersession clears continuity

Radar SHALL discard a retained agent association when successful inventory proves its pane absent, a different reported agent session replaces it, or positive foreground evidence shows a non-shell replacement command without a current agent observation. Radar SHALL NOT persist retained associations across its own restarts.

#### Scenario: Pane closes
- **WHEN** a successful inventory no longer contains the retained pane
- **THEN** its retained association is removed

#### Scenario: Another command occupies the pane
- **WHEN** a retained pane has no reported agent and foreground evidence identifies a non-shell command
- **THEN** the old agent association is removed and the pane is available as an ordinary runtime pane

#### Scenario: New session in the same pane
- **WHEN** a pane reports a different agent session from the previous observation
- **THEN** the old association is replaced by the newly observed session without carrying forward its old ownership or assignment

#### Scenario: Radar restarts
- **WHEN** Radar starts a new process
- **THEN** no retained associations from its previous process are restored
