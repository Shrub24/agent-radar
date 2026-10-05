# Spec Delta

## Purpose

Show an agent's background tasks in the detail the owning extension publishes, joined to the agent by exact identity.

## ADDED Requirements

### Requirement: Joining tasks to an agent

Radar SHALL attach a session's bus tasks to the agent row whose `pi_herdsman_session` equals the `hello` session UUID. Where a row publishes no `pi_herdsman_session`, Radar SHALL attach them to the row whose Herdr pane id equals the `hello` pane. Radar SHALL attach them to no other row.

#### Scenario: A matching agent
- **WHEN** a connected session's UUID equals an agent's `pi_herdsman_session`
- **THEN** that agent's details show the session's tasks

#### Scenario: No matching agent
- **WHEN** a connected session matches no agent
- **THEN** its tasks are held and shown on no row

#### Scenario: The pane as a fallback
- **WHEN** an agent publishes no `pi_herdsman_session` and a connected session's `hello` names that agent's pane
- **THEN** the session's tasks are attached to that agent

#### Scenario: Something other than the UUID or the fallback pane
- **WHEN** an agent publishes a different `pi_herdsman_session` than a connected session, or shares only a session name, tab or working directory with it
- **THEN** no tasks are attached to it

### Requirement: Task detail in the selected row's details

When tasks are attached, the details panel SHALL list each task with its id, its state as published, how long it has run, when it last produced output, and its command, and SHALL show a task's working directory and exit code when published. Text published by an extension SHALL be bounded and sanitized as other external text is, and SHALL be written only to the terminal. A field the extension did not publish SHALL be omitted.

#### Scenario: A running task
- **WHEN** a task is `running` and publishes its command, start and last output
- **THEN** the details show the command, `running`, the time since it started and the time since it last produced output

#### Scenario: An exited task awaiting its reader
- **WHEN** a task is in `review` and publishes an exit code
- **THEN** the details show `review` and the exit code

#### Scenario: A state this version does not know
- **WHEN** a task publishes a state word Radar does not recognise
- **THEN** the details show the word as published

#### Scenario: Control sequences in a command
- **WHEN** a published command contains terminal control sequences
- **THEN** they are not written to the terminal

#### Scenario: An over-long command
- **WHEN** a published command exceeds the length bound
- **THEN** the details show it truncated

### Requirement: The bus list outranks the token count

When both are present, Radar SHALL show the bus's task list and SHALL NOT use the token count in place of it. A disagreement between the two SHALL be stated in the details.

#### Scenario: Tokens and bus disagree
- **WHEN** `pi_bg_running` is `1` and the bus lists two `running` tasks
- **THEN** the details show the two tasks and note that the pane tokens report one

#### Scenario: Tasks no longer running
- **WHEN** `pi_bg_running` is `0` and the bus lists one task in `review`
- **THEN** the details show the task and do not report a disagreement

#### Scenario: No bus connection
- **WHEN** an agent has token facts and no connected session
- **THEN** its details show the token facts as they do without a bus
