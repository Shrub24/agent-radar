# Spec Delta

## MODIFIED Requirements

### Requirement: A subject has one live claim, reserved or granted
A claim SHALL be reserved before its process exists only for a successor to an existing subject, such as a restart. Reserving and granting SHALL pass through one transition that refuses while the subject has any other live claim, reserved or granted; a grant SHALL replace only the reservation naming the presenting process. A launch with no existing subject SHALL reserve an execution and no claim.

#### Scenario: A second reservation
- **WHEN** a subject holds a reserved or granted claim and another launch tries to reserve it
- **THEN** the second reservation is refused

#### Scenario: A grant against another's reservation
- **WHEN** a process presents a session whose subject is reserved for a different process
- **THEN** the report is refused as a duplicate and the reservation is unchanged

#### Scenario: A launch with no subject reserves no claim
- **WHEN** a configured launch is reserved
- **THEN** no claim exists for it until its program reports a session

### Requirement: The daemon withdraws a launch that never presents
Radar SHALL withdraw a managed launch whose execution has not been attached to a subject within a fixed ceiling counted from the execution's recorded reservation time: stop the captured process through its scope, close the launch's own pane, and record the withdrawal and its reason on the execution. The ceiling SHALL NOT be a request parameter. Withdrawing a restart's successor SHALL revoke its reserved claim and leave the subject free to be continued.

#### Scenario: A child never presents
- **WHEN** a launched process has not presented within the ceiling
- **THEN** its process is stopped, its pane is closed and the execution reads as withdrawn with a reason

#### Scenario: A child presents as the ceiling passes
- **WHEN** a launch's first report and the withdrawal arrive together
- **THEN** exactly one wins: an attached execution is stopped as an attached execution, and a withdrawn launch refuses the late report

#### Scenario: The daemon restarts during a launch
- **WHEN** the daemon restarts before a launch presents
- **THEN** the ceiling still counts from the recorded reservation time and the launch is withdrawn on schedule

#### Scenario: A withdrawn launch frees its subject
- **WHEN** a restart's successor launch is withdrawn
- **THEN** the subject has no live claim and a later continue is accepted
