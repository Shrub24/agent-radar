# Design

## Context

This scope was inspected against `main` at `a66c785b`, in a separate jj workspace. The graph index describes the original workspace and is older than this baseline; source in this workspace is authoritative.

Observed seams:

| Seam | Current implementation | Consequence |
|---|---|---|
| Request ingress | `protocol::Request`, `ops::classify`, `server::handle_line`, method-local decoding | Some requests are typed, but direct callers still construct wire requests |
| Admission | `server/admission.rs`: `Admission::Answered(Response)` and dispatch tuple | Replay/refusal decisions already carry serialized response structure |
| Operations | `server/{places,launch,subject_control,input,...}.rs` return `Response` | Domain services and wire rendering share a boundary |
| Persistence | `store::RequestRecord`: typed state/outcome, string method/category/effects and JSON params | Partly typed foundation; exact retry contents and old files must remain readable |
| Public projection | `server/reads.rs::public_record` serializes then removes sensitive keys | Privacy is enforced by editing JSON instead of constructing the public view |
| UI consumer | `server/ui.rs::outcome_status` reads `/result/request/outcome` and error strings | An in-process consumer reparses its own daemon's wire response |
| Rust socket consumer | `client/exchange.rs::Settled::of` inspects string state/outcome | Independent interpretation of the same operation record |
| Reconciliation | UI `Owed` retries reads by request ID every five seconds | Correct no-redispatch intent, but status interpretation should use the record type |
| Delivery | `server/reads.rs::read_events`, existing observation reads, duplex TUI transport | Different semantics; not candidates for one undifferentiated subscription |

The current runtime outcomes, record transitions, event journal, bounded transport and attachment supervision are foundations to reuse, not discarded architecture.

## Goals / Non-Goals

**Goals**

- One domain meaning per fact, with explicit wire/persistence adapters.
- A complete sweep, delivered as family-sized vertical slices, not an isolated UI patch.
- Stable protocol, persisted records and execution guarantees throughout migration.
- Minimal ordinary Rust enums/structs with exhaustive matches for closed vocabularies.
- Keep this branch implementable while the owner accepts the deployed TUI baseline.

**Non-goals**

- A new universal operation framework, event bus, repository layer or dependency-injection hierarchy.
- Turning every string ID into a newtype, every diagnostic into a variant, or every JSON value into an enum.
- Rewriting the daemon as an async actor, moving all state behind the UI actor, or replacing `Core` locks without measured need.
- New subscription endpoints, codec migrations, a TypeScript client migration, tmux control mode, exit-evidence changes, recovery or UX changes.
- Changing deletion consent, verified-exit rules, post-create focus, operation retention or retry policy.

## Decisions

### 1. Typed service contracts; serialization at boundaries

Keep the transport envelope at ingress/egress. Decode each method's params into its owned request type once, then call a typed handler. UI plans invoke the same service using typed commands rather than constructing `Request { method, params }`.

Use the existing operation classification as the closed vocabulary; add typed payload variants only with production consumers. Read methods stay separate from effectful commands so reads never acquire operation durability accidentally.

Why: unifying the semantic entry point removes duplicate validation/interpretation without coupling domain code to a socket. Rejected: generic JSON execution plus typed convenience wrappers, which preserves the problematic internal path; a universal trait-based command framework, which adds abstraction without a current need.

### 2. Keep admission, durable progress and delivery certainty separate

Do not replace everything with `Completed/Failed`. Typed answers must represent:

- A refusal before dispatch, including busy and invalid input.
- An existing same-ID record, including unsettled and terminal replay.
- Admitted/claimed/started work whose result is still pending.
- Settled completed/refused/unknown outcomes.
- Failure to establish whether a submitted request reached execution.

Reuse `RecordState`, `RequestOutcome`, runtime outcome types and protocol refusal codes. Admission returns domain values/records, not `Response`. Transport loss is a delivery fact, not a recorded domain verdict. The UI maps these facts to presentation status once; it does not own a second operation state machine.

Why: these distinctions control whether retry or dispatch is safe. Rejected: the earlier illustrative universal `OpResult`, which did not cover the actual lifecycle, and collapsing pending or missing records into refusal.

### 3. Distinct public and private record views

Construct a typed public projection from the stored record; do not serialize the private record and redact keys afterward. Preserve exactly the current omissions of private launch `env`/`argv`, including method-specific forms. Explicit projections share field types, not duplicate transition logic.

Canonical retry contents remain owned by the admission/persistence boundary. They may retain JSON as a compatibility representation; service logic must not inspect them to rediscover domain state. Do not change same-ID comparison by normalizing an omitted option into an explicit value. Keep legacy records readable, including historical values that current code no longer emits.

