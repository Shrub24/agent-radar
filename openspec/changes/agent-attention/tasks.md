# Tasks

Lean implementation and testing discipline: no ordered event feed, no generic subscription framework, no timing thresholds in the routine gate.

## 1. Hint

- [ ] 1.1 Add the optional `attention` field to the publication reply (typed DTO, generated contract, docs). Scope it to the replying registration's subject using the existing pending-handoff read and the journal position. Verify scope, absence for unaffected clients, and unchanged fixtures.
- [ ] 1.2 pi-radar reads the hint from the publisher's heartbeat reply and invokes one deduplicated wake handler by journal position. Verify a pending handoff wakes within one heartbeat with no extra connection.

## 2. Wake subscription

- [ ] 2.1 Add the wake opener and server module reusing `content.rs` framing, class admission, connection-owned tasks and the write deadline; latest-wins position push. Verify newest-only delivery under a stalled reader, isolation of other subscribers, typed `busy` at budget, and that ordinary and UI capacity are unaffected.
- [ ] 2.2 pi-radar wake client on `radar-msgpack.ts`: open while working or blocked, close after the idle hold, reopen from the stored cursor, fall back to the heartbeat on refusal or loss. Verify bigint-safe positions against the shared fixtures, idle close, reopen, and refused-subscription fallback with injected scheduler and dial.

## 3. Closeout

- [ ] 3.1 Update `docs/control-plane.md` and `docs/agent-registration.md`; record the wake-versus-feed and idle-hold decisions in `context/`.
- [ ] 3.2 Routine gate: fmt, strict Clippy, Rust targets, SDK tests and generated-contract check, strict OpenSpec validation, rationale lint, whitespace.
