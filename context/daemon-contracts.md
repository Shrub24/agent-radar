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

**Rejected alternative:** typing every string. `RequestRecord.method` and `effects` stay strings: they are open sets that older records already hold, and a closed enum would refuse a stored record this build cannot name. `Category` keeps an `Other(String)` for the same reason.

## A refusal's group is stored as its wire word and read through `Category`

**Id:** 0f6a9c51-2d7e-4b83-9a14-6c3e8d5b7f20
**Type:** constraint
**Status:** active
**Evidence:** confirmed
**Source:** `typed-daemon-contracts` design, 2026-10-28
**Verification:** corroborated — `legacy_record_vocabulary_remains_readable_and_lossless` checks an old-shaped record with an unknown category, method and effect; the existing store reopen/replay tests cover persisted operation transitions. No older binary was run against a new store.

`Category` serialises as a string and an unknown word survives a read and a write unchanged. Do not turn it into a closed enum deserialised by name: a record an older or newer build wrote would stop loading, and the store reads every record at start.
