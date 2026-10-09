## MODIFIED Requirements

### Requirement: Lifecycle results do not replace observation
Lifecycle work SHALL not block input, collection or quitting. Outcomes SHALL be shown separately from source diagnostics, with no optimistic row removal, whether read from owner files or control-plane records. Managed controls SHALL remain unavailable until owner implementation acceptance is confirmed and SHALL refuse when their transport is unavailable. An uncertain control-plane outcome SHALL not trigger direct retry.

#### Scenario: Owner control unavailable
- **WHEN** the owner implementation is unconfirmed or no trusted transport exists
- **THEN** managed actions explain their unavailability while observation and focus continue

#### Scenario: Applied operation
- **WHEN** a valid closed or restarted result arrives
- **THEN** Radar reports it and reconciles rows through subsequent inventory rather than fabricating runtime state

#### Scenario: Control-plane outcome unknown
- **WHEN** a control-plane response is lost after possible dispatch
- **THEN** Radar reports uncertainty, issues no direct retry and fabricates no row removal
