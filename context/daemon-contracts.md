# Daemon operation contracts

Why the daemon's operation answers are typed Rust values and where JSON is still the contract.

## An operation answers with a typed value; JSON appears only at the wire and in storage

**Id:** d31a5131-62ba-4dd9-ad74-6909146accde
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer priority on typed contracts across the daemon, then the `typed-daemon-contracts` change, 2026-10-28
**Verification:** corroborated — the Rust control-plane, registry and lifecycle suites and 111 SDK checks pass through the typed paths; public recipe redaction is checked on creation and request reads. Two unchanged baseline tmux assertions remain excluded. Privacy corrections to previously exposed claim/recipe fields are listed in the change inventory.
**Revisit when:** a second consumer language needs the answers (pi-radar migration), or a persisted vocabulary must change incompatibly

`control_plane::answer` holds operation and read result shapes; `respond` renders `Result<T, Refusal>` at the wire edge. Recorded operations return `Reply = Result<Answer, Refusal>` and complete through `Settlement`. `PublicRequestRecord` is separate from the private durable record. The UI freezes a typed `command::Command`; its serialized copy is for retry identity, not execution. UI outcomes carry a public record or a coded error and target, retaining the existing answer envelope. Composed reads invoke typed services rather than encoding and decoding another handler's response.

**Reason:** the same meaning was kept in four places: the handler's `json!`, the UI's `outcome_status`, the socket client's `Settled::of`, and the app's `/error/code` probe. A renamed field or message changed behaviour silently, and the public projection of a record was made by serialising it and deleting keys. A closed vocabulary belongs to one Rust type that the compiler checks exhaustively; the wire is a rendering of it.

**Rejected alternative:** one universal `OpResult` envelope for every operation. The operations differ in what they add beside the record (a container, removed containers, a spawn edge, a close), and a union of them is the hand-built JSON again with an extra layer.

**Rejected alternative:** typing every string. `RequestRecord.method` preserves exact retry identity; effect messages and extension data remain free text, not machine-state selectors. The historical-category exception originally used here was removed in the current-model-only decision below.

## A refusal's group is stored as its wire word and read through `Category`

**Id:** 0f6a9c51-2d7e-4b83-9a14-6c3e8d5b7f20
**Type:** constraint
**Status:** superseded
**Superseded by:** 1652121e-ad7b-47df-a521-6e6a1959922a
**Evidence:** confirmed
**Source:** `typed-daemon-contracts` design, 2026-10-28
**Verification:** contradicted — the maintainer rejected legacy support in the final review; the category adapter and its legacy fixture were removed.

This decision preserved unknown category words through a string adapter so historical records could still load. It was superseded by the explicit rejection of legacy support below.

## Required subject identity and current category vocabulary

**Id:** 1652121e-ad7b-47df-a521-6e6a1959922a
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer final-review correction rejecting legacy support, 2026-10-29
**Verification:** corroborated — required identities and the closed enum pass all Rust targets with the two documented baseline tmux exclusions; the SDK generator/typecheck and all 111 tests pass without generated-file changes.

A lifecycle claim requires its actual `subject_id`; reads and public projections never substitute `claim_id`. `Category` is a closed snake-case enum without a historical-value catch-all. Current-format serialization stays the same. Unsupported old records fail validation; no automatic migration or store deletion is added.

**Reason:** subject and execution-attempt identity have different meanings. Treating one as the other hides invalid data, while preserving obsolete category forms adds a second supported model the maintainer does not want.

**Rejected alternative:** optional subject identity with a claim-ID fallback, plus an `Other(String)` category adapter for historical records. Rejected as legacy support, not a requirement of the current model. Unknown external error codes still establish uncertainty rather than safe non-execution; that is a transport safety rule, not legacy decoding.
