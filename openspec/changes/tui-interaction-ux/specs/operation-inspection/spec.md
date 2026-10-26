# Spec Delta

## Purpose

Keep operations and relevant recorded lifecycle events inspectable with explicit provenance, without creating a second journal or treating observation/diagnostic information as authoritative history.

## ADDED Requirements

### Requirement: Outcomes remain inspectable after transient banners
A frontend SHALL offer a local inspector for retained UI operation references and available domain request records. It SHALL show request identity, operation/target and established status/reason, distinguishing pending, refused, unresolved and completed outcomes. A banner timeout SHALL NOT itself erase the retained outcome or infer success.

#### Scenario: Completion banner expires
- **WHEN** a completed operation's footer line times out
- **THEN** its retained result is still reachable in the local inspector while within established retention

#### Scenario: Operation changed no rows
- **WHEN** a domain operation is refused without changing the fleet
- **THEN** its refusal remains inspectable rather than relying on a tree change to convey the result

### Requirement: Inspection is local and does not execute operations
Opening, scrolling, selecting and closing the inspector SHALL NOT gate other frontends, change shared tree selection or repeat an operation. Local closure SHALL NOT implicitly acknowledge/delete shared records. Any separate shared dismissal action SHALL be explicit.

#### Scenario: Read an uncertain result
- **WHEN** the user opens an unresolved request in the inspector
- **THEN** the UI shows its established uncertainty and identifiers without retrying it

### Requirement: Retention and freshness remain truthful
The inspector SHALL distinguish retained UI information from authoritative domain records and expose stale/unavailable reads and retention limits. After daemon restart it SHALL NOT claim the old volatile session was restored or present unavailable historical associations as complete history. It SHALL preserve existing sanitization and privacy exclusions.

#### Scenario: Old record no longer available
- **WHEN** a requested historical association or record is outside available retention
- **THEN** the inspector states that limitation without inventing an outcome or implying the operation never ran

### Requirement: Relevant events are inspectable with source provenance
A local inspector opened for a subject SHALL expose relevant retained lifecycle events and available operation links, request IDs, timestamps and failure details through existing reads. It SHALL distinguish operation records, recorded events, observation-derived status and available diagnostics. Correlation SHALL use established identities/links, not timestamp proximity or reused pane IDs. Cursor gaps/retention SHALL be explicit.

#### Scenario: Inspect a subject's recent activity
- **WHEN** the inspector is opened for a selected subject with retained linked events and requests
- **THEN** it shows their recorded identities/timestamps and established outcomes without changing shared selection or claiming an exhaustive history

#### Scenario: Status differs from diagnostic text
- **WHEN** diagnostic or observation information is displayed beside an operation record
- **THEN** each source class is labelled and neither is promoted to a lifecycle transition or a new operation outcome

#### Scenario: Event cursor expires
- **WHEN** retained event reads report cursor expiry or a gap
- **THEN** the inspector exposes the missing interval and resumes through existing cursor rules without inferring that no events occurred
