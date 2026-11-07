# Contract inventory

The sweep follows `ops::classify` and `Operation`. Effectful operations still record before dispatch; reads gain no durability.

## Shared contracts

| Boundary | Final owner |
|---|---|
| Wire request | `protocol::Request` retains the original params for exact retry comparison. Handlers decode domain arguments at ingress; internal UI commands use `command::Command` and enter the same typed implementations without decoding their serialized copy. |
| Admission | `Admission::Answered(Box<Reply>)`, `Unsupported`, or `Dispatch`; no serialized response in the admission decision. |
| Completion | `Settlement` carries the typed outcome and category plus human-readable diagnostics/effects. Every operation completion uses it. |
| Durable/public record | `RequestRecord` owns transitions and private retry contents. `PublicRequestRecord` is a separate typed projection without persistence/transition methods. Public launch argv/env are omitted, including recipe data. |
| Result | `answer::Reply` is `Result<Answer, Refusal>` for recorded operations. Additional read/registry/communication result shapes are named structs. `respond` serializes at the transport edge. |
| Rust verdict | `settled()` reads only completed records. UI and socket consumers map the typed verdict rather than interpreting JSON state. |
| Error | `Code` for protocol refusals; `registry::Error` distinguishes missing authority from rejected writes at their origin. Unknown wire error codes remain uncertain. |
| UI outcome | Typed `CommandAnswer` retains the existing `answer.result` / `answer.error` envelope. Focus carries `Target` and a typed `NoClient` runtime outcome, not a diagnostic-message sentinel. |

## Operations and consumers

| Family | Implementation and consumers | Status |
|---|---|---|
| Cancel / stop | `subject_control`, `admission`, UI, Rust lifecycle controls | Typed completion/admission, same-ID replay unchanged. |
| Spawn / enroll / restart | `launch`, `subject_control`, bootstrap socket, Rust controls | Typed operation and launch answers; replacement link and launch contexts retain their existing durability. |
| Abandon / lifecycle reads / reconciliation | `admission`, `registration`, `sweeper`, store, UI reconciliation | Unknown is not converted to success by administrative release; public claims are explicit structs. |
| Topology create / delete / reload | `places`, `command`, UI frozen plans, Rust container controls | Direct typed UI execution; common creation/deletion/reload implementations with external callers; consent remains actor-owned. |
| Mux create / close / focus / input / report | `places`, `input`, runtime, Rust runtime client | Typed requests at ingress and typed outcomes; no-client handling no longer branches on message text. |
| Request/topology/spawn reads | `reads`, `registration`, Rust client | Named public record/page/topology shapes; no new operation records for reads. |
| Registration / subject / publication / presence | `registration`, `registry_read`, `subject_reads`, `sweeper` | Typed registration/channel/context/process projections. Observation builds its registry projection directly rather than serializing and decoding it. Missing writer/registration classification is typed. |
| Communication / handoff / assignment | `handoff`, existing communication/subject/publication models | Named reply envelopes, existing typed authority and acknowledgement models; protocol refusals rendered centrally. |
| Observation / process / output | observation and public-contract modules, `reads`, `subject_reads`, Rust runtime client | Existing typed observation contracts retained. Inspect invokes typed output/foreground services directly, not wire handlers. |
| Tasks | ingest, observation/publication, session, frontend, socket task reader | Shared typed task state and session data; snapshot delivery has no JSON reparse. Paging and freshness unchanged. |
| Events | `reads`, store journal, existing event consumers | Named page/cursor/expiry envelopes; epochs, gaps and replay unchanged. |
| Capabilities | `PingResult`, `Primitives`, `Contract`, Rust handshake | Known capabilities interpreted through existing enums. Unknown advertised contract names remain tolerated. |

## Deliberate boundary representations

- Raw canonical params remain JSON in the retry record. Re-encoding external params before comparison would collapse meaningful omitted/null distinctions.
- Stored method words preserve exact request identity; effect diagnostics and extension provenance remain open text, not machine-state selectors. Unknown advertised contract names are ignored rather than enabling unsupported features.
- `Category` is a closed snake-case enum. Claims require `subject_id`; no projection or execution path substitutes `claim_id`. No legacy decoder or historical-category catch-all is retained.
- `ErrorBody` retains an unknown code word at the socket boundary; consumers classify known words through `Code` and treat unknown ones conservatively.
- Publication/report payloads and genuinely open facts remain JSON. The typed snapshot task field keeps its existing open schema annotation; its wire shape is unchanged.
- JSON/MessagePack encoding, request validation, public projection and persistence remain boundaries. This is not a ban on serde or JSON literals in boundary code or tests.

Removed: `outcome_status`, serialize-then-redact public records, UI error key paths, local task-state interpretation, registry projection JSON round trips, the transitional `Response::typed` adapter, and the unused `managed_control` execution path and its unused client helpers.

## Compatibility corrections found during the sweep

- Managed context publication was placing the entire private claim under `management.subject_id`. It now publishes only the logical subject ID, as the field promises.
- Public request params were exposing recipe argv/env (and root-level replacement argv). Public projection now omits them while preserving the original private retry params. The existing recipe integration test checks both the creation answer and a later request read.
- A pane-split UI plan constructed an untagged JSON object. The typed `CreateRequest::PaneSplit` now supplies the required `kind` tag.

These repair existing privacy/shape promises; no operation, delivery mode or durability policy is added.

## Validation limits

Two tmux integration assertions fail unchanged on the baseline acceptance worktree as well: `a_workspace_that_cannot_be_focused_is_refused_before_creating_anything` and `create_names_the_locations_it_made`. The final Rust gate explicitly excludes only these two; they are deferred rather than weakened or silently retried. Nix/deployment/live acceptance are not part of this change's gate.
