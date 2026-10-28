# Tasks

## 1. First complete path: cancel and stop

- [x] 1.1 Create the method/consumer inventory for the sweep, listing typed and remaining wire contracts, public/private projections and existing compatibility checks. Verify against `ops` classification and server dispatch so every family has an owner and a completion check.
- [ ] 1.2 Migrate cancel/stop through typed params, admission, execution and record/result handling; introduce the minimal shared result/refusal contracts this production path consumes. Preserve same-ID replay and record-before-dispatch; verify existing cancellation/admission tests and add only missing transition evidence. Open: admission still returns `Response` and the handler still settles through the `Settlement` tuple.
- [x] 1.3 Construct the typed public operation projection and migrate operation get/reconciliation, UI status mapping and the Rust operation consumer for this path. Delete replaced key-path/state-string interpretation for migrated responses; verify public redaction, pending versus unknown/refused, and same-ID reconciliation without redispatch. Update the inventory and operation docs. Done for the record projection and both consumers (`RequestRecord::public`/`settled`, `store::Answer`, `ErrorBody::code`); operation docs unchanged because the wire is.

## 2. Remaining lifecycle families

- [ ] 2.1 Migrate spawn and enroll requests, launch context and result handling, including child-close consumers. Keep private launch data out of public responses; verify existing launch/scope/child-close compatibility checks and update the inventory.
- [ ] 2.2 Migrate restart through stop, replacement launch and durable replacement linkage, including its Rust consumers. Verify existing restart replay and no-duplicate-launch tests, and unchanged public response/record shape.
- [ ] 2.3 Migrate abandon and lifecycle get/list plus related reconciliation classifications. Verify unknown remains unknown after administrative release and completed records cannot be resettled; update docs and inventory.

## 3. Topology and physical operations

- [ ] 3.1 Migrate topology create/delete/reload and created-location results, including UI commands and Rust client controls. Verify existing create-focus override/default, survivor reporting and identity/replay checks; preserve actor-owned consent and quiet successful focus.
- [ ] 3.2 Migrate mux create/close/focus/input/report through typed execution and result consumers. Replace message-sentinel branches with structured reasons where needed. Verify existing runtime outcome and no-client recovery checks; update inventory and docs.
- [ ] 3.3 Migrate topology get/list/reconcile read contracts and internal consumers without giving reads operation durability. Verify paging, public record projection and reconciliation behaviour through existing tests.

## 4. Remaining daemon contracts

- [ ] 4.1 Migrate registration/subject/facts/presence request and result paths still relying on JSON probing or closed string state. Verify existing authority and verified-process binding tests, boundary schemas and inventory coverage.
- [ ] 4.2 Migrate communication/handoff/inbox/assignment paths and Rust consumers, preserving subject addressing and writer authority. Verify existing delivery/acknowledgement and replay checks; document unchanged guarantees.
- [ ] 4.3 Migrate observation/process/output, task pages, event pages and capability negotiation to typed producer/consumer contracts where still stringly typed. Preserve bounds, freshness, event cursors/epochs/gaps and unknown-capability handling. Verify existing read/SDK fixtures; do not change codec or delivery policy.

## 5. Persistence and convergence

- [ ] 5.1 Finish the compatible typed vocabulary adapters for operation methods, categories, effects and other owned closed state identified in the inventory. Verify current serialization and supported historical record replay, retaining genuinely open data and exact optional-param retry semantics.
- [ ] 5.2 Remove transitional wire-returning internal adapters and duplicate state interpreters; audit every inventory row to confirm migrated consumers decode only at real boundaries. Verify a scoped source audit and compiler exhaustiveness, not a blanket ban on JSON/strings.
- [ ] 5.3 Run fmt, strict all-target Clippy, all Rust targets, SDK compatibility tests, strict OpenSpec validation and whitespace checks. Reconcile daemon docs and the scope inventory with the final tree; list deliberate remaining boundary representations.

## Workflow follow-up

- Review each vertical change independently; no deployment or main/bookmark advancement without approval.
- Follow with consumer-specific delivery changes described in `design.md`: snapshot push, durable event subscriptions and pi-radar migration have distinct semantics and acceptance criteria.
- Workspace recovery and tmux exit/observation work are separate changes, not hidden completion requirements here.
