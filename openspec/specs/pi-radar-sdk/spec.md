# pi-radar-sdk Specification

## Purpose
The consumer-facing pi-radar SDK: launching and controlling child agents by subject id, with launch outcomes owned by the daemon, so consumers never handle registration or claim identifiers.

## Requirements

### Requirement: A launch result names the child by its subject id
pi-radar SHALL return the launched child's subject id from a ready launch, and SHALL treat a bound edge that carries no subject as unresolved rather than substituting its registration.

#### Scenario: A launch result names the subject
- **WHEN** a launch completes
- **THEN** the result carries the child's subject id, taken from the spawn edge

#### Scenario: An edge bound without a subject
- **WHEN** the daemon binds the edge but records no subject
- **THEN** the launch is unresolved and names the missing subject

### Requirement: A launch takes its parent from the caller's own publication
`launch` SHALL derive the parent from the process's own publication and SHALL NOT accept registration or subject identifiers for the parent from the caller.

#### Scenario: A launch without a parent argument
- **WHEN** a consumer launches a worker
- **THEN** the managed parent is the calling process's own subject and the consumer passes no identity for it

#### Scenario: The caller has no managed subject
- **WHEN** the publication has no granted subject
- **THEN** the launch is refused before any request and says so

### Requirement: The handoff sender is a subject id
A received handoff SHALL expose its sender as the sender's subject id, and a consumer matching a handoff to a child SHALL compare subject ids.

#### Scenario: A handoff from a restarted child
- **WHEN** a child publishes a handoff after its process was replaced
- **THEN** the sender equals the subject id the consumer recorded at launch

### Requirement: The spawn edge records the bound child's subject
The daemon SHALL record the bound child's subject id on the spawn edge's `bound` record, and the public spawn contract and generated schemas SHALL carry it as an optional field.

#### Scenario: A bound edge
- **WHEN** a managed child presents its registration and the edge binds
- **THEN** `bound` carries that registration's granted subject id

### Requirement: A consumer lists its own children
pi-radar SHALL list the calling process's children through Radar's subject-derived child read, each with its subject id, Pi session id, attachment state, current location when known and label. The default SHALL include retained children; live-only selection SHALL be explicit. A caller without a granted subject SHALL be refused.

#### Scenario: Selecting a child by session
- **WHEN** a consumer has a Pi session id for one of its children
- **THEN** the listing names that child's subject id

#### Scenario: A duplicate caller
- **WHEN** the caller's claim is not granted
- **THEN** the listing is refused

### Requirement: Child control is addressed by subject id
pi-radar SHALL address assign, retire, inspect and observe of a child by subject id through daemon subject operations. It SHALL NOT receive, resolve or cache child registration or claim ids, choose a current process from spawn history, or require caller-supplied session identity to validate the target. Its own registration and claim SHALL remain self-publication authority.

#### Scenario: A restarted child keeps its key
- **WHEN** a child's process is replaced and the same subject attaches under a new registration
- **THEN** each operation by the recorded subject id reaches the new process

#### Scenario: A resumed subject has several edges
- **WHEN** a subject has more than one bound edge
- **THEN** Radar resolves the current attachment from subject authority and pi-radar does not rank edges

#### Scenario: A child with no subject on its edge
- **WHEN** an unbound launch cannot yet name a granted child
- **THEN** the SDK reports launch state without substituting a process identifier as the child key

### Requirement: An assignment follows the agent across a restart
pi-radar SHALL publish child assignments by subject to Radar, which SHALL own assignment retention and delivery across process replacement. SDK reads SHALL have no assignment publication side effects and SHALL NOT keep process-specific child writers or replay assignments when a process changes.

#### Scenario: A child restarts with a retained assignment
- **WHEN** a child with a still-valid published assignment attaches under a new registration
- **THEN** Radar delivers that assignment to the granted attachment without an SDK read or replay

#### Scenario: A duplicate holder
- **WHEN** another process opens the same session and is refused the subject
- **THEN** it receives no assignment

### Requirement: One mailbox consumer owns message incorporation
pi-radar SHALL reconcile pending messages on startup and from heartbeat or push attention through one serialized drain. The typed consumer SHALL confirm durable capture before receipt. Unsupported bodies and consumer failures SHALL remain pending; receipt SHALL NOT itself prompt the model or settle domain work.

#### Scenario: Concurrent triggers
- **WHEN** startup, heartbeat and push overlap
- **THEN** one drain incorporates messages and records a bounded rerun rather than starting overlapping consumers

#### Scenario: Retry without attention change
- **WHEN** capture fails and the mailbox revision does not change
- **THEN** the pending message remains eligible for bounded retry

### Requirement: Only continuous Herdsman idle releases the optional push tier
An opted-in consumer SHALL close its subscription after 30 seconds continuously in Herdsman's literal idle status. Waiting or other non-idle statuses SHALL NOT count as idle. Activity resumption SHALL cancel the hold and establish current recipient attention; shutdown and identity replacement SHALL release old work.

#### Scenario: Waiting for children
- **WHEN** the agent remains waiting for a child for longer than 30 seconds
- **THEN** its optional subscription is not closed as idle

#### Scenario: Idle then resumed
- **WHEN** an agent passes the idle hold and later becomes active
- **THEN** it holds no socket during idle and obtains a new baseline on resubscription

### Requirement: Heartbeat fallback preserves delivery
Idle or disconnected consumers SHALL continue discovering pending messages through successful heartbeat replies. Subscription refusal or failure SHALL use bounded backoff, not disable delivery. Revisions SHALL be opaque strings in both codecs; daemon restart SHALL establish a new baseline, not compare unrelated revisions.

#### Scenario: Message arrives without a subscription
- **WHEN** a recipient has no push connection and receives a message
- **THEN** its next successful heartbeat exposes pending work and invokes the same mailbox drain
