# Contract inventory

Scope ledger for `tasks.md`. A row is done when its handler decodes params once, reaches its result without re-reading serialized state, and its Rust consumers read the typed value. Methods come from `ops::classify` and `Operation`; reads are never recorded.

## Shared pieces

| Piece | State |
|---|---|
| Operation record view | **Typed.** `RequestRecord::public()` builds the published record (launch `env`/`argv` stay in the stored record); it replaced a serialize-then-edit function. |
| Verdict | **Typed.** `RequestRecord::settled()` returns the outcome only for a completed record; the UI status and the socket client both read it. |
| Operation answer | **Typed.** `store::Answer` is the one reading of an operation response's `result`. |
| Refusal code | **Typed.** `ErrorBody::code()` reads the wire word through `Code`; an unknown word is `None`, not a refusal. |
| `RequestRecord.method`, `category`, `effects` | String vocabularies. Persisted; historical values must stay readable (task 5.1). |
| Admission result | `Admission::Answered(Response)`. Still wire-shaped (tasks 1.2, 5.2). |
| Operation handlers | Return `Response` built with `json!` (tasks 1.2 to 4.3). |

## Effectful operations (`Operation`)

| Operation | Handler | Consumers reading its answer | Task |
|---|---|---|---|
| `Cancel`, `Stop` | `subject_control::execute_subject_control`, tuple `Settlement` | UI `response_status`, client `Settled::of` | 1.2 (consumers done) |
| `Spawn`, `Enroll`, `Restart`, `Abandon` | `launch`, `subject_control`, `admission` | same, plus `spawn_answer` re-wrapping | 2.1 to 2.3 |
| `Create`, `Close`, `Focus`, `Input`, `Report` | `places`, `input` | client `Settled::{create,close,focus,input,report}` | 3.2 |
| `CreateContainer`, `DeleteContainer`, `ReloadContainer` | `places` | client controls, UI `ui::execute` | 3.1 |

## Reads

| Method group | Producer | Remaining wire-shaped parts | Task |
|---|---|---|---|
| `mux.request.get/list` | `reads` | Done as far as the record goes; the `requests` envelope is `json!` | 3.3 |
| `topology.get/list/reconcile`, `lifecycle.get/list/reconcile`, `spawn.get/list` | `registry` handlers | `json!` results | 3.3, 2.3 |
| `agent.*`, `process.get`, `subject.*` | `registration`, `subject_reads` | Closed state strings, `json!` results | 4.1 |
| `communication.*` | `handoff` | Hand-built `ErrorBody`, `json!` results | 4.2 |
| `observation.get`, `mux.observe/process_info/output`, `tasks.list`, `events.read` | `reads` | Cursor, freshness and page envelopes | 4.3 |
| `daemon.ping` | `server::handle_line` | Capability list | 4.3 |

## Known sentinel and key-path readers still to retire

| Reader | What it recovers | Task |
|---|---|---|
| `app.rs` focus recovery, `ui_session/actor.rs` test | `/error/code` and `/error/target` from a `no_client` answer | 3.2 |
| `lifecycle.rs` registration and launch answers | `answer["registration_id"]`, `result["argv"]` | 2.1 |

This list is the starting point, not a proof of completeness: each task re-greps its family before closing.
