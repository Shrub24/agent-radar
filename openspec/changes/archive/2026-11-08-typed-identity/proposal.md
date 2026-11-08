# Proposal

## Why

A subject id, a claim id and a registration id are all canonical UUID strings, so
nothing stops one being passed where another is meant. That has already caused a
real defect: branch close terminated by subject id where termination expected the
execution's registration, so a close could signal a different incarnation from the
one it discovered. The parameters are still named for one kind and passed another
(`terminate(subject_id)` receives a claim id), and the fix so far has been naming
lookups carefully rather than making the mistake unrepresentable.

## What Changes

- Three identity types, `SubjectId`, `ClaimId` and `RegistrationId`, each a
  validated canonical UUID. They serialize as the same string, so stored records,
  the wire and the published schemas do not change.
- Every lifecycle operation, record field and lookup takes the type of the identity
  it acts on. A string becomes an identity only where it enters: request decoding,
  record decoding and id minting.
- A request names an identity by the field it belongs in, and a lookup never
  resolves one kind through another.

Non-goals: changing any record version or the wire revision; typing other
identifiers (pane, container, request ids); a generic id framework.

## Capabilities

### Modified Capabilities

- `managed-execution`: lifecycle operations accept only the identity kind they act on.

## Impact

- `src/identity.rs` (new), `src/control_plane/lifecycle.rs`, the registry and its
  event projections, the server's lifecycle, spawn, close and launch handlers, the
  public DTOs that carry these ids, and the UI session's execution references.
- Roughly 1,060 references across 45 files, landed in stages that each pass the
  gate on their own.
