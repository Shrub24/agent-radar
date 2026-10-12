# Tasks

## 1. Contract and storage

- [x] 1.1 Add the `facts` snapshot type and storage as a separate channel with its own writer, sequence and lease; bounded fields, unknown-field refusal. Verify bounds, refusal, restart persistence and no merge with execution.
- [x] 1.2 Extend the assignment snapshot with the owner-only optional fields. Verify assignment-only shape, reported owner provenance, unchanged same-UID writer admission, bounds and unchanged execution.

## 2. Reads and events

- [x] 2.1 Expose both in `agent.get`/`agent.list` with provenance and freshness, never the handle, and append pointer events. Verify stale/fresh separation and event pointers.

## 3. Documentation and verification

- [x] 3.1 Update `docs/agent-registration.md`, the canonical fixture (full-comparison validator) and the publisher example.
- [x] 3.2 Run fmt, strict Clippy, locked tests, publisher smoke, strict OpenSpec and the checked Nix build; record in `verification.md`.
