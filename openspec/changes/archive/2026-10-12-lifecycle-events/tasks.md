# Tasks

## 1. Journal

- [x] 1.1 Add a bounded durable event journal with monotonic sequence, epoch and count/byte retention. Facts are durable before events append; a dirty marker makes an unclean restart start a new epoch. Verify ordering, retention, clean/unclean restart behavior and no secret fields.
- [x] 1.2 Append events from registration, retirement, publications, context, spawn effects and child close outcomes. Verify exactly one event per committed fact and none for refusals.

## 2. Reads

- [x] 2.1 Add `events.read` with bounded cancellable long poll, `cursor_expired`, and the `events_v1` capability. Verify wake-up, empty wait, expiry, shutdown joins and unchanged framing.

## 3. Documentation and verification

- [x] 3.1 Document the contract, resynchronisation guidance and a reconnecting consumer example with a canonical fixture validated against a disposable daemon.
- [x] 3.2 Run fmt, strict Clippy, full locked tests, PTY and publisher smokes, strict OpenSpec and checked Nix; record in verification.md.
