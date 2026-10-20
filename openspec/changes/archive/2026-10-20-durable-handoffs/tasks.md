## 1. Daemon contract and persistence

- [x] 1.1 Add the negotiated `communication.handoff.v1` contract and typed publish/inbox/get/ack methods.
- [x] 1.2 Persist immutable bounded handoff records and separate receipts atomically; enforce idempotency, recipient identity, retention, and hard capacity bounds.
- [x] 1.3 Append payload-free publication/receipt pointer events and recover pending work by inbox scan.
- [x] 1.4 Add focused durability, replay, successor identity, acknowledgement, retention, and capacity tests.

## 2. pi-radar SDK and generated contracts

- [x] 2.1 Add explicit Schemars DTOs for publication params, handoff and receipt; regenerate directional schemas and TypeScript types.
- [x] 2.2 Add a process-bound pi-radar communication helper for publish, inbox, get, and ack; do not expose caller-selected sender identity in its API.
- [x] 2.3 Test generated artifact drift, SDK reply validation, and identity binding.

## 3. Herdsman end-to-end integration

- [x] 3.1 Pass `RadarSubject.subject_id` through `RadarWorkerLaunch.parent.subject_id` and `launchRadarWorker` → `client.spawn.managed.parent_subject_id`; publish to the accepted parent and retain staging until confirmed.
- [x] 3.2 Recover the owner's inbox, persist the payload into its existing completion record, then acknowledge; preserve existing wrapup and settlement policy.
- [x] 3.3 Remove only the superseded duplicate handoff transport paths after a tested cutover; retain local pre-publication retry staging.
- [x] 3.4 Test stable-ID retry after an uncertain reply, persistence before acknowledgement, failed persistence remaining unacknowledged, and sender/run mismatch refusal.

## 4. Documentation and gates

- [x] 4.1 Document the public protocol, authority boundary, bounds, retention and recovery semantics.
- [x] 4.2 Run focused tests, package tests, generated-artifact checks, Rust formatting/Clippy/full tests, strict OpenSpec validation, and Nix build.
- [x] 4.3 Keep interactive live Pi/Herdsman/tmux acceptance explicitly deferred if the user has not requested it.

## Deferred

- Live daemon/restart fault injection and interactive Pi/Herdsman/tmux acceptance remain deferred.
- Fix and verify Herdsman idle/busy wake delivery as an independent change.
- UI, question/reply, peer mail, background-task transport, blobs, threads, scheduling, model-based delivery, generic workflow composition.
