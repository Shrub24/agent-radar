# Proposal

## Why

Agents need addressed communication, not a stream of daemon facts to interpret as notifications. Radar needs one durable message lane whose delivery survives disconnection and execution replacement, while pi-radar owns how those messages affect the live agent.

## What Changes

- Introduce a subject-addressed message lane: immutable typed messages, explicit recipient sets, per-recipient receipts, bounded inbox reads and retained retrieval.
- Keep routing and delivery in the daemon; keep notification, agent wake and assignment policy in pi-radar and its consumer. A transport wake does not mean injecting an LLM prompt.
- Make worker handoff the first message body. **BREAKING:** replace the handoff-specific wire contract and storage path in one coordinated cutover; retain its parent-only publishing policy and receipt semantics. No parallel handoff mailbox or compatibility adapter.
- Add per-recipient attention state to heartbeat replies and optional MessagePack subscriptions. These carry pending count and mailbox revision, never a raw-journal position.
- Drop subscriptions only after continuous Herdsman idle; waiting/blocked states do not count as idle. Heartbeat delivery remains available without a persistent connection.
- Leave raw events, observations, ordinary JSON RPC and the TUI publication lane separate.

## Capabilities

### New Capabilities

- `agent-messages`: addressed message publication, delivery accounting, inbox recovery and consumer ownership.

### Modified Capabilities

- `durable-handoffs`: handoff is a typed message using the shared mailbox, with parent routing and Herdsman settlement ownership unchanged.
- `agent-registration`: heartbeat attention is scoped to the verified recipient subject.
- `daemon-transport`: bounded recipient-attention subscription.
- `pi-radar-sdk`: one message consumer and one activity-controlled attention loop.

## Impact

The communication store and handlers; public DTOs and generated schemas/TypeScript; publication replies; transport admission; pi-radar communication, publication and runtime adapters; the Herdsman handoff port; documentation and fixtures. The MessagePack foundation is on `wip/pi-radar-msgpack`; typed daemon contracts are merged. Implemented for the handoff body; further body kinds and general send grants remain open.

Existing handoff records require an explicit operator decision before cutover: drain the old mailbox, or explicitly export/reset it. The new daemon must refuse incompatible stored communication state, not silently discard pending content. No general migration subsystem is proposed.
