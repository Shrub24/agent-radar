# Spec Delta

## ADDED Requirements

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
