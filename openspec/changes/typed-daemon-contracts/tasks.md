# Tasks

## 1. First complete path: cancel and stop

- [x] 1.1 Inventory every operation family, public/private boundary and Rust consumer against `ops` classification and server dispatch. Final ownership and deliberate boundary representations are in `inventory.md`.
- [x] 1.2 Carry cancel/stop through typed arguments, `Admission`, `Reply` and `Settlement`, preserving same-ID replay and record-before-dispatch.
- [x] 1.3 Separate `PublicRequestRecord` from private records; migrate get/reconciliation, UI verdict mapping and Rust operation consumers. Remove JSON key-path and state-string interpretation.

## 2. Remaining lifecycle families

- [x] 2.1 Type spawn/enroll and launch/child-close answers and their consumers. Keep launch data private; retain existing scope and authority checks.
- [x] 2.2 Carry restart through typed stop/replacement results and durable replacement linkage. Existing replay/no-duplicate-launch tests remain authoritative.
- [x] 2.3 Type abandon and lifecycle read/reconciliation projections. Administrative release preserves uncertainty; completed records cannot be resettled.

## 3. Topology and physical operations

- [x] 3.1 Type create/delete/reload, created identities and frozen UI commands. Internal callers invoke the same typed implementations as external callers without JSON self-calls. Actor-owned consent and quiet focus remain unchanged.
- [x] 3.2 Type mux operation results and Rust consumers. `FocusOutcome::NoClient` replaces message-text classification; target recovery stays typed through the UI answer.
- [x] 3.3 Type request/topology/spawn pages and reconciliation results without adding read durability.

## 4. Remaining daemon contracts

- [x] 4.1 Type registration, process, subject, channel/context and presence projections. Build observation records directly; classify missing registry authority at its origin rather than by diagnostic text.
- [x] 4.2 Type communication/handoff/assignment reply envelopes and refusals, preserving addressing, writer authority and acknowledgement semantics.
- [x] 4.3 Type observation/process/output composition, task producer/consumer data, event cursors/pages and capability negotiation. Preserve existing bounds, freshness, gaps, unknown-capability handling and delivery policies.

## 5. Persistence and convergence

- [x] 5.1 Add string-compatible `Category` adapters; keep genuinely open historical methods, effect diagnostics and data as strings/JSON. A legacy-shaped record test checks unknown vocabulary preservation; existing replay tests check durable transitions.
- [x] 5.2 Remove transitional wire-returning composition and duplicate interpreters. Retire `Response::typed`, old `managed_control`, response-key-path readers and registry/task JSON round trips. The inventory records the remaining real serialization boundaries.
- [x] 5.3 Final fmt, strict all-target Clippy, Rust targets (543 library tests, 172 control-plane tests plus the other suites), SDK/schema generation and typechecks/111 tests, all 36 strict OpenSpec items, rationale lint and whitespace checks pass. The Rust command excludes exactly the two baseline tmux failures documented below; the unfiltered suite is not claimed green.

## Verification and corrections

- Existing Rust checks cover admission/replay, scope and process authority, restart linkage, public projection, create-focus policy, survivor reporting, read paging and event epochs. The SDK gate checks generated contracts, TypeScript consumers and 111 tests.
- The scripted ping fixture now supplies the task-lane status that the real daemon already emits; no production fallback is added for an incomplete stub.
- The existing recipe test now checks private argv/env are absent from both its creation answer and later operation reads, not only container reads.
- The no-client tmux assertion now checks the typed runtime reason rather than its old diagnostic-string representation.
- Two tmux assertions fail unchanged on the baseline acceptance worktree: `a_workspace_that_cannot_be_focused_is_refused_before_creating_anything` and `create_names_the_locations_it_made`. Per the maintainer's test-deferral direction, the final full Rust command excludes exactly these two. They are not weakened, marked passing or hidden by retries.
- One intermediate systemd-backed lifecycle run could not establish scope membership and returned the existing fail-closed `unknown`. The subsequent full control-plane run passed; no safety assertion was relaxed.
- Privacy corrections discovered by typed projections: managed context now emits the logical subject ID rather than a private claim object; public operation params omit recipe/root replacement argv/env. A typed pane-split plan also fixes its missing `kind` tag. See `inventory.md`.
- No Nix build, deployment or live acceptance is included.

## Workflow follow-up

- Keep this isolated from `main` and leave the local bookmark unmoved until owner review; no push without approval.
- Consumer-specific snapshot push, durable event subscriptions and pi-radar transport migration remain separate follow-ons in `design.md`.
- Workspace recovery and tmux exit/observation changes are not hidden requirements of this sweep.