Closed current categories/effects use typed vocabularies. Historical unknown values, if encountered in supported stored records, remain representable at the persistence boundary rather than being relabelled or dropped. No new unknown-value policy is introduced for the current wire protocol.

Why: public privacy and private replay have different responsibilities. Rejected: serializing the private record to every consumer, which risks leaks, and rejecting legacy state to make a new enum convenient.

### 4. Domain errors are structured where a caller branches

Use existing refusal codes and domain outcomes, with typed data for machine decisions and a human message for display. Keep I/O causes or free diagnostics as such. Remove comparisons to sentinel messages where a concrete branch depends on their meaning, including focus recovery. Distinguish missing records from store/read failures in reconciliation; neither authorizes redispatch.

Why: a renamed message must not alter execution or recovery. Rejected: one enormous error enum across unrelated subsystems and string-to-enum conversions scattered among consumers.

### 5. Complete slices, followed by the rest of the daemon

First slice: cancel/stop from decode through admission and settlement to typed public record and both Rust consumers. Introduce only the shared contract pieces that this path uses. Then migrate restart/spawn/enroll/abandon, topology and runtime operations, and finally registry/communication/read APIs. Shared adapters may support unmigrated wire handlers temporarily, but no migrated family retains two internal authorities; remove transitional adapters by completion.

For every family record the entry point, params, result, public projection, Rust consumers and compatibility checks in an inventory. That inventory is the scope ledger, not a blanket replacement of every `json!` call. Include handoff, input/report, child-close, observation/process/output, tasks, event pages and capability negotiation rather than declaring the sweep complete after lifecycle alone.

Why: the owner requested the full sweep; complete paths expose integration errors sooner than building unused type infrastructure.

### 6. Delivery adoption is the next change, not hidden in typing

After the typed contract spine, scope consumer-led migrations:

| Lane | Preserve | Reuse |
|---|---|---|
| Observation/tasks snapshots | Coherent latest state, freshness and reconnect baseline | Shared immutable payloads, hashes where useful, bounded frames, supervised sockets |
| Lifecycle/communication event feeds | Ordered durable cursors, epochs, retention gaps and replay | Framing, connection budgets and task ownership, not latest-wins coalescing |
| Commands/results | Request identity, admission and uncertainty | Typed services and correlated replies; no automatic resubmission after loss |

Each follow-on names a real consumer and replaces its path completely. Existing external APIs are removed only through an explicit compatibility decision; keeping an observation read API for external users is not keeping two TUI writers. Reuse `content.rs`, transport framing and connection task ownership when they fit; do not generalize the three-lane UI publication into a plugin bus. Pi-radar brings its own TypeScript/u64/MessagePack fixture checks.

Why: mixing new delivery semantics with a large type migration obscures causality and increases acceptance risk. Rejected: convert every daemon endpoint to the TUI protocol or move durable event delivery into the UI actor.

## Migration Plan

Use the separate `agent-radar-daemon-contracts` workspace based on accepted-code `main`; do not move `main` or `wip/shared-ui-session`. No deployed service or live acceptance environment changes.

Land independently compiling vertical changes. Preserve existing protocol and persistence encoding, so rollback means using the previous binary, not rolling back state. Any necessary incompatible schema/storage change stops that slice for explicit review; it is not silently absorbed into this refactor. Keep the owner-visible scope inventory current as each family is migrated.

## Validation

Existing compatibility/schema/SDK checks protect external shape, optional fields, public redaction and supported fixtures. Existing lifecycle tests protect same-ID replay, record-before-effect, terminal immutability, verification, abandon semantics and no redispatch after uncertain delivery. Add only focused missing evidence at changed semantic seams; do not create one integration test for every enum variant.

Routine gates: formatting, strict all-target Clippy, Rust targets, SDK tests, strict OpenSpec validation and whitespace checks. Nix build and live Kitty acceptance are not routine gates for this planning/refactor branch.

## Risks / Trade-offs

- Broad scope can become endless cleanup → family inventory, complete slice boundaries and explicit exclusions.
- Typed defaults can alter retry equivalence → preserve raw canonical retry contents and optional-field semantics at the boundary.
- Public DTO conversion can leak launch data → preserve redaction evidence and never derive public responses from a private record serializer.
- An unknown old category can break startup → compatible persistence adapter; no blanket enum deserialization of historical files.
- A new result shape can imply false refusal → explicit distinction between pending, unavailable evidence and execution uncertainty.
- Socket supervision can be regressed by adjacent cleanup → retain existing transport ownership; change delivery in a separate reviewed slice.
