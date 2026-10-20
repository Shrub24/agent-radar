# Spec Delta

## Purpose

Restart an explicitly managed lead or standalone session while preserving its logical session identity and live child relationships. The daemon replaces the live process binding and reports physical outcomes; assignment authority remains with Herdsman.

## ADDED Requirements

### Requirement: Restart preserves logical session identity and replaces only the live binding
The daemon SHALL represent a managed lead/standalone logical session separately from its live process incarnation and mux location. A confirmed restart SHALL bind the replacement process incarnation to the same logical session subject without treating the prior incarnation as current. The logical session identity SHALL NOT be inferred from a PID, pane, title, or an unverified session UUID alone.

#### Scenario: Confirmed lead restart
- **WHEN** an authorized restart completes and the replacement registers against the saved session
- **THEN** the logical session subject remains stable, the replacement process incarnation becomes its current live binding, and the old incarnation is retained only as historical evidence

#### Scenario: Ambiguous session attachment
- **WHEN** the saved session is attached by multiple live processes or the daemon cannot establish which process is the replacement
- **THEN** it SHALL NOT silently choose or overwrite a binding, and the restart result remains refused or unresolved

### Requirement: Restart eligibility comes from Radar's managed-session record
Radar SHALL offer restart only for a managed lead/standalone with a saved launch specification and a session-bound topology whose child relationships can be preserved. Caller-reported PID, role, label and assignment-writer provenance do not establish eligibility.

#### Scenario: Legacy process-parented child edge
- **WHEN** the target session has a v1 child edge without session-parent identity
- **THEN** restart is unavailable until an explicit, evidence-backed migration associates that edge; no session parent is inferred

#### Scenario: Missing launch specification
- **WHEN** the selected managed subject has no saved launch specification
- **THEN** restart is unavailable and no effect occurs

### Requirement: Restart is an explicitly confirmed TUI action
The TUI SHALL revalidate the selected logical session and exact process birth identity before dispatch. Restart SHALL NOT be exposed as a public daemon method, CLI command or Herdsman operation. Until Herdr provides positive pane-ready/shell-ready evidence for a fresh pane, Radar SHALL keep restart unavailable and SHALL NOT create a pane then refuse or infer readiness from timing, output, or marker injection.

#### Scenario: Restart requested outside the TUI
- **WHEN** a caller attempts restart through a public daemon protocol method, CLI command or Herdsman operation
- **THEN** no such restart surface is available

#### Scenario: Target identity changed after confirmation opened
- **WHEN** the selected logical session or process birth identity differs at confirmation
- **THEN** restart is refused before dispatch

#### Scenario: Herdr cannot establish fresh pane readiness
- **WHEN** a restart target is selected but the runtime exposes no positive readiness acknowledgement for a newly created pane
- **THEN** no restart action is offered and no pane or process effect occurs; Radar explains the missing capability

### Requirement: Restart stops the old incarnation gracefully before replacement
The daemon SHALL request graceful termination of the exact old process through an incarnation-bound process handle and launch a replacement only after positive evidence that the old process has stopped. It SHALL NOT escalate to a forced kill. A process that remains live past the bounded graceful-stop interval, or whose identity or exit cannot be verified, SHALL block replacement launch.

#### Scenario: Prior process may still be live
- **WHEN** the old process birth identity is still present after the graceful-stop interval or cannot be checked conclusively
- **THEN** the daemon does not launch a replacement and reports refusal or unresolved verification

#### Scenario: Graceful stop times out
- **WHEN** the old process has not exited by the bounded graceful-stop deadline
- **THEN** the daemon does not send a forced signal and does not launch a replacement

### Requirement: Restart operation outcomes are durable and non-retrying
An accepted restart SHALL have a durable identity and result. A possibly dispatched stop or launch SHALL NOT be retried automatically, and an uncertain effect SHALL remain unknown until independently reconciled.

#### Scenario: Lost launch response
- **WHEN** replacement launch may have been dispatched but its result is not confirmed
- **THEN** the durable operation reads unknown and is not automatically replayed as another launch

### Requirement: Live managed children survive lead restart unchanged
Restarting a lead SHALL NOT close, kill, relaunch, re-register or rewrite the runtime topology of its live managed children. Child relationships SHALL be attached to the stable logical parent session subject, not the replaceable parent process incarnation. The daemon SHALL preserve each child’s own identity and report current child liveness/location separately from historical relationship facts.

#### Scenario: Child remains live through restart
- **WHEN** a bound child remains live while its parent lead is restarted
- **THEN** its child identity and logical parent relationship remain unchanged, and no child lifecycle operation is dispatched

#### Scenario: Child evidence is unavailable after restart
- **WHEN** the daemon cannot freshly verify a child after restarting its parent
- **THEN** the relationship remains stored while the child’s current liveness/location is reported unknown or stale, not stopped or detached

### Requirement: Restart outcome does not alter assignment or execution facts
A physical restart result SHALL describe only the daemon’s lifecycle effects and verified bindings. It SHALL NOT claim assignment completion, answer pending questions, or synthesize or overwrite execution and assignment publications. Radar determines physical restart eligibility from daemon-owned provenance; Herdsman remains responsible for assignment state and reconciliation.

#### Scenario: Successful physical restart with unresolved assignment
- **WHEN** the daemon restarts a lead whose assignment is still active or unsettled
- **THEN** it reports physical restart completion while preserving the assignment and execution facts unchanged
