# Spec Delta

## RENAMED Requirements

- FROM: `### Requirement: A subject has one live claim, reserved or granted`
- TO: `### Requirement: A subject separates its granted claim from its reservation`

## ADDED Requirements

### Requirement: Radar owns attachment identity across presence retries
Radar SHALL mint and return claim identities. Session start SHALL send registration and session, resolving an existing applicable reservation or creating an ordinary claim. The client SHALL persist the returned ID for heartbeats and exact release. Retry of an unanswered start SHALL recover its applicable recorded decision. Reports naming terminal claims SHALL NOT resurrect them.

#### Scenario: A successor discovers its reserved claim
- **WHEN** the verified successor reports presence without knowing its reserved claim ID
- **THEN** Radar resolves and returns that same claim ID without requiring a claim-ID launch environment variable

#### Scenario: A first presence response is lost
- **WHEN** a client retries the same attachment attempt after the first response is lost
- **THEN** Radar returns the recorded claim identity and applicable standing without creating another attachment

#### Scenario: A released session is attached again
- **WHEN** the same registration legitimately begins a new attachment attempt for the same session after release
- **THEN** the new attempt is distinct and delayed reports from the released attempt cannot restore its authority

#### Scenario: A daemon restarts between receipt and client persistence
- **WHEN** a presence decision was committed but its response was not persisted by the client before a daemon restart
- **THEN** retry recovers the same decision and claim identity without executing lifecycle effects during replay

### Requirement: Standing properties do not replace attachment authorisation
Claim standing SHALL distinguish reservation, pending verification, grant, denial and release. Shared policy questions SHALL use derived semantic predicates. Granted standing alone SHALL NOT authorise operations without matching subject and execution evidence. Reserve and grant transitions SHALL enforce their different exclusion rules; independent stored authority flags SHALL NOT be introduced.

#### Scenario: A granted claim is no longer current
- **WHEN** a claim has granted standing but no longer matches the subject's current grant
- **THEN** the authority resolver refuses to treat it as the subject's authorised attachment

#### Scenario: A reservation coexists with an incumbent grant
- **WHEN** a subject has both an incumbent granted claim and a successor reservation
- **THEN** the incumbent remains authorised, another reservation is excluded and successor grant remains blocked until incumbent release

### Requirement: Attachment lookup is derived from authoritative claims
Claim records SHALL own attachment identity and standing. Current grant, reservation and granted history SHALL be resolved through maintained indexes derived from those records, not independently writable subject pointers. Transitions SHALL update the indexes under serialisation; startup SHALL rebuild them and reject incompatible authority. Queries SHALL NOT reconstruct ownership or opportunistically inspect processes to release another claim.

#### Scenario: An attachment transition is interrupted
- **WHEN** the daemon restarts after committing a claim transition but before updating its in-memory index
- **THEN** startup rebuilds that index from authoritative history without dispatching process effects

#### Scenario: A reservation cancellation is interrupted
- **WHEN** a cancellation is durable and the daemon restarts before its derived lookup is updated
- **THEN** rebuilding excludes the cancelled reservation and preserves later attachments

#### Scenario: Claim admission encounters an incumbent
- **WHEN** a session start encounters another granted claim in the maintained model
- **THEN** it refuses the conflicting grant without invoking admission-time process reconciliation

### Requirement: Session boundaries release and establish exact attachments
The integration SHALL preserve Pi's ordered awaited shutdown/start hooks, including returning asynchronous handler promises. Shutdown SHALL release the exact session claim; start SHALL establish the next attachment. Reload MAY grant a new claim for the same registration and session after shutdown releases the old one. Start reason SHALL describe intent, not identity. A quit reason alone SHALL NOT establish process exit.

#### Scenario: Same-session reload
- **WHEN** Pi emits awaited shutdown followed by start with reason reload for the same session
- **THEN** Radar finishes the old exact-claim release before establishing a new claim on the unchanged registration and subject

#### Scenario: The predecessor releases after a successor reservation
- **WHEN** shutdown releases a claim whose successor is already reserved on another registration
- **THEN** only the named predecessor claim releases and the reservation remains unchanged, without a Reserved skip in generic replacement logic

### Requirement: Registration finalisation settles its outstanding claims
Session shutdown SHALL use the shared exact-claim release transition. Verified execution end and failed-launch withdrawal SHALL use shared explicit registration finalisation to settle outstanding attachments and applicable launch-owned resources. Finalisation SHALL be idempotent through existing durable recovery seams, retain history and prevent late grant on a terminal execution. It SHALL NOT introduce a separate workflow engine or completion store.

#### Scenario: A process exits without session shutdown
- **WHEN** the observer verifies the registered process has ended
- **THEN** its execution end is recorded and its outstanding claims settle without waiting for a new claimant to discover its death

