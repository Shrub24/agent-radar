# Spec Delta

## MODIFIED Requirements

### Requirement: Launch specifications are explicit private records
Radar SHALL hold a private executable, argv, cwd and environment recipe for a managed execution, captured when the daemon admits its launch, and SHALL NOT reconstruct it from titles, scrollback or process arguments. Public reads SHALL omit it. Registration and ordinary publication SHALL execute nothing.

#### Scenario: Register saved-session launch
- **WHEN** a trusted publisher supplies a launch specification
- **THEN** it is durably stored without spawning, typing into or closing a pane

#### Scenario: Missing launch specification
- **WHEN** an agent has no explicit launch specification
- **THEN** its record states unavailable and no resume command is inferred

#### Scenario: Adopt a running lead
- **WHEN** Radar verifies an eligible Pi session's process and pane and captures its launch context
- **THEN** it records a managed session subject with no parent if the session was untracked, without requiring prior launch intent

#### Scenario: Recipe captured at managed launch
- **WHEN** the daemon admits a managed launch
- **THEN** the recipe it records is the one it is about to execute, not one inferred afterwards

## ADDED Requirements

### Requirement: Managed launch uses a pre-minted process registration
A managed process SHALL present the registration id Radar allocated before its launch, with its source, incarnation and process birth identity. Radar SHALL bind that registration only when its verified birth identity matches the launch's captured identity; identical retry SHALL remain idempotent. The publisher SHALL capture the registration id from its launch environment and remove it before launching children. Neither a claim id nor a separate spawn credential is required from the publisher.

#### Scenario: A registration binds its spawn
- **WHEN** a launched process presents its pre-minted registration id
- **THEN** Radar verifies its birth identity against the captured launch and returns that same registration id

#### Scenario: Reserved identity is visible
- **WHEN** the launch has not received a verified presence report
- **THEN** its reserved claim and process registration remain visible and the operator can cancel the launch

#### Scenario: Another process presents the registration
- **WHEN** a process presents a registration id whose captured birth identity differs from its own
- **THEN** binding is refused and the original launch identity is unchanged

#### Scenario: Malformed or unknown registration
- **WHEN** a process supplies a malformed registration id or one with no admitted launch or existing binding
- **THEN** Radar refuses rather than falling back to adoption

### Requirement: Presence binds the launch-issued registration
A launched publisher SHALL present its pre-minted registration id with its source, incarnation, current session and process birth identity. Radar SHALL resolve the existing launch through that registration and SHALL NOT require a claim from the launch environment. Every fresh session load SHALL use an ordinary fresh client claim, and renewals SHALL reuse that claim.

#### Scenario: Child bootstrap
- **WHEN** a launched child reports its own incarnation and the registration it was launched with
- **THEN** the returned registration id is the pre-minted one and the reserved attachment becomes an ordinary granted client claim

#### Scenario: Registration answer is lost
- **WHEN** the child repeats the identical registration after an uncertain acknowledgement
- **THEN** it receives the existing registration and no second binding or registration is created

#### Scenario: Registration reused by another incarnation
- **WHEN** another incarnation attempts to bind an existing registration
- **THEN** registration refuses and the original binding remains unchanged

#### Scenario: Registration presented after its process exited
- **WHEN** a process presents the registration of a launch whose captured process has exited
- **THEN** registration refuses and the recorded launch keeps its own process

### Requirement: Managed standing requires an admitted or recovered execution
Radar SHALL treat managed lifecycle standing as a consequence of an execution it admitted through its managed launch path or of a previously recorded execution it reverified. An externally started process MAY be registered, observed and retain its durable session identity, children and inbox, but SHALL NOT be reported as a managed execution. Every managed launch SHALL be admitted durably before it executes.

#### Scenario: Managed launch
- **WHEN** Radar admits and executes a launch through its managed path
- **THEN** the execution is managed and its recipe is recorded

#### Scenario: Agent started outside Radar
- **WHEN** a Pi process reports itself without an admitted or recovered execution
- **THEN** Radar marks it as observed, preserves its existing functionality and session history, and offers no new scope-dependent capability or substitute implementation

#### Scenario: Admission precedes execution
- **WHEN** the launch record cannot be made durable
- **THEN** the command does not execute

