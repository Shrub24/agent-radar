## ADDED Requirements

### Requirement: A live agent is marked stale when its binary was replaced

Radar SHALL compare each live agent's running executable with the program `PATH` resolves for the same program name, and SHALL mark the agent stale only when the running executable's file has been replaced or removed, or when both the running and installed executables are Nix store paths whose store roots differ. Radar SHALL read nothing from a process's environment. When the running process, its executable or the installed program cannot be read, the freshness SHALL be unknown and nothing SHALL be shown.

#### Scenario: Package updated after the session started
- **WHEN** a live agent's executable is under one Nix store root and `PATH` resolves its program under a different root
- **THEN** the agent is stale

#### Scenario: Executable replaced in place
- **WHEN** the running executable's link reports it deleted
- **THEN** the agent is stale

#### Scenario: Wrapper and inner binary
- **WHEN** the running executable and the `PATH` match are different files inside the same Nix store root
- **THEN** the agent is current

#### Scenario: A deliberate other build
- **WHEN** the running executable is outside the Nix store and is not the file `PATH` resolves
- **THEN** the agent is not stale and has no mark

#### Scenario: Nothing to compare
- **WHEN** the process has gone, its link is unreadable, no `PATH` match exists, or the platform cannot read it
- **THEN** freshness is unknown and no mark or claim appears

### Requirement: Staleness is a label, not a state

A stale agent row SHALL carry a mark in the configured stale colour and its details SHALL state the running and installed program identities. Staleness SHALL NOT change the row's state word, ordering, attention or working jumps, filtering, folds or retained/exited treatment. A retained row has no live process and SHALL show no staleness.

#### Scenario: Stale row
- **WHEN** an agent is stale
- **THEN** its row shows the stale mark and its details name the outdated and installed programs
- **AND** its state, order and jump behaviour are as if it were current

#### Scenario: Not the installed program
- **WHEN** the running executable is a different program from the installed one without being outdated
- **THEN** the details say it is not the installed program and the row has no mark

#### Scenario: Retained row
- **WHEN** an agent row is retained after its agent returned
- **THEN** it shows no staleness

### Requirement: Live agent panes are inspected for their process

Radar SHALL query the foreground process of every pane that currently reports an agent, in addition to the panes it already queries, so each live agent row has a process to compare. The query SHALL follow the existing deadlines, cancellation and failure rules, and a pane whose foreground is a shell or inconclusive SHALL show no staleness.

#### Scenario: Agents view
- **WHEN** the agents view is active
- **THEN** each live agent pane has foreground evidence recorded without the process view being open

#### Scenario: Unreadable foreground
- **WHEN** a pane's foreground evidence is a shell or inconclusive
- **THEN** its agent row shows no staleness and its other facts are unchanged