#### Scenario: Finalisation is interrupted
- **WHEN** terminal execution evidence is durable but its claim or resource cleanup has not settled
- **THEN** existing recovery resumes idempotent finalisation without clearing another registration's claims or repeating established external effects

### Requirement: Reservation mismatch fails the intended operation
When a verified registration presents a session inconsistent with its own reservation, Radar SHALL settle the mismatched reservation, record failure of the intended restart or continuation and surface it in the TUI. It SHALL apply ordinary attachment eligibility to the actual session. It SHALL NOT automatically retry the failed intent or cancel another registration's reservation.

#### Scenario: Restart presents a different session
- **WHEN** the successor presents a session incompatible with its reserved subject
- **THEN** the intended restart fails visibly, its reservation settles and the actual session follows ordinary attachment eligibility without counting as restart success

#### Scenario: Actual session belongs to another reservation
- **WHEN** the presented session resolves to a subject reserved for another registration
- **THEN** that reservation remains intact and the conflicting attachment is refused

## MODIFIED Requirements

### Requirement: A subject separates its granted claim from its reservation
A subject SHALL have at most one current granted claim and one exclusive reserved claim. Reserving a successor SHALL preserve the incumbent's grant and block competing reservations and grants. Only the reserved registration MAY fulfil the reservation, after the incumbent releases. Grant SHALL transition the reserved claim to granted and update derived resolution under serialisation. Cancellation SHALL remove only the matching reservation without changing an incumbent grant. Fresh launches without a preassigned subject SHALL reserve no claim.

#### Scenario: A second reservation
- **WHEN** a subject has a reserved successor and another launch tries to reserve it
- **THEN** the second reservation is refused without changing either the reservation or the incumbent grant

#### Scenario: A grant against another's reservation
- **WHEN** a process presents a session whose subject is reserved for a different process
- **THEN** the report is refused as a duplicate and the reservation and incumbent grant are unchanged

#### Scenario: A launch with no subject reserves no claim
- **WHEN** a fresh configured launch is reserved without a preassigned subject
- **THEN** no claim exists for it until its verified process reports a session

#### Scenario: A successor is reserved while the incumbent runs
- **WHEN** restart reserves a successor against the exact current granted claim
- **THEN** the incumbent retains its subject authority and remains current while the successor holds the exclusive reservation

#### Scenario: The successor presents before release
- **WHEN** a reserved successor attempts to become granted while the incumbent grant remains active
- **THEN** the new grant is refused and the incumbent remains authorised

#### Scenario: The successor fulfils its reservation
- **WHEN** the incumbent has released and the verified reserved registration presents the authorised session
- **THEN** its claim becomes current and granted and its reservation is consumed in the same transition

#### Scenario: Stopping the incumbent fails
- **WHEN** restart cannot establish the predecessor's verified exit and cancels its undispatched successor reservation
- **THEN** no successor is launched and the incumbent grant remains unchanged

#### Scenario: A stale release follows a new grant
- **WHEN** a release naming the predecessor claim arrives after the successor has been granted
- **THEN** it cannot release or redirect an operation to the successor

#### Scenario: Grant and reservation survive daemon restart
- **WHEN** the daemon restarts with an incumbent grant and a reserved successor
- **THEN** both standings are recovered distinctly without dispatching lifecycle effects during replay

### Requirement: The daemon withdraws a launch that never presents
Radar SHALL withdraw an unpresented managed launch after a fixed ceiling measured from its recorded execution reservation: stop the captured process through its verified scope, close its verified launch-owned pane, and record the reason on the execution. The ceiling SHALL NOT be a request parameter. Withdrawal SHALL finalise its registration through shared cleanup, settle its outstanding claims, preserve other registrations' grants and never restore authority to an exited predecessor. A settled withdrawal SHALL NOT repeat its effects.

#### Scenario: A child never presents
- **WHEN** a launched process has not presented within the ceiling
- **THEN** its process is stopped, its pane is closed and the execution reads as withdrawn with a reason

#### Scenario: A child presents as the ceiling passes
- **WHEN** a launch's first report and the withdrawal arrive together
- **THEN** the result is serialised: a granted execution uses attached-execution stop semantics, while a withdrawn launch refuses the late report

#### Scenario: The daemon restarts during a launch
- **WHEN** the daemon restarts before a launch presents
- **THEN** the ceiling still counts from the recorded reservation time and the launch is withdrawn on schedule

#### Scenario: A withdrawn launch frees its subject
- **WHEN** a successor is withdrawn after its predecessor exited and released
- **THEN** the subject is detached without a reservation and may be continued, with no predecessor grant restored

#### Scenario: A later attachment must not be cleared
- **WHEN** delayed withdrawal or cancellation refers to a reservation superseded by a later grant or reservation
- **THEN** that later attachment state remains unchanged
