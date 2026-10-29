# Spec Delta

## ADDED Requirements

### Requirement: Attention subscriptions are recipient scoped
The trusted socket SHALL accept a verified process opener that upgrades to bounded length-prefixed MessagePack and sends a gap-free recipient attention baseline followed by latest attention updates. Raw events, event cursors and message bodies SHALL NOT be sent on this connection. Loss of the recipient grant SHALL end access.

#### Scenario: Message arrives during subscription establishment
- **WHEN** a recipient message is accepted while the baseline watch is established
- **THEN** the baseline or a subsequent update reports it without a missed transition

#### Scenario: A stale execution retains a socket
- **WHEN** a subscriber loses its verified subject grant
- **THEN** it receives no further recipient information and its subscription ends

### Requirement: Attention delivery is bounded independently of messages
Only attention updates MAY coalesce. Established subscriptions SHALL use a separate budget of 16, a 4 KiB frame cap and bounded writes. Exhaustion SHALL return typed busy without consuming ordinary or UI established capacity. Quiet subscribers SHALL have no silence deadline.

#### Scenario: Slow subscriber
- **WHEN** a subscriber stalls beyond the write deadline
- **THEN** its tasks, connection and permit are released without affecting other subscribers or durable pending messages

#### Scenario: Quiet or capacity-limited subscriber
- **WHEN** all 16 established attention slots are occupied by quiet recipients
- **THEN** they remain attached, another attention opener is refused, and ordinary/UI established capacity remains separate
