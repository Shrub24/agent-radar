# Tasks

## 1. Types and lifecycle domain

- [x] 1.1 Add `SubjectId`, `ClaimId` and `RegistrationId` with transparent serde, string schemas, `parse`, `random`, `as_str` and `Display`, and no conversions between them. Test that a non-canonical string is refused on parse and on deserialize, and that serialization is the bare string.
- [x] 1.2 Type the lifecycle records and leaf operations. Gate green; schemas and fixtures unchanged.

## 2. Registry and events

- [x] 2.1 Type `ProcessFacts`, attachments, placements and their map keys. Gate green.
- [x] 2.2 Type the registry's own registration record (`Registration.registration_id`) and the registration-keyed registry reads (`get`, `context`, `published_facts`), deleting the `RegistrationId::required` conversions stage 2.1 left at their callers. Gate green.

## 3. Server and wire

- [x] 3.1 Parse identity fields at request decode, type the public DTOs, and delete every stage-1 call-site conversion. Formatting, strict Clippy, all Rust targets, SDK typecheck and 133 tests, unchanged-schema contract check, OpenSpec validation, and checked Nix package build passed. Presentation fixtures now use canonical registration UUIDs; the startup-record refusal test uses the backend-independent bind API, with a failing negative control when record validation is removed.
- [x] 3.2 Delete any lookup that resolves one identity kind through another; a request naming a subject in a claim field is refused, not resolved. Public event claim and registration pointers are typed; process-only events never invent a subject, and assignment publication events name their subject rather than a registration. Delivery/replay and real publication tests pass; restoring the record-key-as-subject fallback fails the regression. Full Rust/SDK/OpenSpec gate and checked Nix package build passed.

## 4. UI session

- [ ] 4.1 Type execution references in close and restart plans. Gate green.
