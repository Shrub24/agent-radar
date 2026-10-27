# Spec Delta

## Purpose

Replace the predecessor's two-payload JSON publication interface without changing shared navigation, interaction or domain-admission semantics. Apply after `shared-ui-session` has supplied this capability baseline.

## MODIFIED Requirements

### Requirement: Attachment establishes a fresh ordering baseline
Attach and reconnect SHALL establish a fresh attachment UUID and action counter starting at zero, with the first action one. Subscription baseline delivery SHALL establish readiness and the authoritative current head. Old-attachment responses SHALL be discarded. Attaching SHALL NOT reset canonical session state or the actor's publication counter.

#### Scenario: Daemon reconnect
- **WHEN** a frontend reconnects after losing the daemon
- **THEN** it adopts the new attachment and complete subscription baseline rather than comparing counters across daemon lifetimes

#### Scenario: Another dashboard attaches
- **WHEN** a second frontend attaches to an existing session
- **THEN** its subscription receives the current selection and filter without resetting them

### Requirement: Subscription starts from a gap-free current baseline
The TUI `ui.open` subscription role SHALL establish one primary subscription per attachment with an authoritative binary baseline and subsequent latest-state heads, without a registration/baseline gap. Attach acceptance SHALL NOT by itself mark the subscription ready. Local head update IDs SHALL reject old delivery, not request history replay. Reconnect SHALL establish a fresh attachment and baseline.

#### Scenario: Session changes between attach and subscribe
- **WHEN** the actor publishes after attach returns but before subscription registration completes
- **THEN** the subscription baseline or a subsequent pushed head includes that change or a newer coherent state without waiting for an unrelated future update

#### Scenario: Second primary subscription
- **WHEN** another subscription is requested for an attachment that already has its primary subscription
- **THEN** it is refused without replacing the existing subscription

### Requirement: Publications carry ordered content identities
Each published UI head SHALL have one increasing server update ID and identify its observation, tree and UI-session payloads by XXH3-128 hashes of their deterministic MessagePack bytes. Clients SHALL adopt complete heads in the current attachment's ordering scope, tolerate skipped snapshots and reject older updates. Content identity SHALL NOT substitute for action, interaction or domain-request identity.

#### Scenario: Selection-only update
- **WHEN** selection changes without changing observation or visible rows
- **THEN** the head retains the observation and tree hashes while naming updated session content

#### Scenario: Old content repeats
- **WHEN** a lane changes from content A to B and back to A
- **THEN** its return has a newer head update ID and can reuse retained A content

### Requirement: Content references are retrievable with bounded retention
The daemon SHALL make content retrievable before publishing its hash, pin the current head's payloads and bound historical retention by bytes and entries. Missing content SHALL return explicit expiry with the current head for recovery. The client SHALL keep the displayed head and bounded handover work, not historical content. A superseded fetch SHALL NOT roll the display back. Every field of each published payload SHALL contribute to its content identity; publication metadata SHALL remain outside payloads.

#### Scenario: Subscriber lacks a payload
- **WHEN** coalescing skipped inline content required by a head
- **THEN** the client obtains that content by hash or recovers from the current head after expiry

#### Scenario: A fetch finishes late
- **WHEN** a payload fetch finishes after a newer target head has arrived
- **THEN** it is fenced and cannot replace the newer intended display
