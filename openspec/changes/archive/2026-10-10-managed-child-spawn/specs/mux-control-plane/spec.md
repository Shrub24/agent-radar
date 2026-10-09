# mux-control-plane Specification (delta)

## ADDED Requirements

### Requirement: Launch runs a resolved command in a pane this daemon created

A backend MAY declare a launch capability, which SHALL execute the exact
command resolved for a spawn in a pane the daemon created. A backend without
it SHALL refuse a spawn before dispatch, and launch SHALL carry no recovery
claim. An adapter that reaches the pane by typing SHALL refuse, before any
pane input, a resolved argv holding a newline: it SHALL NOT split the command,
and it SHALL NOT write a launch script.

#### Scenario: A backend that declares launch
- **WHEN** the daemon launches a child in a pane it created
- **THEN** the declared capability covers the operation and the resolved command is what runs

#### Scenario: A backend that does not
- **WHEN** a spawn needs launch from a backend that does not declare it
- **THEN** the operation is refused before dispatch and no effect is recorded

#### Scenario: A command a typing adapter cannot carry
- **WHEN** the command resolved for a spawn holds a newline in its argv
- **THEN** the launch is refused before any pane input and no launch script is written

#### Scenario: Launch is not recovery
- **WHEN** a launch completes
- **THEN** nothing about a stopped, resumed or restarted process is claimed
