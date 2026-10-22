# Spec Delta

## MODIFIED Requirements

### Requirement: Launch specifications are explicit private records
Radar SHALL hold a private executable, argv, cwd and environment recipe for a managed execution, captured when the daemon admits its launch. It SHALL store this privately and SHALL NOT reconstruct it from titles, scrollback or process arguments. Public snapshot reads SHALL omit the executable arguments, environment and session paths. Registration and ordinary publication SHALL execute nothing.

#### Scenario: Register saved-session launch
- **WHEN** a trusted publisher supplies a launch specification
- **THEN** it is durably stored without spawning, typing into or closing a pane

#### Scenario: Missing launch specification
- **WHEN** an agent has no explicit launch specification
- **THEN** its record states unavailable and no resume command is inferred

#### Scenario: Recipe captured at managed launch
- **WHEN** the daemon admits a managed launch
- **THEN** the recipe it records is the one it is about to execute, not one inferred afterwards

### Requirement: Registration may carry a single-use spawn token
A registration MAY present the pre-minted claim of the attempt that launched it, privately, with its own source, incarnation and process claim. Registration SHALL consume that claim only for the exact registration whose verified birth identity matches the recorded expected one, and identical retry SHALL remain idempotent. One launch SHALL present one credential, so the per-edge token retires rather than becoming a second one. The publisher SHALL remove the claim id from its launch environment after capture.

#### Scenario: A registration binds its spawn
- **WHEN** a registration presents the claim of a pending attempt it was launched with
- **THEN** that attempt binds to that exact registration and reports the location it was launched into

#### Scenario: A public read hides the token
- **WHEN** any agent registration, get or list read is served
- **THEN** no claim id of a pending attempt and no launch credential appear in the response

#### Scenario: A spent or malformed token
- **WHEN** a registration presents a claim that already bound a process or is not a valid claim id
- **THEN** the registration is refused and nothing binds

#### Scenario: A foreign process presents the claim
- **WHEN** a registration presents a claim whose recorded expected birth identity is not that process
- **THEN** the registration is refused and the claim keeps its recorded expectation

#### Scenario: Claim id is not persisted
- **WHEN** a launched process has captured its claim id
- **THEN** the claim id appears in no sidecar, fact, label or log the publisher writes

### Requirement: Spawn bootstrap reuses exact registration
A launched publisher SHALL present its pre-minted claim privately in registration with its own source, incarnation and process claim. Successful registration SHALL consume that claim for that exact registration, with identical retry remaining idempotent. The publisher SHALL remove the claim id from its launch environment after capture and SHALL NOT persist it in sidecars, facts, labels or logs.

#### Scenario: Child bootstrap
- **WHEN** a launched child registers with its own incarnation and the claim it was launched with
- **THEN** the returned registration id identifies that registration and the attempt binds to it

#### Scenario: Registration answer is lost
- **WHEN** the child repeats the identical registration after an uncertain acknowledgement
- **THEN** it receives the existing registration and no second binding or registration is created

#### Scenario: Token reused by another incarnation
- **WHEN** another incarnation attempts to consume a claim that is already bound
- **THEN** registration refuses and the original binding remains unchanged

#### Scenario: Claim presented after its process exited
- **WHEN** a registration presents the claim of an attempt whose process is recorded as exited
- **THEN** registration refuses and the recorded attempt keeps its own process

## ADDED Requirements

### Requirement: Managed standing requires an admitted or recovered execution
Radar SHALL treat managed lifecycle standing as a consequence of an execution it admitted through its managed launch path or of a previously recorded execution it reverified. An externally started process MAY be registered, observed and retain its durable session identity, children and inbox, but SHALL NOT be reported as a managed execution. Every managed launch SHALL be admitted durably before it executes.

#### Scenario: Managed launch
- **WHEN** Radar admits and executes a launch through its managed path
- **THEN** the execution is managed and its recipe is recorded

#### Scenario: Agent started outside Radar
- **WHEN** a Pi process reports itself without an admitted or recovered execution
- **THEN** Radar reports it as observed and offers no managed execution controls, while its session identity and history remain intact

#### Scenario: Admission precedes execution
- **WHEN** the launch record cannot be made durable
- **THEN** the command does not execute

### Requirement: A managed launch pre-mints its attempt's claim
Radar SHALL mint the claim for a managed launch before the launch is dispatched, recording that attempt's expected process birth identity, and SHALL embed the claim id in the launched environment as the identity the process presents. Registration SHALL bind that claim only when the presented id names an unspent pre-minted claim for the same subject and the registration's verified birth identity equals the expected one.

#### Scenario: Launch pre-mints its claim
- **WHEN** Radar admits a managed launch
- **THEN** the claim exists with its expected process identity before the command executes, and the launched process is given its id

#### Scenario: The child presents the claim
- **WHEN** the launched process registers presenting its claim id
- **THEN** the claim binds to that registration and the subject's current claim is the pre-minted one

#### Scenario: A leaked claim id cannot bind another process
- **WHEN** a process presents a claim id whose recorded birth identity is not that process
- **THEN** binding is refused and the claim is unchanged

#### Scenario: A pending claim is not published
- **WHEN** a pre-minted claim has no bound process yet
- **THEN** its id appears in no list or snapshot read, and only the requesting caller and the launched process hold it

#### Scenario: The first pre-mint wins
- **WHEN** two launch paths, including a restart, pre-mint a claim for one subject
- **THEN** exactly one becomes current and the other is refused or left pending rather than replacing it

#### Scenario: No generation identity
- **WHEN** a subject's attempts are read in order
- **THEN** they are ordered by their claim records and current pointer, and no attempt is identified or fenced by an ordinal

### Requirement: A claim is the record of one attempt
A claim SHALL record one attempt's expected and bound process, its recipe, its signal and exit evidence, and its successor. Exactly one claim per subject SHALL be current. An attempt SHALL NOT be identified or fenced by an ordinal generation.

#### Scenario: Attempt facts stay with the attempt
- **WHEN** a subject's attempt is read after a daemon restart
- **THEN** that attempt's process, recipe, signal, exit and successor are read from its own claim

#### Scenario: Current claim is a pointer
- **WHEN** a subject's latest attempt is needed
- **THEN** it is named by the subject's current claim rather than found by comparing ordinals

### Requirement: Subject identity outlives its attempts
Radar SHALL hold the durable subject as a record carrying its identity, its managed metadata and its parent subject, and SHALL NOT copy those facts forward onto each attempt. A subject's identity SHALL NOT be derived from a registration, a pane or a launch, and releasing an attachment SHALL NOT delete the subject or its history.

#### Scenario: Successor keeps the subject
- **WHEN** a subject's process is replaced by a new attempt
- **THEN** the subject id and its managed metadata are unchanged and only the current claim pointer moves

#### Scenario: Release leaves the attempt readable
- **WHEN** the attachment is released while the process still runs
- **THEN** the attempt record and its facts remain readable and the subject is no longer attached

#### Scenario: Existing records are promoted, not re-keyed
- **WHEN** an execution record written before this consolidation is read
- **THEN** its existing id becomes its claim id and its stored logical subject is promoted to the subject record

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
