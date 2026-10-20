# Durable Herdsman Handoffs

## Problem

Herdsman's worker completion/handoff path currently carries payload, delivery and acknowledgement across extension-owned files and lifecycle code. That duplicates durable transport machinery and makes recovery and identity boundaries harder to reason about. Radar already owns stable managed subjects, a durable journal and the local control-plane socket; it should provide the first reusable communication path without taking over Herdsman's completion policy.

## Scope

Add a bounded durable handoff record with immutable content, a stable message ID, sender and recipient identity, optional correlation, a bounded summary, pending-inbox retrieval, explicit read-after-ack, and a separate durable recipient receipt. Make it available through the existing daemon protocol and pi-radar SDK. Integrate Herdsman so a worker's final handoff is published to its managed parent and the owner records the received artifact before acknowledging it.

Radar owns publication, durable content, recipient accounting and pointer events. Herdsman owns handoff validity, worker eligibility, assignment semantics, retry/settlement decisions and lifecycle disposition. Publication and acknowledgement do not authorize process closure or settle an assignment.

## Out of scope

No generic blob store, threads, arbitrary mail types, question/reply UI, delivery scheduler, policy model, MCP/TUI interface, background-task migration, or generic workflow engine. The independently identified Herdsman idle-wake behavior is not implicitly solved by the broker; validate and fix that delivery boundary separately.

## Success

A worker can publish one immutable, bounded handoff to its parent's stable Radar subject using a stable request ID; a lost reply is safely retried; the owner recovers it from its inbox after restart, durably records it locally, then acknowledges receipt; the full payload remains retrievable after acknowledgement; and Herdsman's lifecycle settlement remains a separate decision.