### Requirement: Managed launch pre-mints a registration and optionally reserves a claim
Radar SHALL allocate a process registration before dispatch. For a launch associated with a subject, it SHALL reserve that subject's attachment in standing `reserved` before dispatch. A reservation excludes a competing launch but grants no attachment authority. `pending` SHALL remain the standing for a client report still being verified, not a launch reservation. A launch without a subject SHALL NOT fabricate a claim.

#### Scenario: Launch reserves an attachment
- **WHEN** Radar admits a launch for a subject
- **THEN** its registration and reserved claim exist before dispatch and only its registration id is injected into the child's environment

#### Scenario: Child reports a session
- **WHEN** the child presents its pre-minted registration, verified birth identity, current session and fresh client claim
- **THEN** Radar checks session exclusivity and replaces the reservation with the granted client claim

#### Scenario: Reservation is visible and controllable
- **WHEN** a reserved launch is still starting
- **THEN** Radar shows it as starting and the operator can cancel it without granting handoff or inbox authority

#### Scenario: The first reservation wins
- **WHEN** two launch paths, including a restart, reserve an attachment for one subject
- **THEN** exactly one becomes current and the other is refused rather than replacing it

#### Scenario: No generation identity
- **WHEN** a subject's attachment is resolved
- **THEN** it is identified by its exact current claim, not an ordinal

### Requirement: A claim records a subject attachment
A claim SHALL associate its claim id, subject id and process registration id with the attachment's standing and provenance. A subject SHALL have at most one current claim. A process may change its session attachment without changing its registration or physical group. Claims SHALL NOT own process-lifetime facts.

#### Scenario: Attachment changes within a process
- **WHEN** a process reloads or opens another session
- **THEN** the ordinary presence path creates a fresh claim under the same registration and a delayed release cannot revoke the new claim

#### Scenario: Current claim is a pointer
- **WHEN** a subject's attachment is needed
- **THEN** it is named by the subject's current claim rather than found by comparing ordinals

### Requirement: Subject identity outlives its attempts
Radar SHALL hold the durable subject as a record carrying its identity, its managed metadata and its parent subject, and SHALL NOT copy those facts forward onto each attempt. A subject's identity SHALL NOT be derived from a registration, a pane or a launch, and releasing an attachment SHALL NOT delete the subject or its history.

#### Scenario: Successor keeps the subject
- **WHEN** a subject's process is replaced by a new attempt
- **THEN** the subject id and its managed metadata are unchanged and only the current claim pointer moves

#### Scenario: Release leaves the attempt readable
- **WHEN** the attachment is released while the process still runs
- **THEN** the attempt record and its facts remain readable and the subject is no longer attached

### Requirement: A launch records its durable parent and reads as the caller's own
A managed launch SHALL record the logical parent subject that requested it on its spawn edge when the launch is admitted, from the identity the daemon validated. A caller SHALL read its own launches without naming a parent: the daemon SHALL resolve the caller's current subject and page only that subject's edges. A read SHALL NOT accept another subject's id from a caller as a parent. Parameters and results of this read SHALL be published as generated schemas.

#### Scenario: Parentage survives a session change
- **WHEN** a parent resumes its session elsewhere and reads its launches
- **THEN** the edges it launched are still its own, and no session id was needed to find them

#### Scenario: Another subject's launches are not readable
- **WHEN** a caller asks for launches that belong to a different subject
- **THEN** the read is refused and no page of that subject's edges is returned

#### Scenario: Unmanaged parent
- **WHEN** a launch was requested by a caller with no subject
- **THEN** its edge records no parent subject rather than a session or a registration standing in for one

## REMOVED Requirements

### Requirement: Registration may carry a single-use spawn token
**Reason**: A managed launch now supplies its pre-minted registration id, not a secret token or session claim. Public registration identity is safe to expose; binding requires the captured process birth identity.

**Migration**: pi-radar captures `PI_RADAR_REGISTRATION_ID` and reports it through presence. It no longer consumes an env claim or sends `spawn_token` in presence.

### Requirement: Spawn bootstrap reuses exact registration
**Reason**: The token-based bootstrap protocol is replaced by presence binding the registration Radar allocated before launch. Ordinary fresh session claims use the same path for managed and observed processes.

**Migration**: Use the launch-issued registration on the first report and subsequent reports; do not substitute it for a session claim id.
