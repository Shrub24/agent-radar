# Spec Delta

## ADDED Requirements

### Requirement: An execution exists by its registration
A managed execution SHALL be created from its registration alone, with an optional verified placement and an optional launch intent. It SHALL acquire a subject and claim only when a session attaches to it.

#### Scenario: A launch has no subject before its program runs
- **WHEN** a configured launch has reserved its execution and its program has not yet reported a session
- **THEN** no subject and no claim are recorded for it and the observation shows only its pane

#### Scenario: A session attaches to a launched execution
- **WHEN** the launched program reports its first presence
- **THEN** a subject and claim are attached to that execution through the same enrollment an adopted execution uses

### Requirement: Every way of managing a process uses one execution lifecycle
Adoption, configured launches, recipe launches, restart and spawn SHALL create executions through one lifecycle that differs only in its inputs: whether the process is identified at creation or when it claims its launch, whether a successor claim is attached up front, and whether a spawn edge is composed.

#### Scenario: Restart differs only by its attached claim
- **WHEN** a managed agent restarts
- **THEN** its successor execution is created as a launch would be, with the existing subject's successor claim attached up front

## MODIFIED Requirements

### Requirement: Managed executions have claim-scoped placement
Every managed execution SHALL identify its registration and verified hosting pane independently of spawn parentage. Current placement SHALL reflect verified pane moves without changing subject identity or historical spawn provenance. A pane handle without its backend generation SHALL NOT authorize removal.

#### Scenario: Admitted agent has no spawn edge
- **WHEN** `radar run --agent` admits an agent in an existing pane
- **THEN** container discovery can find its exact execution through verified placement without inventing a spawn edge

#### Scenario: Pane moves between tabs
- **WHEN** fresh topology verifies a managed pane moved
- **THEN** current containment follows its new location while subject identity and recorded producer parentage remain unchanged
