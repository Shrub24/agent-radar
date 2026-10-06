# lifecycle-actions Specification

## Purpose
Let an operator close panes or restart managed workers through the authority that owns them, without confusing a request with an applied effect.

## Requirements

### Requirement: Lifecycle actions require separate confirmation
Radar SHALL open confirmation for x (selected pane/agent close), X (its tab close) and r (managed-worker restart). Opening confirmation SHALL send nothing. It SHALL name the target and possible losses, default to Cancel, and require activation of Confirm. Task/workspace rows SHALL not redirect destructive actions to their parents.

#### Scenario: Close confirmation
- **WHEN** x is pressed on an eligible row
- **THEN** a confirmation names the pane and any process, assignment or outstanding work that may be lost, without sending a request

#### Scenario: Cancel or accidental Enter
- **WHEN** Escape is pressed or the initially selected Cancel is activated
- **THEN** no lifecycle action is submitted

#### Scenario: Mouse confirmation
- **WHEN** the operator clicks Confirm in the drawn dialog
- **THEN** the same checks as keyboard confirmation apply
- **AND** tree second-click focus never serves as lifecycle confirmation

#### Scenario: Filter entry
- **WHEN** a lifecycle key is typed while editing the filter
- **THEN** it is filter text and opens no confirmation

### Requirement: Confirmation is tied to observed identity
Radar SHALL refuse lifecycle submission from stale inventory or absent targets. Confirmation SHALL freeze operation and target identities and SHALL be revalidated before submission. A changed identity or containment SHALL cancel the action with a reason, not redirect it to the current selection.

#### Scenario: Replacement during confirmation
- **WHEN** the pane's session or managed run changes before Confirm
- **THEN** Radar sends nothing and reports the target changed

#### Scenario: Collection fails
- **WHEN** inventory is no longer current at confirmation
- **THEN** no action is submitted

### Requirement: Direct runtime close requires positive unmanaged evidence
Radar SHALL route unmanaged pane/tab close through its runtime provider only with current positive unmanaged evidence. Pi panes without ownership metadata are uncertain. Managed and continuity-retained managed associations SHALL never use direct runtime close. Unsupported providers SHALL refuse explicitly.

#### Scenario: Ordinary unmanaged pane
- **WHEN** current evidence reports no agent and no retained or uncertain managed association
- **THEN** confirmed close can reach the runtime provider

#### Scenario: Unverified Pi pane
- **WHEN** a Pi pane publishes no ownership metadata
- **THEN** direct close is refused, not treated as unmanaged

#### Scenario: Retained managed association
- **WHEN** a managed association is locally retained while its pane still exists
- **THEN** direct runtime close remains forbidden

### Requirement: Container close is all or nothing at admission
Radar SHALL refuse an entire tab close when any observed member is managed or uncertain. It SHALL perform no partial pane closure and SHALL recheck containment before direct tab close.

#### Scenario: Mixed tab
- **WHEN** a tab contains an ordinary pane and a managed or unverified Pi pane
- **THEN** tab close is refused and neither pane is closed

### Requirement: Managed operations are owner-routed
Radar SHALL send confirmed managed close/restart only to the exact parent-session owner using herdsman-control/v1, with exact label/run identity, available cross-checks and matching confirmation. Missing identity or owner SHALL refuse. Restart SHALL be limited to idle retained managed workers; leads, standalone sessions and busy targets SHALL not be restarted.

#### Scenario: Idle managed worker restart
- **WHEN** an eligible idle managed worker is confirmed
- **THEN** the request echoes its operation, label and run UUID and asks its exact owner to restart it

#### Scenario: Busy or parentless target
- **WHEN** restart targets a working/waiting/blocked worker or a lead/standalone session
- **THEN** Radar refuses without a mux launch fallback

#### Scenario: Close abandoned assignment
- **WHEN** a managed close is confirmed for active work
- **THEN** Radar warns that the assignment may be abandoned and the owner decides eligibility

### Requirement: Owner file transport preserves trust and identity
Requests SHALL use trusted owner-created user-owned 0700 non-symlink directories, unique UUID filenames, atomic 0600 publication, an 8 KiB limit and absolute expiry. Radar SHALL not create owner directories or overwrite requests. Results SHALL be bounded and validated against request id, version and operation; unknown fields SHALL be tolerated.

#### Scenario: Untrusted or missing directory
- **WHEN** an owner directory is absent, symlinked, foreign-owned or has loose permissions
- **THEN** Radar refuses and writes nothing

#### Scenario: Mismatched result
- **WHEN** a result names a different request or operation
- **THEN** Radar reports invalid evidence and does not claim completion

### Requirement: Outcomes come from files, never local timeout
Radar SHALL derive result, started, not_executed and pending in that precedence. Claimed requests SHALL never be retried; local timeout and shutdown SHALL not imply cancellation. Pending or unknown actions SHALL block duplicate submission for that target until authoritative resolution.

#### Scenario: Claim without result
- **WHEN** a claim exists but no result exists
- **THEN** Radar reports started with unknown outcome even after expiry and does not retry
- **AND** a later valid result can refine the display

#### Scenario: Expired unclaimed request
- **WHEN** neither result nor claim exists and absolute expiry passed
- **THEN** Radar reports not_executed

#### Scenario: Owner refusal
- **WHEN** a valid refused result exists
- **THEN** Radar shows the owner's category and message without removing or changing the row optimistically

#### Scenario: Quit after submitting
- **WHEN** Radar quits with a published request
- **THEN** quitting remains responsive and Radar does not delete or cancel the request

### Requirement: Lifecycle results do not replace observation
Lifecycle work SHALL not block input, collection or quitting. Outcomes SHALL be shown separately from source diagnostics, with no optimistic row removal. Managed controls SHALL remain unavailable until owner implementation acceptance is confirmed and SHALL refuse at runtime when its transport is unavailable.

#### Scenario: Owner control unavailable
- **WHEN** the owner implementation is unconfirmed or no trusted transport exists
- **THEN** managed actions explain their unavailability while observation and focus continue

#### Scenario: Applied operation
- **WHEN** a valid closed or restarted result arrives
- **THEN** Radar reports it and reconciles rows through subsequent inventory rather than fabricating runtime state
