## MODIFIED Requirements

### Requirement: Launch specifications are explicit private records
Radar SHALL retain a private executable, argv, cwd and environment restart recipe captured from the verified managed execution. Either launched or adopted sessions SHALL be eligible for the same lifecycle controls when their evidence requirements are met. Restart SHALL resume the selected subject's fresh current session without replaying an initial prompt or one-use launch credentials. Titles and scrollback SHALL NOT supply launch recipes. Public reads SHALL omit private launch material. Ordinary baseline reporting SHALL execute nothing.

#### Scenario: Register saved-session launch
- **WHEN** a trusted publisher supplies a launch specification
- **THEN** it is durably stored without spawning, typing into or closing a pane

#### Scenario: Missing launch specification
- **WHEN** an agent has no explicit launch specification
- **THEN** its record states unavailable and no resume command is inferred

#### Scenario: Adopt a running lead
- **WHEN** Radar verifies an eligible Pi session's process and pane and captures its launch context
- **THEN** it records a managed session subject with no parent if the session was untracked, without requiring prior launch intent

#### Scenario: Unsafe restart recipe
- **WHEN** the captured recipe cannot safely resume the selected session without replaying a possible initial prompt
- **THEN** restart refuses before terminating the process

### Requirement: Current session context is mutable and fenced
The daemon SHALL accept current session context separately from registration, with one writer at a time, strictly forward sequence, bounded lease and daemon-time freshness. A session change SHALL NOT change the process registration identity, but SHALL resolve its active managed association to the selected session's subject. Context replay SHALL NOT extend its lease. Effects SHALL validate the current subject attachment, not only the unchanged process identity.

#### Scenario: A process switches or forks its session
- **WHEN** a registered publisher reports a newly selected session
- **THEN** its registration is unchanged and its active subject becomes the subject for that session

#### Scenario: Another handle tries to take the record
- **WHEN** a handle that is not the current writer reports context before retirement or lease expiry
- **THEN** the daemon refuses it and the incumbent's record is unchanged

#### Scenario: One process changes session
- **WHEN** a registered publisher reports a newly selected session
- **THEN** its process registration remains unchanged and its active subject becomes the subject for that session
- **AND** the previous subject retains its own children, inbox and history

#### Scenario: An equal sequence repeats identical content
- **WHEN** a publisher replays the same sequence and content after an uncertain acknowledgment
- **THEN** the daemon returns the stored context without extending its lease or duplicating an established binding

#### Scenario: Another writer tries to take the record
- **WHEN** a noncurrent writer reports context before retirement or lease expiry
- **THEN** the daemon refuses it and the incumbent's context is unchanged

#### Scenario: Session changes while a control is pending
- **WHEN** a process switches away from the subject named by a pending effect before dispatch
- **THEN** the effect does not execute against that process on the strength of PID/birth identity alone

## ADDED Requirements

### Requirement: Managed Pi subjects identify tracked sessions
A Pi subject SHALL be Radar's durable identity for one canonical Pi session UUID, independent of process, session-file path and pane. One subject SHALL have at most one accepted active process attachment; one process SHALL have at most one current session-subject attachment. A new session SHALL NOT inherit another session's parent or inbox solely because it occupies the same process. Missing durable authority SHALL NOT be recreated from Pi custom entries.

#### Scenario: A new process resumes a known session
- **WHEN** a fresh process reports a tracked session and the previous attachment is released or verified no longer active for that session
- **THEN** Radar binds it to the existing subject and retains that subject's relationships and inbox identity

#### Scenario: An untracked session is opened
- **WHEN** eligible Pi reports a session absent from Radar's durable records
- **THEN** Radar creates a new subject rather than recovering authority from conversation contents

### Requirement: Pi enrollment uses one verified binding path
pi-radar SHALL request automatic management by default through its process-local registration/context reporting. Radar SHALL apply the same binding rules to adoption, correlated launch registration, explicit enrollment and reconciliation. Binding SHALL require verified process birth identity, verified pane occupancy and fresh canonical current-session evidence. Herdsman SHALL consume pi-radar's current association rather than run another registrar.

