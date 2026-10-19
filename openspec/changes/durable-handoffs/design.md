# Design: Durable Herdsman Handoffs

## Authority and boundaries

Radar owns the immutable published handoff record, its stable ID, resolved sender/recipient identity, pending obligation and recipient receipt. Herdsman owns what the artifact means, whether it is a valid completion, whether a worker may close, and the assignment/lifecycle disposition. A publish result is not an assignment acknowledgement; a communication receipt is not understanding or settlement.

The record is separate from its receipt. A receipt means the recipient-side consumer durably recorded the handoff and can recover it locally. The payload remains retrievable by the addressed recipient after acknowledgement.

## Identity

The pi-radar baseline exposes the process-bound handoff port on its existing shared `RadarRuntimeServices` alongside the publication/execution services, reusing the one existing client and publisher. Herdsman obtains it through `requestRadarRuntime(events)` and does not construct another publisher/client or open the socket. The SDK binds the sender to the process's current registered agent ID; callers do not supply an arbitrary identity to model-facing tools. The daemon resolves the fresh published session context. If it carries a Radar launch marker, the daemon validates the marker against the current durable logical subject, execution record, spawn request and generation. The marker is correlation evidence, not authority. A verified successor may retry under the same logical subject; a stale execution marker is refused. An unmanaged process may be identified by its exact current Pi session.

Managed recipients are addressed by stable logical subject ID so a verified successor can recover the same inbox. In this first handoff contract, a managed sender can publish only to the exact `parent_subject_id` in its Radar lifecycle record; a root sender with no parent cannot use this route. Radar enforces the relationship. `RadarSubject` exposes the current managed `subject_id`; Herdsman supplies it as `RadarWorkerLaunch.parent.subject_id`; pi-radar's `launchRadarWorker` forwards it through `client.spawn` as `ManagedLaunchMetadata.parent_subject_id`. Radar validates and persists that exact value, then injects it into the worker as `PI_RADAR_MANAGED_PARENT_SUBJECT_ID`. The child handoff port reads that process value; completion tools do not accept a destination parameter. The daemon validates it against durable parent metadata, so the environment is a routing hint, not authority. Never substitute `ownerSessionId`, registration ID, pane, or inferred lineage. This first contract has no unmanaged/session recipient route; unresolved or ambiguous logical identity is refused.

## Durable shape

Store one immutable JSON record per communication ID in the daemon state directory, written atomically under a bounded private directory. Store the receipt in a separate atomic record. The existing journal receives small `communication.handoff_published` and `communication.handoff_acknowledged` pointer events containing the message ID and recipient subject when one exists; payloads never enter the journal. Inbox recovery scans durable records, so a missed/coalesced notification cannot lose an obligation.

The initial versioned record includes ID, sender identity, resolved recipient, optional correlation ID, bounded summary, bounded content and safe-range publication time. Receipt includes handoff ID, the current verified recipient identity and safe-range receipt time. Reusing an ID with the same logical sender and identical immutable content is idempotent; reusing it with different content is a definite refusal. Acknowledgement is idempotent and does not overwrite the first receipt.

Content is capped at 384 KiB and summaries at 600 characters. Pending items are capped per recipient; aggregate storage has a bounded record/byte limit. Acknowledged content is retained for 30 days, then may be pruned. Pending content has no age-based expiry; if capacity is exhausted, new publication is refused rather than silently dropping an obligation.

## Protocol and SDK

Negotiate one `communication.handoff.v1` contract. Initial methods: `communication.publish`, `communication.inbox`, `communication.get`, and `communication.ack`. The inbox returns pending IDs only; the recipient fetches each payload individually so multiple large messages cannot exceed the single-line RPC bound. Public DTOs use the existing Schemars-to-JSON-Schema-to-TypeScript pipeline. `pi-radar` exposes publish, inbox, retrieve and acknowledge through the existing shared runtime service, without making callers reimplement socket exchange, client creation or identity derivation.

The producer supplies a stable handoff ID and correlation ID where needed. Consumers recover by inbox scan and may use journal pointer events as a wake hint, never as the source of the obligation. No streaming payloads, event replay framework, or delivery-policy API is introduced.

## Herdsman integration

At worker completion, the worker's Herdsman integration stages the result locally with its existing stable completion ID, attempts publication to the managed parent subject using the worker's process-bound pi-radar identity, and retains staging only until Radar confirms the same immutable record exists. If the reply is lost, it retries the same ID and payload. The owner scans its inbox, validates the handoff against its own assignment/completion state, writes the retrieved payload into its existing completion record and handoff artifact before acknowledging, and then runs existing wrapup/settlement policy. Each local file uses its existing atomic-write path; no cross-file transaction is claimed. Once publication is confirmed, Radar is authoritative for the brokered copy; the local completion record is a consumer-side recovery copy, not a second publication authority.

This sequence avoids coupling broker acknowledgement to `agent_complete`, worker closure, assignment completion, or the Pi wake mode. The existing idle-wake path must be tested independently so an idle owner actually processes an available inbox item.

## Alternatives rejected for this slice

- Relocating Herdsman completion state into Radar: crosses domain authority and duplicates completion semantics.
- Using only events as delivery: loses recoverability when events are coalesced or the recipient is offline.
- Embedding content in journal events: increases journal growth and conflates durable content with event history.
- Exact execution-only addressing: strands pending handoffs after a verified restart.
- Generic blob storage, conversations, routing policies or scheduler abstractions: no current consumer requires them.
- Filesystem polling as a second transport: preserves the duplicated delivery system this work is meant to remove.

## Failure/recovery boundaries

A publish timeout is not proof of non-publication; retry with the same ID and bytes. A definite ID/content conflict stops automatic retry. If the daemon restarts, durable records and receipts remain; inbox scan rebuilds recipient work independently of event cursor. If the recipient restarts, only a verified successor with the same logical subject may retrieve/acknowledge. If the recipient has no current Radar-managed subject identity, inbox/retrieval/acknowledgement is refused. If local receipt persistence fails, do not acknowledge. A receipt does not end the Herdsman completion obligation.
