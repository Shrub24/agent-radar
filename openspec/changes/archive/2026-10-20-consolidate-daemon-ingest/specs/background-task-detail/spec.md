## MODIFIED Requirements

### Requirement: Joining tasks to an agent

Radar SHALL attach a session's tasks to the agent row whose registry-published current session UUID equals the `hello` session UUID. Radar SHALL NOT use a pane identifier, pane token, session name, tab or working directory to attach tasks, and SHALL attach them to no other row.

#### Scenario: A matching agent
- **WHEN** a connected session's UUID equals an agent's registry-published current session
- **THEN** that agent's details show the session's tasks

#### Scenario: No matching agent
- **WHEN** a connected session matches no agent
- **THEN** its tasks are held and shown on no row

#### Scenario: The pane as a fallback
- **WHEN** an agent publishes no session and a connected session's `hello` names that agent's pane
- **THEN** no tasks are attached to that agent

#### Scenario: Something other than the UUID or the fallback pane
- **WHEN** an agent publishes a different session than a connected session, or shares only a name, tab, pane or working directory with it
- **THEN** no tasks are attached to it

#### Scenario: Session changes
- **WHEN** an agent publishes a new current session
- **THEN** tasks of its previous session no longer attach to it
