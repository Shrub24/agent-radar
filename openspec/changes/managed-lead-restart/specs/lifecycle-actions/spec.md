# Spec Delta

## MODIFIED Requirements

### Requirement: Managed operations are owner-routed
Radar SHALL send confirmed managed close and worker restart through their existing owner-routed operations, with exact subject/run identity and matching confirmation. Worker restart SHALL remain limited to idle retained managed workers. A lead/standalone restart, if enabled, SHALL use the daemon's internal lifecycle operation after explicit TUI confirmation, with exact logical session/process revalidation, daemon-owned launch provenance, and positive old-process exit evidence. It SHALL NOT add a public restart method, CLI command or Herdsman restart operation. Until Herdr provides positive pane-ready/shell-ready acknowledgement for a fresh nonfocused pane, restart SHALL remain unavailable and no pane/process effect may occur. No timing, output heuristic or marker injection may substitute for readiness.

#### Scenario: Idle managed worker restart
- **WHEN** an eligible idle managed worker is confirmed
- **THEN** the request echoes its operation, label and run UUID and asks its exact owner to restart it

#### Scenario: Confirmed managed lead restart
- **WHEN** the operator confirms restart of an eligible lead/standalone subject and the daemon revalidates its logical subject, captured launch specification and stopped prior process
- **THEN** Radar submits one daemon lifecycle request for that frozen subject and reports the daemon's durable outcome without optimistic row replacement

#### Scenario: Busy or parentless target
- **WHEN** restart targets a working/waiting/blocked worker or a lead/standalone session without the new managed-restart eligibility and authorization
- **THEN** Radar refuses without a mux launch fallback

#### Scenario: Busy or ambiguous target
- **WHEN** restart targets a busy session, a stale or ambiguous logical subject, or a process whose exit cannot be confirmed
- **THEN** Radar refuses without launching a replacement

#### Scenario: No mux launch fallback
- **WHEN** the daemon restart method is unavailable or its result is unknown
- **THEN** Radar reports unsupported or unknown and never retries through direct Herdr or another launch path

#### Scenario: Close abandoned assignment
- **WHEN** a managed close is confirmed for active work
- **THEN** Radar warns that the assignment may be abandoned and the owner decides eligibility
