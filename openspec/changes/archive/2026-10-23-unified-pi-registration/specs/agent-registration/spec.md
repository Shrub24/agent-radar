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
pi-radar SHALL report process birth identity, current location, session UUID and a session-load claim ID through one presence operation, with acknowledged `registration_id` and `subject_id` when known. Radar SHALL resolve internal process registration, mutable context and durable session subject separately. It SHALL return `registration_id`, claim ID and `granted`, `duplicate`, `pending` or `released` standing; only a grant includes `subject_id`. Retries SHALL reuse the claim ID. The public process identifier SHALL be `registration_id`, without an `agent_id` compatibility alias. Generic publisher registration/context operations SHALL NOT be pi-radar's session-management path.

#### Scenario: Reconnect reasserts a claim
- **WHEN** pi-radar renews or reconnects for the same loaded session
- **THEN** Radar returns the stored decision without allocating another subject or creating a fresh admission opportunity

#### Scenario: A process switches or forks its session
- **WHEN** the same process reports a new session-load claim
- **THEN** its process registration remains unchanged, the new session is resolved to its own subject, and the previous subject retains its children, inbox and history

#### Scenario: An equal sequence repeats identical content
- **WHEN** a publisher replays the same sequence and content after an uncertain acknowledgment on a separately fenced context channel
- **THEN** the daemon returns the stored context without extending its lease

#### Scenario: Another handle tries to take the record
- **WHEN** a handle that is not the current context writer reports before retirement or lease expiry
- **THEN** the daemon refuses it and leaves the incumbent record unchanged

#### Scenario: Process location changes
- **WHEN** the same verified process reports a new pane
- **THEN** Radar updates mutable location without changing process registration or subject identity solely because of placement

#### Scenario: Another process presents an acknowledged registration
- **WHEN** the report's registration ID belongs to another process birth identity
- **THEN** Radar refuses the report and preserves the current attachment

#### Scenario: Verification is temporarily unavailable
- **WHEN** process or pane evidence cannot be established
- **THEN** Radar retains `pending` standing, publishes no subject authority, and permits retry with the same claim

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
pi-radar SHALL request automatic management for each eligible Pi session through `agent.presence`. Radar SHALL require verified process birth identity, pane occupancy and fresh session evidence. Manual adoption, managed launch and reconnect SHALL use this same report path. Herdsman SHALL consume the returned association and SHALL NOT run enrollment or register with the daemon.

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
Radar SHALL durably record duplicate claim decisions, keep those registrations baseline-visible with a distinct inline warning, and grant no subject or inbox authority. Renewal, incumbent release and daemon restart SHALL NOT promote the rejected claim. Only a fresh Pi session-load claim SHALL permit reconsideration.

#### Scenario: Incumbent exits after duplicate opens
- **WHEN** a duplicate remains running after the incumbent exits
- **THEN** it remains unmanaged until a fresh session load or restart makes it eligible for verification again

#### Scenario: Verification is temporarily unavailable
- **WHEN** process verification or daemon recovery cannot establish whether a claim can be granted
- **THEN** its standing is pending and the same claim may retry without a fresh session load

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
