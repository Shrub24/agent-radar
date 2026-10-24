## 1. Subject discovery and authority

- [x] 1.1 Build one durable-subject projection and bounded `subject.get/list/children` handlers with live/retained/all selection, replacing public `agent.get/list`. Verify with an existing control-plane scenario extended to cover one subject through grant, exit and retained discovery; document params and outcomes alongside generated schema pairs.
- [x] 1.2 Derive child authority from the caller's current granted attachment and recorded parentage. Extend existing refused-claim/parent tests to show a duplicate caller and an unrelated parent cannot inspect or mutate the child; operator discovery remains distinct.
- [x] 1.3 Preserve internal registration-to-pane joins and duplicate/unresolved fleet diagnostics while subject inventory contains one durable agent. Verify using existing observation/TUI cases rather than adding another process-list API; update the observation contract descriptions.

## 2. Subject-owned assignment

- [x] 2.1 Move assignment storage, writer fencing and leases to the subject; project the same record for the granted child without a second process-owned writable copy. Extend the assignment lifecycle test to replace the child and read its assignment without parent list/observe calls; verify retirement, stale restoration and duplicate refusal using existing coverage where available. Document the ownership and freshness rules.
- [x] 2.2 Route assignment update/retire through authorised subject operations and the existing publication machinery. Verify a retained child accepts the owner's update and a replacement cannot make an expired assignment fresh; publish params/result schema pairs and contract advertisement together.

## 3. Child operations and SDK simplification

- [x] 3.1 Resolve child observe/inspect and launch status inside Radar; consumer results expose subject state without child registration/claim ids. Reuse bounded mux reads and existing attachment validation. Verify a replaced attachment cannot produce mixed-incarnation inspection and keep launch granted/withdrawn tests passing; document detached/unknown outcomes.
- [x] 3.2 Switch pi-radar children/list/observe/inspect/assignment/restart and launch readiness to subject operations, retaining only self registration/claim awareness. Delete edge ranking, process-specific child writers, assignment-follow-on-read and caller session matching. Update existing SDK tests to demonstrate reads are side-effect free and child replacement needs no rebinding; update the SDK README and examples in this slice.

## 4. Matching release and acceptance

- [ ] 4.1 Regenerate and check public schemas/contracts, update wire revision to 9 and SDK minimum/runtime versions, and prepare matching next-minor versions and changelog. Update the Herdsman adoption brief with subject-only child calls and remove process-read guidance. Verify contract drift/typecheck and package checks; no legacy aliases or migration scaffold.
- [ ] 4.2 Run the repository Rust/SDK suites, fmt, strict Clippy, strict OpenSpec validation and Nix build. Record remaining evidence gaps without running live tmux/daemon/systemd acceptance; supply the operator a short grant → exit → retained continue → replacement assignment acceptance sequence.

## Acceptance sequence (operator)

Run against a disposable tmux server with this build and its matching pi-radar:

1. Launch a worker under a lead; confirm `subject.list` shows it `live` and
   `subject.children` (from the lead's registration) includes it.
2. End the worker process; confirm the subject reads `detached` under
   `selection: "all"` and is absent from the default `live` list, and that its
   execution facts are not presented as current.
3. Continue the retained subject; confirm the same `subject_id` returns `live`
   with a new registration, and the lead's `subject.child.get` still resolves it.
4. Publish a child assignment, replace the child, and confirm the replacement's
   granted attachment receives the assignment without a parent read.
5. Confirm a non-parent process is refused by `subject.children` and
   `subject.child.get`, and that `agent.get`/`agent.list` are gone
   (`bad_params`), with the daemon reporting wire revision 9.

## Workflow follow-up

- Review the artifacts before applying. Publish tags only on explicit approval.
- The operator runs live acceptance. Archive after the change's review and acceptance requirements are satisfied.
