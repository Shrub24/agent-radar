## ADDED Requirements

### Requirement: Dead-pane cleanup is fenced to the creating instance
A pane id is not an identity: a backend that addresses a server may hand the same pane ids to panes this daemon never created. `spawn.close` SHALL refuse, before the request is admitted and before anything is dispatched, when the spawn edge records no verified creating instance, when that instance cannot be verified now, or when the process serving now is not the one that served the create. No generation SHALL be inferred from a pane id, a server name or history.

#### Scenario: Another server generation
- **WHEN** the server that served the create is gone and another now answers to the same name with the same pane ids
- **THEN** close refuses before admission and dispatches nothing, and the pane that now carries the id is untouched

#### Scenario: No verified instance
- **WHEN** the edge records no verified creating instance for its pane
- **THEN** close refuses before admission and dispatches nothing

#### Scenario: The serving process changed
- **WHEN** the process serving now is not the one the edge recorded for its pane
- **THEN** close refuses before admission and dispatches nothing

### Requirement: Closing an exited child is a defined result
When the recorded pane exists within its recorded containment, is fenced to the instance that created it, and the bound child's spawn edge holds a recorded exit observation for that exact pane, `spawn.close` SHALL close the retained pane, persist the outcome durably and return `already_exited` carrying that observation. It SHALL require no live foreground occupant. Replay of the same request SHALL return the stored result without dispatching again.

#### Scenario: Exited child, retained pane
- **WHEN** a tmux child has exited, its pane is dead and its edge records the exit
- **THEN** close returns `already_exited` with the exit observation and the pane is closed once

#### Scenario: Pane respawned after the exit
- **WHEN** the pane is no longer reported dead
- **THEN** the live-occupant verification applies and a non-matching occupant refuses

#### Scenario: No recorded exit
- **WHEN** the pane is dead but the edge holds no exit observation for it
- **THEN** close refuses with no record and no dispatch

#### Scenario: Pane absent
- **WHEN** the recorded pane is not in a successful inventory
- **THEN** close refuses as unresolved and claims neither closure nor exit
