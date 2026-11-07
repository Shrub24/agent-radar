# Design

## Types

`src/identity.rs` defines `SubjectId`, `ClaimId` and `RegistrationId`. Each wraps a
canonical lowercase UUID and offers `parse(&str) -> Option<Self>`, `random()`,
`as_str()` and `Display`. Serialization goes through the bare string, and the
`JsonSchema` impl is `String`'s, inlined: `#[schemars(with = "String")]` on the
type still emits a named definition that every field references, which changes
the published schemas. JSON and the generated schemas are unchanged and the SDK
needs no regeneration.

Deserializing validates. An id in a stored record that is not a canonical UUID
fails to decode, which startup verification already turns into a named refusal.

There is no `From<String>`, no `Deref<Target = str>` and no conversion between the
three types. A conversion that would compile silently is the defect being removed.
`SessionUuid` stays as it is: a session UUID is reported by the agent, not minted by
Radar, and it already has its own type.

## Where strings become identities

1. Request decoding in the server's handlers: a field that names an identity is
   parsed into its type, and a malformed one is refused as `bad_params`, as it is
   today.
2. Record decoding: serde, through the transparent representation.
3. Minting: `random()` at the one site that creates each kind.

Everything after these points passes the typed value. `as_str()` appears only where
an id leaves: paths, event payloads, and the wire.

## Lookups

`process_of_claim` and `process_of_registration` keep their split and take typed
arguments. Any remaining lookup that accepts either kind is deleted, and its callers
pass the kind they hold.

## Staging

The change is too wide for one reviewable diff: a first attempt reached about 220
compile errors and was reverted. Each stage below compiles, passes the gate and is
committed before the next starts.

1. Types and the lifecycle domain: records (`SubjectRecord`, `Claim`,
   `ProcessRecord`) and the leaf operations (`terminate`, `stop`, `cancel_many`,
   `record_exit`, `record_completion`, `place`, `restart`). Callers outside the
   module convert at their call site, marked by `parse`, until their own stage.
2. Registry and event projections: `ProcessFacts`, attachments, placements and
   their keyed maps.
3. Server handlers and public DTOs: the decode boundary moves to the request, and
   the stage-1 call-site conversions are deleted.
4. UI session: execution references in close and restart plans.

## Risks

- A schema drifts if a type loses its `schemars` override. The SDK contract check
  in the gate catches it.
- Staging leaves temporary `parse` calls at module edges. Stage 3 deletes them, and
  the change is not done while any remain.
- A fresh `spawn.create` reserves a subject whose id equalled its launch claim id,
  and enrollment (`lifecycle.rs`, an absent logical subject) did the same, so for
  those records one string was written into both roles. Decided: a subject identity
  invented at either site is minted (`SubjectId::random()`), never borrowed from the
  claim. Records already holding one string in both fields still decode — both are
  canonical UUIDs — and read as "this claim belongs to this subject" rather than as
  a conversion.
- The claim read (`lifecycle::read`) was keyed on a parameter it called
  `subject_id` and stored as the claim id, and built the subject from the same
  string. Decided: it is `read_claim(&ClaimId)`, and the subject comes from the
  attachment, refusing when the attachment names none or names an unreadable one.
  No lookup resolves one identity kind through another.
