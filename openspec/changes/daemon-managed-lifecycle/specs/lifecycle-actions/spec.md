## MODIFIED Requirements

### Requirement: Lifecycle actions require separate confirmation
Radar SHALL open separate confirmation for x (selected close), X (tab close), r (managed-subject restart), and any offered enrollment, cancellation or stop action. Opening confirmation SHALL send nothing. It SHALL name the exact target and possible losses, default to Cancel, and require activation of Confirm. Task/workspace rows SHALL NOT redirect destructive actions to their parents.

#### Scenario: Close confirmation
- **WHEN** x is pressed on an eligible row
- **THEN** confirmation names the target and possible process/assignment losses without submitting a request

#### Scenario: Cancel or accidental Enter
- **WHEN** Escape is pressed or the initially selected Cancel is activated
- **THEN** no lifecycle action is submitted

#### Scenario: Mouse confirmation
- **WHEN** the operator clicks Confirm in the drawn dialog
- **THEN** the same frozen-target checks as keyboard confirmation apply
- **AND** tree second-click focus does not confirm lifecycle actions

#### Scenario: Filter entry
- **WHEN** a lifecycle key is typed while editing the filter
- **THEN** it is filter text and opens no confirmation

### Requirement: Managed operations are owner-routed
Radar SHALL submit confirmed managed controls to the daemon's lifecycle authority with the exact subject and expected binding generation. The daemon SHALL revalidate identity and eligibility before effects. Managed restart SHALL be operator-confirmed and require verified exit plus an explicit restartable subject. Missing provenance, unsupported capabilities or changed bindings SHALL refuse; no Herdsman file or direct-mux fallback SHALL execute the control. Herdsman SHALL retain assignment and result authority.

#### Scenario: Idle managed worker restart
- **WHEN** an operator confirms restart of an eligible managed subject
- **THEN** the TUI submits the frozen subject/generation to the daemon and monitors its durable operation

#### Scenario: Busy or parentless target
- **WHEN** a busy or parentless subject is selected for restart
- **THEN** activity or parent absence alone does not authorize restart; the daemon requires explicit managed provenance, current recipe and operator confirmation
- **AND** unsupported or changed generations refuse without owner-file fallback

#### Scenario: Close abandoned assignment
- **WHEN** an operator confirms a physical control that may abandon active work
- **THEN** the warning names that risk and no control outcome declares the assignment complete

### Requirement: Lifecycle results do not replace observation
Lifecycle requests SHALL NOT block input, collection or quitting. Radar SHALL show daemon operation outcomes separately from observation and source diagnostics, without optimistic row removal. Disconnect, local timeout or a lost answer SHALL NOT cancel an accepted daemon operation or imply a result. Uncertain effects SHALL NOT trigger a fresh mutation or another transport. Unbound or unjoined registrations SHALL NOT be controlled through their merely reported pane.

#### Scenario: Control-plane outcome unknown
- **WHEN** a managed operation may have been accepted but its answer is lost
- **THEN** Radar reads the same request ID and neither retries through a different route nor fabricates row removal

#### Scenario: Owner control unavailable
- **WHEN** the daemon lifecycle implementation is unsupported or unavailable
- **THEN** managed controls explain their unavailability while observation and focus continue, with no owner-file fallback

#### Scenario: Applied operation
- **WHEN** a durable daemon operation reports its confirmed effects
- **THEN** Radar displays them and reconciles through observations rather than fabricating rows

#### Scenario: Operator quits
- **WHEN** Radar exits while a managed operation is pending
- **THEN** quitting is responsive and the daemon retains operation ownership

#### Scenario: Unjoined registration
- **WHEN** a registration cannot be joined to verified physical evidence
- **THEN** a pane-targeted control refuses; an explicit daemon-managed launch reference may still support cancellation through its own verified evidence

## REMOVED Requirements

### Requirement: Owner file transport preserves trust and identity
**Reason:** Managed physical lifecycle now belongs to the daemon, not the TUI's Herdsman requester.
**Migration:** Use the trusted daemon socket and capability-gated lifecycle requests. Do not remove Herdsman's own mailbox/result contracts.

### Requirement: Outcomes come from files, never local timeout
**Reason:** Managed controls use durable daemon operations instead of TUI-owned file claims and expiry polling.
**Migration:** Read the daemon operation by request ID; retain unknown outcomes and duplicate suppression. Local timeout remains non-authoritative.