#### Scenario: Manual Pi becomes managed
- **WHEN** pi-radar reports an eligible manually started Pi session
- **THEN** it receives a managed subject association without operator enrollment or Herdsman

#### Scenario: Retried report or daemon restart
- **WHEN** an unchanged attachment is reported after a lost reply or daemon restart
- **THEN** Radar restores or returns the recorded binding without creating another subject or repeating a launch

#### Scenario: Verification is unavailable
- **WHEN** required process/location evidence is unavailable
- **THEN** baseline publication remains possible but management is visibly unresolved

### Requirement: Supplied launch correlation constrains binding
Radar SHALL validate supplied launch correlation against its admitted launch and process evidence before binding. Invalid correlation SHALL NOT fall back to adoption. A validated launch and an adopted session SHALL converge on the same binding representation. Dispatch, managed registration and application readiness SHALL remain distinct outcomes.

#### Scenario: Child registers after launch
- **WHEN** a child registers with valid launch correlation and matching evidence
- **THEN** Radar binds its session subject with the recorded parent relationship rather than creating an unrelated lead

#### Scenario: Correlation is invalid
- **WHEN** correlation is invalid or identifies a launch inconsistent with the reported process
- **THEN** Radar refuses binding without inferring launch success or creating an alternative subject

### Requirement: Duplicate live sessions remain preliminary
A process reporting a session with another accepted live attachment SHALL remain baseline-visible with a duplicate-session warning and without that subject's management or inbox authority. The conflict SHALL NOT auto-promote the duplicate: reconsideration requires a fresh resume from persisted session state or a process restart, then ordinary verification. Heartbeats, daemon restart and extension reload SHALL NOT clear it, and unavailable observations SHALL NOT establish incumbent release.

#### Scenario: Incumbent exits after duplicate opens
- **WHEN** a duplicate remains running after the incumbent exits
- **THEN** it remains unmanaged until a fresh session load or restart makes it eligible for verification again

#### Scenario: Host cannot distinguish fresh session loading
- **WHEN** the Pi adapter cannot reliably distinguish a fresh load from reconnect or extension reload
- **THEN** the duplicate must restart its process rather than receive a guessed promotion

### Requirement: Relationships and receipt authority follow the session subject
Logical parent relationships SHALL remain Radar-owned durable subject relationships rather than depend on the original parent registration. Execution-specific spawn and communication provenance SHALL remain historical records. Handoff access SHALL resolve through the caller's current accepted subject binding. Receipt SHALL NOT imply application settlement or permission to close a worker.

#### Scenario: Lead resumes in a different tab
- **WHEN** an eligible fresh lead attaches to its session subject in another tab
- **THEN** its child links and inbox identity remain and the UI retains a basic parent association while rows stay in their physical tabs
- **AND** registration moves no pane and requires no reunion operation

#### Scenario: Session switch changes inbox access
- **WHEN** a process switches from session A to session B
- **THEN** its active association no longer grants A's inbox rights, and A's pending messages remain addressed to A's subject

#### Scenario: Child process is replaced
- **WHEN** a fresh process validly attaches to an existing child session subject
- **THEN** it retains the recorded parent rather than inheriting parentage from its new pane or caller-supplied claims

### Requirement: Session breadcrumbs are not a management protocol
pi-radar SHALL NOT write or require Pi-session launch identity markers. Historical entries SHALL NOT gate controls, restore authority or produce routine marker-mismatch state. Initial launch correlation, process verification, writer leases and effect fencing SHALL remain independent of session entries.

#### Scenario: Old marker exists or host lacks custom entries
- **WHEN** a session contains historical launch markers or the host cannot write custom session entries
- **THEN** ordinary binding behavior is unchanged and no old execution identity is inherited

### Requirement: Legacy subject adaptation preserves authority
Radar SHALL preserve existing subject IDs, operation history, inboxes and parentage when establishing an unambiguous session association. It SHALL NOT silently merge conflicting legacy subjects or infer missing session evidence. Rollback support SHALL be documented according to persisted-format compatibility rather than presumed.

#### Scenario: Multiple old subjects name one session
- **WHEN** legacy records give conflicting subject identities for the same session
- **THEN** Radar exposes an unresolved association without merging or deleting their authority
