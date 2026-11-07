# Tasks

## 1. Types and lifecycle domain

- [x] 1.1 Add `SubjectId`, `ClaimId` and `RegistrationId` with transparent serde, string schemas, `parse`, `random`, `as_str` and `Display`, and no conversions between them. Test that a non-canonical string is refused on parse and on deserialize, and that serialization is the bare string.
- [x] 1.2 Type the lifecycle records and leaf operations. Gate green; schemas and fixtures unchanged.

## 2. Registry and events

- [ ] 2.1 Type `ProcessFacts`, attachments, placements and their map keys. Gate green.

## 3. Server and wire

- [ ] 3.1 Parse identity fields at request decode, type the public DTOs, and delete every stage-1 call-site conversion. Gate green, including the SDK contract check with no schema diff.
- [ ] 3.2 Delete any lookup that resolves one identity kind through another; a request naming a subject in a claim field is refused, not resolved.

## 4. UI session

- [ ] 4.1 Type execution references in close and restart plans. Gate green.
