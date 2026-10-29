# Spec Delta

## ADDED Requirements

### Requirement: Heartbeats expose recipient mailbox attention
A successful managed publication reply SHALL expose attention for the caller's verified subject: pending delivery count and an opaque mailbox revision. Unrelated mailbox or raw-journal activity SHALL NOT change that attention. Unmanaged callers SHALL receive no subject mailbox hint.

#### Scenario: One recipient has pending work
- **WHEN** a message is accepted for subject P but not Q
- **THEN** P's next successful heartbeat reports pending delivery
- **AND** Q's attention does not change because of that message

### Requirement: Attention failure does not undo presence
An attention-read failure SHALL NOT invalidate an already accepted presence publication or be reported as an empty mailbox. The reply SHALL distinguish unavailable attention, and the client SHALL preserve its pending state for retry.

#### Scenario: Mailbox read fails after presence acceptance
- **WHEN** presence was accepted but attention cannot be read
- **THEN** presence remains accepted and attention is explicitly unavailable
